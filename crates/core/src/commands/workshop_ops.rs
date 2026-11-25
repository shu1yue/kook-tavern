use anyhow::Result;
use chrono::Utc;
use tavern_domain::{
    CardTheme, EngineResponse, ForgeScenarioOutput, ForgeScenarioRequest, IncomingMessage,
    Scenario, sanitize_scenario_id,
};

use crate::commands::turn_support::{InFlightLlmGuard, check_daily_budget_for_slot};
use crate::commands::workshop::{load_or_new_draft, render_desk, warn_notice};
use crate::engine::GameEngine;
use crate::workshop_ports::ForgeSlotOutcome;

pub(crate) const FORGE_COOLDOWN_SECS: u64 = 60;

/// Read-only readiness check used before emitting a KOOK thinking placeholder for `/workshop forge`.
pub(crate) fn is_forge_placeholder_ready(engine: &GameEngine, user_id: &str) -> bool {
    let has_premise = engine
        .store
        .get_workshop_draft(user_id)
        .ok()
        .flatten()
        .is_some_and(|d| d.premise.trim().chars().count() >= 8);
    if !has_premise {
        return false;
    }
    engine
        .store
        .can_acquire_forge_slot(user_id, FORGE_COOLDOWN_SECS)
        .unwrap_or(false)
}

pub(crate) async fn workshop_forge(
    engine: &GameEngine,
    msg: &IncomingMessage,
) -> Result<EngineResponse> {
    let draft = load_or_new_draft(engine, &msg.user_id)?;
    if draft.premise.trim().chars().count() < 8 {
        return Ok(warn_notice(
            "✏️ 还没填写主线脑洞喵",
            "请先在 `/store` 挑一套世界书并点一个灵感种子按钮，或者发 `/workshop idea <你的主线想法>`，再来点击「🔥 AI 锻造剧本」！",
        ));
    }

    // Acquire in-flight slot and verify daily budget FIRST so concurrent forges cannot race past quota
    // and budget-denied attempts do not burn the user's forge cooldown.
    let in_flight = InFlightLlmGuard::acquire(engine.in_flight_llm.clone());
    if let Some(limit) = check_daily_budget_for_slot(engine, in_flight.prior_in_flight())? {
        return Ok(limit);
    }

    let slot_token = match engine
        .store
        .try_acquire_forge_slot(&msg.user_id, FORGE_COOLDOWN_SECS)?
    {
        ForgeSlotOutcome::Acquired { token } => token,
        ForgeSlotOutcome::Cooldown { wait_secs } => {
            return Ok(warn_notice(
                "⏳ 锻造炉冷却中",
                &format!("为防止连点浪费大模型配额，每位创作者每 {FORGE_COOLDOWN_SECS} 秒可锻造一次，请 **{wait_secs} 秒**后再试喵！"),
            ));
        }
    };

    let orig_updated_at = draft.updated_at;
    let base_pack = draft
        .worldbook_id
        .as_deref()
        .and_then(|id| engine.worldbooks.get(id));
    let req = ForgeScenarioRequest {
        draft: draft.clone(),
        base_pack,
    };

    let output = match engine.llm.forge_scenario(&req).await {
        Ok(out) => out,
        Err(err) => {
            // Transport/upstream error before a paid completion: CAS-release only our own slot token.
            let _ = engine.store.release_forge_slot(&msg.user_id, &slot_token);
            return Ok(EngineResponse::Notice {
                title: "❌ AI 锻造请求暂时失败".into(),
                body: format!("上游接口暂时不可用，未消耗你的冷却次数，请稍后重试：\n> {err}"),
                theme: CardTheme::Danger,
                show_actions: false,
            });
        }
    };

    let (mut sc, usage) = match output {
        ForgeScenarioOutput::Forged { scenario, usage } => (*scenario, usage),
        ForgeScenarioOutput::ValidationFailed { error, usage } => {
            // Upstream LLM charged tokens: record usage AND keep cooldown active to block retry-drain!
            let cny = engine.cfg.estimate_usage_cny(&usage);
            let _ = engine.store.record_usage(&msg.channel_id, &usage, cny);
            return Ok(EngineResponse::Notice {
                title: "❌ AI 锻造结果未通过校验".into(),
                body: format!("模型返回的结构未通过强类型 Schema 校验（已计入配额，请微调脑洞后稍候重试）：\n> {error}"),
                theme: CardTheme::Danger,
                show_actions: false,
            });
        }
    };

    let cny = engine.cfg.estimate_usage_cny(&usage);
    let _ = engine.store.record_usage(&msg.channel_id, &usage, cny);

    ensure_non_builtin_id(engine, &mut sc);
    let summary = format_forged_preview(&sc);

    // Guard against stale-draft clobbering if creator edited/cleared draft while LLM was running
    let mut latest = load_or_new_draft(engine, &msg.user_id)?;
    if latest.updated_at != orig_updated_at {
        return Ok(warn_notice(
            "⚠️ 草稿在锻造期间已被你修改",
            "检测到你在 AI 锻造期间改动或清空了草稿，为避免覆盖你的新编辑，本次旧预览未写入草稿；请确认最新设定后再试喵！",
        ));
    }
    latest.forged_scenario = Some(Box::new(sc));
    latest.forged_summary = summary;
    latest.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&latest)?;
    Ok(render_desk(engine, &msg.channel_id, latest))
}

pub(crate) fn workshop_publish(
    engine: &GameEngine,
    msg: &IncomingMessage,
) -> Result<EngineResponse> {
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    let Some(mut sc) = draft.forged_scenario.as_deref().cloned() else {
        return Ok(warn_notice(
            "🔥 还没有可上架的锻造预览",
            "请先点击「🔥 AI 锻造剧本」（或发 `/workshop forge`），预览无误后再点击「🚀 一键上架」！",
        ));
    };
    ensure_non_builtin_id(engine, &mut sc);
    let allow_overwrite = engine.cfg.is_admin(&msg.user_id);
    if !allow_overwrite && engine.scenarios.get(&sc.id).is_some() {
        sc.id = append_unique_suffix(&sc.id, &msg.user_id);
    }

    match engine.scenarios.publish_community(&sc, allow_overwrite) {
        Ok(published_id) => {
            // Consume forged preview on publish so one draft cannot be re-published 32x to fill the cap!
            draft.invalidate_forged();
            draft.updated_at = Utc::now();
            let _ = engine.store.upsert_workshop_draft(&draft);
            Ok(EngineResponse::Notice {
                title: format!("🚀 剧本《{}》已成功上架酒馆！", sc.name),
                body: format!(
                    "- **剧本 ID**：`{published_id}`（已存入 `scenarios/community/` 并零重启热重载生效）\n\
                     - **体量与规则**：{} 幕 ｜ {} ｜ {} 个结局 ｜ {} 条世界书\n\
                     - **开玩方式**：现在去 **`🎲炉火一号桌`** / **`🕯️迷雾二号桌`** / **`🌌星轨三号桌`** 发 `/start {published_id}`，或点 `/menu` 绿色按钮就能立刻开团喵！",
                    sc.acts.len(),
                    sc.rules.check_system.short_badge(),
                    sc.endings.len(),
                    sc.worldbook.len()
                ),
                theme: CardTheme::Success,
                show_actions: false,
            })
        }
        Err(err) => Ok(EngineResponse::Notice {
            title: "❌ 上架剧本失败".into(),
            body: format!("{err}"),
            theme: CardTheme::Danger,
            show_actions: false,
        }),
    }
}

pub(crate) fn workshop_delete_community(
    engine: &GameEngine,
    msg: &IncomingMessage,
    arg: &str,
) -> Result<EngineResponse> {
    if !engine.cfg.is_admin(&msg.user_id) {
        return Ok(warn_notice(
            "🔒 仅管理员可下架社区剧本",
            "为防止误删他人创作，只有 `ADMIN_USER_IDS` 中的管理员可执行 `/workshop delete <剧本ID>`。",
        ));
    }
    let id = arg.trim();
    if id.is_empty() {
        return Ok(warn_notice("❓ 请指定剧本 ID", "用法：`/workshop delete <社区剧本ID>`"));
    }
    match engine.scenarios.delete_community(id) {
        Ok(true) => Ok(EngineResponse::Notice {
            title: format!("🗑️ 已下架社区剧本 `{id}`"),
            body: "已从 `scenarios/community/` 安全移除并即时刷新剧本库（官方内置剧本不受影响）。".into(),
            theme: CardTheme::Success,
            show_actions: false,
        }),
        Ok(false) => Ok(warn_notice("❓ 未找到该社区剧本", &format!("社区目录里找不到 `{id}`。"))),
        Err(err) => Ok(warn_notice("❌ 无法删除", &err.to_string())),
    }
}

pub(crate) async fn workshop_import_url(
    engine: &GameEngine,
    msg: &IncomingMessage,
    arg: &str,
) -> Result<EngineResponse> {
    if !engine.cfg.is_admin(&msg.user_id) {
        return Ok(warn_notice(
            "🔒 仅管理员可远程导入 SillyTavern 资产",
            "为防止外链滥用，只有 `ADMIN_USER_IDS` 管理员可执行 `/workshop import <白名单HTTPS链接>`（支持 GitHub Raw / Gist / KOOK 附件的 `.json` 或 `.png` 角色卡/世界书）。",
        ));
    }
    let url = arg.trim();
    if url.is_empty() {
        return Ok(warn_notice(
            "📥 SillyTavern 远程导入用法",
            "用法：`/workshop import <HTTPS链接>`\n允许域名：`raw.githubusercontent.com` / `gist.githubusercontent.com` / `img.kookapp.cn` / `file.kookapp.cn` / `media.kookapp.cn`（最大 2 MiB，自动转为本引擎剧本并上架社区库）。",
        ));
    }
    match engine.st_importer.import_from_url(url).await {
        Ok(mut sc) => {
            ensure_non_builtin_id(engine, &mut sc);
            match engine.scenarios.publish_community(&sc, true) {
                Ok(id) => Ok(EngineResponse::Notice {
                    title: format!("📥 SillyTavern 剧本《{}》导入并上架成功！", sc.name),
                    body: format!(
                        "- **剧本 ID**：`{id}`\n- **世界书条目**：{} 条 ｜ **开场地点**：{}\n\n现在去任意跑团桌发 `/start {id}` 即可开玩喵！",
                        sc.worldbook.len(),
                        sc.initial_location
                    ),
                    theme: CardTheme::Success,
                    show_actions: false,
                }),
                Err(err) => Ok(EngineResponse::Notice {
                    title: "❌ 导入剧本上架失败".into(),
                    body: format!("{err}"),
                    theme: CardTheme::Danger,
                    show_actions: false,
                }),
            }
        }
        Err(err) => Ok(EngineResponse::Notice {
            title: "❌ 远程导入失败".into(),
            body: format!("{err}"),
            theme: CardTheme::Danger,
            show_actions: false,
        }),
    }
}

fn ensure_non_builtin_id(engine: &GameEngine, sc: &mut Scenario) {
    if engine.scenarios.is_builtin(&sc.id) {
        let prefix: String = sc.id.chars().take(45).collect();
        sc.id = sanitize_scenario_id(&format!("{prefix}_ws"));
    }
}

fn append_unique_suffix(base: &str, user_id: &str) -> String {
    let mut h: u32 = 0x811c_9dc5;
    for b in user_id
        .as_bytes()
        .iter()
        .chain(Utc::now().timestamp_nanos_opt().unwrap_or(0).to_le_bytes().iter())
    {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    let prefix: String = base.chars().take(36).collect();
    sanitize_scenario_id(&format!("{prefix}_{:04x}", h & 0xffff))
}

fn format_forged_preview(sc: &Scenario) -> String {
    let acts = sc
        .acts
        .iter()
        .enumerate()
        .map(|(i, a)| format!("第{}幕「{}」", i + 1, a.title))
        .collect::<Vec<_>>()
        .join(" → ");
    let roles = sc
        .preset_roles
        .iter()
        .map(|r| r.name.as_str())
        .collect::<Vec<_>>()
        .join(" / ");
    format!(
        "《{}》（`{}`）\n- 简介：{}\n- 幕次：{}\n- 预设职业：{}\n- 地图节点：{} 个 ｜ 结局：{} 重 ｜ 世界书：{} 条",
        sc.name, sc.id, sc.tagline, acts, roles, sc.map_nodes.len(), sc.endings.len(), sc.worldbook.len()
    )
}
