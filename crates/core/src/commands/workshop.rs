use anyhow::Result;
use chrono::Utc;
use tavern_domain::{
    CardTheme, CheckSystem, EngineResponse, IncomingMessage, WorkshopDraft, WorldbookEntry,
    normalize_theme,
};

use crate::commands::workshop_ops::{
    workshop_delete_community, workshop_forge, workshop_import_url, workshop_publish,
};
use crate::engine::GameEngine;

const MAX_CUSTOM_LORE_PER_DRAFT: usize = 6;

/// Browse the open-source Worldbook & Setting Store (`/store [pack_id]`).
pub fn cmd_store(engine: &GameEngine, channel_id: &str, arg: &str) -> EngineResponse {
    let trimmed = arg.trim();
    if !trimmed.is_empty() {
        let Some(pack) = engine.worldbooks.get(trimmed) else {
            return warn_notice(
                "❓ 未找到该世界书包",
                &format!("找不到世界书包 `{trimmed}`，发 `/store` 查看全部开源世界书商店！"),
            );
        };
        return EngineResponse::WorldbookDetail {
            channel_id: channel_id.to_string(),
            pack: Box::new(pack),
        };
    }

    let packs = engine
        .worldbooks
        .list()
        .into_iter()
        .map(|p| p.summary_view())
        .collect::<Vec<_>>();
    if packs.is_empty() {
        return warn_notice(
            "📦 世界书商店暂无条目",
            "当前未加载到 `worldbooks/*.yaml` 开源世界书包，可直接发 `/workshop` 使用空白原创世界观创作喵！",
        );
    }
    EngineResponse::WorldbookStoreMenu {
        channel_id: channel_id.to_string(),
        packs,
    }
}

/// Interactive Guided Scenario Creation Workshop (`/workshop [subcommand]`).
pub async fn cmd_workshop(
    engine: &GameEngine,
    msg: &IncomingMessage,
    arg: &str,
) -> Result<EngineResponse> {
    let trimmed = arg.trim();
    if trimmed.is_empty() || matches!(trimmed.to_ascii_lowercase().as_str(), "desk" | "面板" | "show") {
        let draft = load_or_new_draft(engine, &msg.user_id)?;
        return Ok(render_desk(engine, &msg.channel_id, draft));
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").trim().to_ascii_lowercase();
    let rest = parts.next().map(str::trim).unwrap_or("");

    match sub.as_str() {
        "wb" | "worldbook" | "世界书" => select_worldbook(engine, msg, rest),
        "hook" | "种子" | "钩子" => select_hook(engine, msg, rest),
        "hook1" => select_pack_and_hook(engine, msg, rest, 0),
        "hook2" => select_pack_and_hook(engine, msg, rest, 1),
        "hook3" => select_pack_and_hook(engine, msg, rest, 2),
        "acts" | "幕数" | "篇幅" => set_acts(engine, msg, rest),
        "rule" | "规则" => set_rule(engine, msg, rest),
        "theme" | "主题" => set_theme(engine, msg, rest),
        "title" | "剧名" | "标题" => set_title(engine, msg, rest),
        "idea" | "premise" | "脑洞" | "主线" => set_idea(engine, msg, rest),
        "lore" | "设定" => add_custom_lore(engine, msg, rest),
        "clear" | "reset" | "清空" => {
            let _ = engine.store.delete_workshop_draft(&msg.user_id)?;
            let fresh = WorkshopDraft::new(&msg.user_id);
            Ok(render_desk(engine, &msg.channel_id, fresh))
        }
        "forge" | "gen" | "锻造" | "生成" => workshop_forge(engine, msg).await,
        "publish" | "上架" | "发布" => workshop_publish(engine, msg),
        "delete" | "unpublish" | "del" | "下架" => workshop_delete_community(engine, msg, rest),
        "import" | "导入" => workshop_import_url(engine, msg, rest).await,
        _ => Ok(EngineResponse::Notice {
            title: "🛠️ 剧本工坊指令速查（推荐直接点卡片按钮！）".into(),
            body: [
                "- `/store` — 浏览开源世界书商店（6 大经典世界观底座 + 现成灵感钩子）",
                "- `/workshop` — 打开你的个人 5 步引导创作台",
                "- `/workshop wb <世界书ID|none>` — 选用或清空世界书底座",
                "- `/workshop acts <3|5|6>` — 自由选择短篇 3 幕 / 中篇 5 幕 / 史诗 6 幕",
                "- `/workshop rule <d20|2d6|d100>` — 选择 D&D / 2d6 / CoC 规则体系",
                "- `/workshop title <剧本名>` — 自定义剧本名称",
                "- `/workshop idea <你的主线脑洞>` — 写下开场危机与核心悬念",
                "- `/workshop lore 标题 | 关键词1,关键词2 | 设定内容` — 追加私房世界书（最多 6 条）",
                "- `/workshop forge` — AI 融合世界书与脑洞，一键锻造完整剧本并校验预览",
                "- `/workshop publish` — 将预览剧本一键上架到酒馆剧本库（零重启即时可玩！）",
                "- `/workshop import <URL>` / `/workshop delete <ID>` — 管理员导入 ST 世界书/角色卡或下架社区本",
            ]
            .join("\n"),
            theme: CardTheme::Info,
            show_actions: false,
        }),
    }
}

pub(crate) fn load_or_new_draft(engine: &GameEngine, user_id: &str) -> Result<WorkshopDraft> {
    Ok(engine
        .store
        .get_workshop_draft(user_id)?
        .unwrap_or_else(|| WorkshopDraft::new(user_id)))
}

pub(crate) fn render_desk(
    engine: &GameEngine,
    channel_id: &str,
    draft: WorkshopDraft,
) -> EngineResponse {
    let base_pack = draft
        .worldbook_id
        .as_deref()
        .and_then(|id| engine.worldbooks.get(id))
        .map(Box::new);
    EngineResponse::WorkshopDesk {
        channel_id: channel_id.to_string(),
        user_id: draft.user_id.clone(),
        draft: Box::new(draft),
        base_pack,
    }
}

fn select_worldbook(
    engine: &GameEngine,
    msg: &IncomingMessage,
    arg: &str,
) -> Result<EngineResponse> {
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    let trimmed = arg.trim();
    if trimmed.is_empty() || matches!(trimmed.to_ascii_lowercase().as_str(), "none" | "clear" | "空白") {
        draft.worldbook_id = None;
        draft.invalidate_forged();
        draft.updated_at = Utc::now();
        engine.store.upsert_workshop_draft(&draft)?;
        return Ok(render_desk(engine, &msg.channel_id, draft));
    }
    let Some(pack) = engine.worldbooks.get(trimmed) else {
        return Ok(warn_notice(
            "❓ 未找到该世界书包",
            &format!("找不到 `{trimmed}`，发 `/store` 浏览可选的开源世界书包。"),
        ));
    };
    draft.worldbook_id = Some(pack.id.clone());
    draft.check_system = pack.recommended_system;
    draft.act_count = pack.recommended_acts.clamp(3, 6);
    draft.theme = normalize_theme(&pack.theme).to_string();
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn select_pack_and_hook(
    engine: &GameEngine,
    msg: &IncomingMessage,
    pack_id: &str,
    hook_idx: usize,
) -> Result<EngineResponse> {
    let Some(pack) = engine.worldbooks.get(pack_id.trim()) else {
        return Ok(warn_notice("❓ 世界书包不存在", "请重新发 `/store` 浏览世界书商店。"));
    };
    let Some(hook) = pack.starter_hooks.get(hook_idx) else {
        return Ok(warn_notice("❓ 钩子编号无效", "该世界书暂未配置该编号的灵感种子。"));
    };
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    draft.worldbook_id = Some(pack.id.clone());
    draft.check_system = pack.recommended_system;
    draft.act_count = pack.recommended_acts.clamp(3, 6);
    draft.theme = normalize_theme(&pack.theme).to_string();
    draft.premise = hook.clone();
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn select_hook(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    let Some(wb_id) = draft.worldbook_id.as_deref() else {
        return Ok(warn_notice("ℹ️ 请先选世界书", "先在 `/store` 挑一套开源世界书，再一键选用它的灵感钩子喵！"));
    };
    let Some(pack) = engine.worldbooks.get(wb_id) else {
        return Ok(warn_notice("❓ 世界书包不存在", "请重新从 `/store` 挑选世界书。"));
    };
    let idx = arg.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1));
    let Some(hook) = idx.and_then(|i| pack.starter_hooks.get(i)) else {
        return Ok(warn_notice("❓ 钩子编号无效", "请点卡片上的灵感种子按钮，或输入 `/workshop hook 1`（1~3）。"));
    };
    draft.premise = hook.clone();
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn set_acts(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Ok(n) = arg.trim().parse::<u32>() else {
        return Ok(warn_notice("❓ 无效幕数", "支持 `/workshop acts 3`（短篇）、`5`（中篇）或 `6`（史诗）。"));
    };
    if !matches!(n, 3 | 5 | 6) {
        return Ok(warn_notice("❓ 幕数需为 3、5 或 6", "短篇 3 幕（1晚）、中篇 5 幕（2~3晚）、史诗 6 幕（多晚长团）。"));
    }
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    draft.act_count = n;
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn set_rule(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let Some(sys) = CheckSystem::parse(arg) else {
        return Ok(warn_notice("❓ 未知规则体系", "支持 `/workshop rule d20`、`2d6` 或 `d100`。"));
    };
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    draft.check_system = sys;
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn set_theme(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    if !matches!(arg.trim().to_ascii_lowercase().as_str(), "ember" | "fog" | "metro") {
        return Ok(warn_notice("❓ 未知氛围主题", "支持 `ember`（暖金奇幻）、`fog`（冷蓝悬疑）、`metro`（霓虹科幻）。"));
    }
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    draft.theme = normalize_theme(arg).to_string();
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn set_title(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let clean: String = arg.trim().chars().take(24).collect();
    if clean.chars().count() < 2 {
        return Ok(warn_notice("❓ 剧名太短", "请提供至少 2 个字的剧本名称，例如 `/workshop title 血色钟楼誓约`。"));
    }
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    draft.title = clean;
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn set_idea(engine: &GameEngine, msg: &IncomingMessage, arg: &str) -> Result<EngineResponse> {
    let clean: String = arg.trim().chars().take(500).collect();
    if clean.chars().count() < 8 {
        return Ok(warn_notice(
            "✏️ 脑洞再具体一点喵",
            "请写至少 8 个字的主线想法（例如：`/workshop idea 港督大选前夜灯塔核心被盗，四大公会互相甩锅`），或直接点世界书的灵感种子按钮！",
        ));
    }
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    draft.premise = clean;
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

fn add_custom_lore(
    engine: &GameEngine,
    msg: &IncomingMessage,
    arg: &str,
) -> Result<EngineResponse> {
    let segs: Vec<&str> = arg.split(['|', '｜']).map(str::trim).collect();
    if segs.len() < 3 {
        return Ok(warn_notice(
            "📖 追加私房世界书格式",
            "用法：`/workshop lore 标题 | 关键词1,关键词2 | 设定正文`\n例：`/workshop lore 血红怀表 | 怀表,钟楼,诅咒 | 这块怀表每逢午夜会倒转三分钟，持有者能听到死者的最后遗言`",
        ));
    }
    let title: String = segs[0].chars().take(24).collect();
    let keywords: Vec<String> = segs[1]
        .split([',', '，', '、', '/'])
        .map(|s| s.trim().chars().take(16).collect::<String>())
        .filter(|s| !s.is_empty())
        .take(6)
        .collect();
    let content: String = segs[2..].join(" | ").chars().take(400).collect();
    if title.is_empty() || content.chars().count() < 8 {
        return Ok(warn_notice("❌ 设定标题或正文过短", "标题不能为空，设定正文至少需要 8 个字。"));
    }
    let mut draft = load_or_new_draft(engine, &msg.user_id)?;
    if draft.custom_lore.len() >= MAX_CUSTOM_LORE_PER_DRAFT {
        return Ok(warn_notice(
            "📦 自定义世界书已达上限",
            &format!("单份草稿最多追加 {MAX_CUSTOM_LORE_PER_DRAFT} 条私房世界书（不含所选开源世界书底座）。"),
        ));
    }
    let idx = draft.custom_lore.len() + 1;
    draft.custom_lore.push(WorldbookEntry {
        id: format!("wb_user_{idx}"),
        title,
        keywords,
        always_active: false,
        player_visible: false,
        priority: 95,
        content,
    });
    draft.invalidate_forged();
    draft.updated_at = Utc::now();
    engine.store.upsert_workshop_draft(&draft)?;
    Ok(render_desk(engine, &msg.channel_id, draft))
}

pub(crate) fn warn_notice(title: &str, body: &str) -> EngineResponse {
    EngineResponse::Notice {
        title: title.to_string(),
        body: body.to_string(),
        theme: CardTheme::Warning,
        show_actions: false,
    }
}
