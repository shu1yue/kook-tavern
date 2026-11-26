use tavern_domain::{EndingBanner, EngineResponse, LiveStoryHud, SuggestedCheck, WorldBulletin};

/// Returns true when KOOK rejects a message via its content moderation filter (`code=40011`).
pub fn is_kook_moderation_error(err: &anyhow::Error) -> bool {
    let s = format!("{err:#}");
    s.contains("40011") || s.contains("请检查消息内容")
}

/// Replaces benign TRPG firearm/explosive/gore terms that commonly trigger KOOK's automated
/// `code=40011` content filter with era-appropriate fantasy/steampunk wording.
pub fn soften_kook_text(input: &str) -> String {
    const RULES: &[(&str, &str)] = &[
        ("霰弹枪", "矮人火铳"),
        ("左轮枪", "转轮火铳"),
        ("持枪", "持铳"),
        ("开枪", "击发火铳"),
        ("拔枪", "拔出火铳"),
        ("举枪", "举起火铳"),
        ("枪声", "铳声"),
        ("枪口", "铳口"),
        ("枪火", "铳火"),
        ("枪击", "火铳射击"),
        ("手枪", "短铳"),
        ("步枪", "长铳"),
        ("炸毁", "摧毁"),
        ("炸掉", "毁掉"),
        ("炸弹", "炼金爆弹"),
        ("炸药", "炼金火药"),
        ("引爆", "触发过载"),
        ("爆炸", "轰鸣震荡"),
        ("爆破", "强行破拆"),
        ("屠杀", "清剿"),
        ("自杀", "自尽"),
        ("尸体", "遗骸"),
        ("鲜血淋漓", "伤痕累累"),
        ("血腥", "惨烈"),
    ];
    let mut out = input.to_string();
    for &(from, to) in RULES {
        if out.contains(from) {
            out = out.replace(from, to);
        }
    }
    out
}

pub fn soften_live_hud(hud: &LiveStoryHud) -> LiveStoryHud {
    LiveStoryHud {
        turn_number: hud.turn_number,
        rule_badge: hud.rule_badge.clone(),
        current_goal: soften_kook_text(&hud.current_goal),
        clock_lines: hud.clock_lines.iter().map(|s| soften_kook_text(s)).collect(),
        party_lines: hud.party_lines.iter().map(|s| soften_kook_text(s)).collect(),
        turn_progress: hud.turn_progress.iter().map(|s| soften_kook_text(s)).collect(),
        opening_roles: hud.opening_roles.clone(),
    }
}

pub fn soften_response(resp: &EngineResponse) -> EngineResponse {
    match resp {
        EngineResponse::StoryTurn {
            channel_id,
            turn_number,
            scene_title,
            location,
            world_time,
            act_badge,
            narrative,
            state_changes,
            whisper_hints,
            suggested_actions,
            suggested_check,
            world_bulletin,
            ending_reached,
            direct_messages,
        } => EngineResponse::StoryTurn {
            channel_id: channel_id.clone(),
            turn_number: *turn_number,
            scene_title: soften_kook_text(scene_title),
            location: soften_kook_text(location),
            world_time: world_time.clone(),
            act_badge: act_badge.as_deref().map(soften_kook_text),
            narrative: soften_kook_text(narrative),
            state_changes: state_changes.iter().map(|s| soften_kook_text(s)).collect(),
            whisper_hints: whisper_hints.iter().map(|s| soften_kook_text(s)).collect(),
            suggested_actions: suggested_actions.iter().map(|s| soften_kook_text(s)).collect(),
            suggested_check: suggested_check.as_ref().map(|c| SuggestedCheck {
                stat: c.stat.clone(),
                dc: c.dc,
                reason: soften_kook_text(&c.reason),
            }),
            world_bulletin: world_bulletin.as_ref().map(|b| WorldBulletin {
                turn_number: b.turn_number,
                masthead: soften_kook_text(&b.masthead),
                headline: soften_kook_text(&b.headline),
                body: soften_kook_text(&b.body),
                faction: soften_kook_text(&b.faction),
            }),
            ending_reached: ending_reached.as_ref().map(|e| EndingBanner {
                id: e.id.clone(),
                title: soften_kook_text(&e.title),
                kind: e.kind.clone(),
                newly_unlocked: e.newly_unlocked,
            }),
            direct_messages: direct_messages.clone(),
        },
        EngineResponse::Notice { title, body, theme, show_actions } => EngineResponse::Notice {
            title: soften_kook_text(title),
            body: soften_kook_text(body),
            theme: *theme,
            show_actions: *show_actions,
        },
        other => other.clone(),
    }
}

/// Guaranteed-deliverable fallback StoryTurn card if KOOK STILL blocks a narrative with `40011`
/// even after softening: preserves all live turn buttons and progress so the table NEVER stalls!
pub fn safe_story_turn_fallback(resp: &EngineResponse) -> EngineResponse {
    let EngineResponse::StoryTurn {
        channel_id,
        turn_number,
        scene_title,
        location,
        world_time,
        act_badge,
        suggested_actions,
        ending_reached,
        ..
    } = resp
    else {
        return resp.clone();
    };
    EngineResponse::StoryTurn {
        channel_id: channel_id.clone(),
        turn_number: *turn_number,
        scene_title: format!("第 {turn_number} 回合 · 剧情已安全入账"),
        location: soften_kook_text(location),
        world_time: world_time.clone(),
        act_badge: act_badge.as_deref().map(soften_kook_text),
        narrative: format!(
            "⚠️ 本轮《{}》部分措辞触发了 KOOK 平台内容审核拦截（40011），但**本轮剧情、道具、线索与掷骰结果已完整保存入账**！\n\n👉 你可以点 **「🪪 我的角色卡」** / 悬浮 HUD 查看最新背包与前情提要，直接点下方按钮继续推演，或点「🎛️ 面板/设置」→「↩️ 回档上轮」换一种表述重演喵！",
            soften_kook_text(scene_title)
        ),
        state_changes: Vec::new(),
        whisper_hints: Vec::new(),
        suggested_actions: suggested_actions.iter().map(|s| soften_kook_text(s)).collect(),
        suggested_check: None,
        world_bulletin: None,
        ending_reached: ending_reached.as_ref().map(|e| EndingBanner {
            id: e.id.clone(),
            title: soften_kook_text(&e.title),
            kind: e.kind.clone(),
            newly_unlocked: e.newly_unlocked,
        }),
        direct_messages: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softens_trpg_firearm_and_explosive_terms_cleanly() {
        let raw = "楼上的枪声又响了一记，老雷恩持枪戒备，修女艾琳企图炸毁钟楼。";
        let fixed = soften_kook_text(raw);
        assert!(!fixed.contains("持枪"));
        assert!(!fixed.contains("枪声"));
        assert!(!fixed.contains("炸毁"));
        assert!(fixed.contains("持铳"));
        assert!(fixed.contains("铳声"));
        assert!(fixed.contains("摧毁"));
    }
}
