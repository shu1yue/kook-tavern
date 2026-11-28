use chrono::Utc;
use tavern_core::GameEngine;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage, ManagedTable};
use tracing::{info, warn};

use crate::api::KookClient;
use crate::cards::render_kook_card_json;

fn build_table_channel_name(scenario_name: &str) -> String {
    let short: String = scenario_name
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '·' | '/' | '\\' | '#' | ':' | '：'))
        .take(8)
        .collect();
    let label = if short.is_empty() { "跑团桌" } else { short.as_str() };
    let tag = (Utc::now().timestamp().rem_euclid(900)) + 100;
    format!("🎲-{label}-{tag}")
}

impl KookClient {
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_create_table_request(
        &self,
        engine: &GameEngine,
        lobby_channel_id: &str,
        guild_id: &str,
        scenario_id: &str,
        scenario_name: &str,
        creator_user_id: &str,
        creator_user_name: &str,
    ) {
        let meta = self.resolve_channel_meta(lobby_channel_id).await;
        let eff_guild = meta
            .as_ref()
            .map(|m| m.guild_id.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(guild_id);
        let parent_id = meta
            .as_ref()
            .map(|m| m.parent_id.as_str())
            .filter(|s| !s.is_empty() && *s != "0");

        let chan_name = build_table_channel_name(scenario_name);
        let new_channel_id = match self
            .create_text_channel(eff_guild, parent_id, &chan_name)
            .await
        {
            Ok(id) => id,
            Err(err) => {
                warn!(
                    lobby_channel_id,
                    guild_id = eff_guild,
                    error = %format!("{err:#}"),
                    "创建 KOOK 独立包间频道失败"
                );
                let fail = EngineResponse::Notice {
                    title: "⚠️ 暂时无法创建独立包间频道".into(),
                    body: format!(
                        "调用 KOOK 建频道接口失败（常见原因：机器人在本服角色尚未开启 **「管理频道」** 权限）。\n\n👉 群主在 KOOK 服务器角色设置里给机器人勾选 **管理频道** 即可使用独立包间；现在也可以先点菜单上的 **「▶️ 本频道开桌」** 原地开玩喵！\n> 详情：{}",
                        err
                    ),
                    theme: CardTheme::Warning,
                    show_actions: false,
                };
                let card = render_kook_card_json(lobby_channel_id, &fail);
                let _ = self.send_channel_card(lobby_channel_id, &card).await;
                return;
            }
        };

        self.cache_channel_guild(&new_channel_id, eff_guild);
        let record = ManagedTable {
            channel_id: new_channel_id.clone(),
            guild_id: eff_guild.to_string(),
            lobby_channel_id: lobby_channel_id.to_string(),
            scenario_id: scenario_id.to_string(),
            scenario_name: scenario_name.to_string(),
            creator_user_id: creator_user_id.to_string(),
            created_at: Utc::now(),
        };
        if let Err(err) = engine.register_managed_table(&record) {
            warn!(new_channel_id, error = %err, "写入包间台账失败，回滚删除新建频道");
            let _ = self.delete_channel(&new_channel_id).await;
            return;
        }
        info!(
            new_channel_id,
            guild_id = eff_guild,
            scenario_id,
            creator = creator_user_id,
            "已创建独立跑团包间"
        );

        // Automatically initialize the scenario inside the brand-new spotless table channel!
        let init_msg = IncomingMessage {
            guild_id: Some(eff_guild.to_string()),
            channel_id: new_channel_id.clone(),
            user_id: creator_user_id.to_string(),
            user_name: creator_user_name.to_string(),
            content: format!("/start {scenario_id}"),
            is_direct: false,
        };
        match engine.handle_message(init_msg).await {
            Ok(start_resp) => {
                if let EngineResponse::StoryTurn {
                    turn_number,
                    ref ending_reached,
                    ..
                } = start_resp
                {
                    let hud = engine.get_live_story_hud(&new_channel_id).ok().flatten();
                    let is_ended = ending_reached.is_some();
                    self.deliver_story_turn_serialized(
                        &new_channel_id,
                        turn_number,
                        is_ended,
                        start_resp,
                        hud.as_ref(),
                        Some(engine),
                    )
                    .await;
                } else {
                    let card = render_kook_card_json(&new_channel_id, &start_resp);
                    let _ = self.send_channel_card(&new_channel_id, &card).await;
                }
            }
            Err(err) => {
                warn!(new_channel_id, error = %err, "新包间自动开团失败");
            }
        }

        // Post a sleek clickable portal card in the lobby pointing directly to the new channel!
        let portal = EngineResponse::Notice {
            title: format!("🚪 独立跑团包间已备好：《{scenario_name}》"),
            body: format!(
                "已由 **{creator_user_name}** 开启一间全新干净的独立包间（绝不混入旧团记录）：\n\n👉 **点击蓝字直接入桌**：(chn){new_channel_id}(chn)\n\n包间里已经自动摆好**序幕剧情卡、无剧透任务书与一键选职业按钮**；跑完后在包间面板或终局卡点 **「🗑️ 拆掉包间」** 即可一键收桌喵！"
            ),
            theme: CardTheme::Success,
            show_actions: false,
        };
        let card = render_kook_card_json(lobby_channel_id, &portal);
        let _ = self.send_channel_card(lobby_channel_id, &card).await;
    }

    pub(crate) async fn handle_close_table_request(
        &self,
        channel_id: &str,
        lobby_channel_id: &str,
        title: &str,
        closed_by_name: &str,
    ) {
        let _ = self.live_cards().take(channel_id);
        match self.delete_channel(channel_id).await {
            Ok(()) => {
                info!(channel_id, title, closed_by_name, "已成功拆除 KOOK 独立包间频道");
                if !lobby_channel_id.is_empty() && lobby_channel_id != channel_id {
                    let note = EngineResponse::Notice {
                        title: format!("🧹 独立包间已拆桌收尾：《{title}》"),
                        body: format!(
                            "本桌已由 **{closed_by_name}** 完成拆桌并清理干净，随时可在大厅点 `/menu` 开启新包间喵！"
                        ),
                        theme: CardTheme::Secondary,
                        show_actions: false,
                    };
                    let card = render_kook_card_json(lobby_channel_id, &note);
                    let _ = self.send_channel_card(lobby_channel_id, &card).await;
                }
            }
            Err(err) => {
                warn!(channel_id, error = %format!("{err:#}"), "删除 KOOK 包间子频道失败");
                let note = EngineResponse::Notice {
                    title: "⚠️ 存档已清空，但自动删频道失败".into(),
                    body: format!(
                        "《{title}》的本局存档与托管记录已安全清空，但调用 KOOK 删除子频道失败（可能缺少管理频道权限）。请管理员手动删掉本子频道即可喵。\n> 详情：{err}"
                    ),
                    theme: CardTheme::Warning,
                    show_actions: false,
                };
                let card = render_kook_card_json(channel_id, &note);
                let _ = self.send_channel_card(channel_id, &card).await;
            }
        }
    }
}
