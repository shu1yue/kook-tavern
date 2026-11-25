use anyhow::Result;
use tavern_core::{BoxFuture, LlmProvider};
use tavern_domain::{
    DmStructuredReply, DmTurnOutput, LlmUsage, NpcUpdate, PendingAction, Player, PrivateWhisper,
    RoomState, Scenario, StatePatch, SuggestedCheck, SummaryOutput, TurnRecord,
    format_batched_actions, match_dynamic_worldbook,
};

#[derive(Debug, Default, Clone)]
pub struct MockLlmProvider;

impl LlmProvider for MockLlmProvider {
    fn generate_turn<'a>(
        &'a self,
        scenario: &'a Scenario,
        room: &'a RoomState,
        players: &'a [Player],
        _recent_turns: &'a [TurnRecord],
        actions: &'a [PendingAction],
    ) -> BoxFuture<'a, Result<DmTurnOutput>> {
        Box::pin(async move {
            let joined_actions: String = actions
                .iter()
                .map(|a| format!("{}尝试「{}」", a.character_name, a.action_text.trim()))
                .collect::<Vec<_>>()
                .join("；与此同时，");

            let corpus = format_batched_actions(actions);
            let matched = match_dynamic_worldbook(scenario, &corpus, 2);
            let lore_hint = matched
                .first()
                .map(|e| format!("（触发世界书条目：{}）", e.title))
                .unwrap_or_default();

            let mut state_patches = Vec::new();
            let mut new_clues = Vec::new();
            let mut private_whispers = Vec::new();
            let mut npc_updates = Vec::new();

            if let Some(first_action) = actions.first() {
                let lower = first_action.action_text.to_lowercase();
                if lower.contains("检查")
                    || lower.contains("搜查")
                    || lower.contains("调查")
                    || lower.contains("钥匙")
                {
                    state_patches.push(StatePatch {
                        player_name: first_action.character_name.clone(),
                        hp_delta: 0,
                        san_delta: 0,
                        add_items: vec!["沾着荧光微尘的半截铜钥匙".to_string()],
                        remove_items: vec![],
                        status_note: Some("指尖沾染微弱星雾荧光".to_string()),
                    });
                    new_clues.push("断钥匙背面刻着模糊的编号 03-17".to_string());
                    npc_updates.push(NpcUpdate {
                        name: "老雷恩".to_string(),
                        attitude: Some("警惕".to_string()),
                        status: Some("注意到你们拿起了半截断钥匙".to_string()),
                    });
                    if players
                        .iter()
                        .any(|p| p.character_name == first_action.character_name)
                    {
                        private_whispers.push(PrivateWhisper {
                            player_name: first_action.character_name.clone(),
                            secret: "你借着炉火余光，独独瞥见柜台后的老酒保看到这半截钥匙时，左手下意识按住了台下的霰弹枪。".to_string(),
                        });
                    }
                } else if lower.contains("冲")
                    || lower.contains("打")
                    || lower.contains("踢")
                    || lower.contains("破门")
                {
                    state_patches.push(StatePatch {
                        player_name: first_action.character_name.clone(),
                        hp_delta: -2,
                        san_delta: -1,
                        add_items: vec![],
                        remove_items: vec![],
                        status_note: Some("动作过猛，擦伤了手臂".to_string()),
                    });
                }
            }

            let mut add_flags = Vec::new();
            let mut advance_act = false;
            let mut trigger_ending_id = None;
            let joined_lower = corpus.to_lowercase();
            if joined_lower.contains("推进") || joined_lower.contains("进入下一幕") {
                add_flags.push("突破核心线索节点".to_string());
                advance_act = true;
            }
            if (joined_lower.contains("真结局") || joined_lower.contains("触发结局"))
                && let Some(first_end) = scenario.endings.first()
            {
                add_flags.push("达成终局条件".to_string());
                trigger_ending_id = Some(first_end.id.clone());
            }

            let turn_next = room.turn_number + 1;
            let base_time = room
                .world_time
                .split(" · 推进")
                .next()
                .unwrap_or(&room.world_time);
            let reply = DmStructuredReply {
                scene_title: format!("{} · 第 {} 幕", scenario.name, turn_next),
                narrative: format!(
                    "[Mock DM 离线演练] {}。\n寒风掠过窗棂，烛火剧烈摇曳了一下。周围人的目光齐刷刷聚向你们，空气里弥漫着山雨欲来的紧张气息。{}",
                    joined_actions, lore_hint
                ),
                private_whispers,
                state_patches,
                new_clues,
                npc_updates,
                world_time: Some(format!("{base_time} · 推进{}分钟", turn_next * 10)),
                location: Some(room.location.clone()),
                suggested_actions: vec![
                    "盘问眼前最可疑的人".to_string(),
                    "仔细检查关键痕迹".to_string(),
                    "顺着线索深入调查".to_string(),
                ],
                suggested_check: Some(SuggestedCheck {
                    stat: "侦查".to_string(),
                    reason: "搜寻隐秘细节".to_string(),
                }),
                add_flags,
                advance_act,
                trigger_ending_id,
            };

            Ok(DmTurnOutput {
                reply,
                usage: LlmUsage {
                    prompt_tokens: 1200,
                    completion_tokens: 240,
                    cache_hit_tokens: 800,
                    cache_miss_tokens: 400,
                },
            })
        })
    }

    fn summarize_turns<'a>(
        &'a self,
        _scenario: &'a Scenario,
        existing_summary: &'a str,
        turns: &'a [TurnRecord],
    ) -> BoxFuture<'a, Result<SummaryOutput>> {
        Box::pin(async move {
            let titles: Vec<String> = turns
                .iter()
                .map(|t| format!("第{}轮「{}」", t.turn_number, t.scene_title))
                .collect();
            let mut out = existing_summary.to_string();
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&format!("【阶段摘要】历经：{}", titles.join(" → ")));
            Ok(SummaryOutput {
                summary: out,
                usage: LlmUsage::default(),
            })
        })
    }
}
