use tavern_domain::{DmStructuredReply, RoomState};
use tracing::warn;

/// Defensively parses LLM JSON output:
/// 1. Direct full-string JSON parse;
/// 2. Markdown code-fence extraction (` ```json ... ``` `);
/// 3. Streaming `serde_json::Deserializer` scan from every `{` offset (immune to stray quotes
///    or malformed earlier `{...}` thought blocks);
/// 4. Graceful fallback to plain narrative if the model ignored JSON formatting.
pub fn parse_dm_reply(raw: &str, room: &RoomState) -> DmStructuredReply {
    let trimmed = raw.trim();

    if let Ok(parsed) = serde_json::from_str::<DmStructuredReply>(trimmed) {
        return normalize_reply(parsed, room);
    }

    for fenced in extract_fenced_blocks(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<DmStructuredReply>(fenced) {
            return normalize_reply(parsed, room);
        }
    }

    for (idx, ch) in trimmed.char_indices() {
        if ch != '{' {
            continue;
        }
        let mut stream =
            serde_json::Deserializer::from_str(&trimmed[idx..]).into_iter::<DmStructuredReply>();
        if let Some(Ok(parsed)) = stream.next() {
            return normalize_reply(parsed, room);
        }
    }

    warn!("LLM 未返回标准 JSON，已自动降级为纯文本叙事输出");
    DmStructuredReply {
        scene_title: format!("{} · 第 {} 幕", room.title, room.turn_number + 1),
        narrative: trimmed.to_string(),
        private_whispers: Vec::new(),
        state_patches: Vec::new(),
        new_clues: Vec::new(),
        world_time: Some(room.world_time.clone()),
        location: Some(room.location.clone()),
        suggested_actions: Vec::new(),
    }
}

fn normalize_reply(mut reply: DmStructuredReply, room: &RoomState) -> DmStructuredReply {
    if reply.scene_title.trim().is_empty() {
        reply.scene_title = format!("{} · 第 {} 幕", room.title, room.turn_number + 1);
    }
    if reply.narrative.trim().is_empty() {
        reply.narrative = "（周围陷入一阵意味深长的沉默，似乎有什么正在暗处酝酿……）".to_string();
    }
    reply
}

fn extract_fenced_blocks(text: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        let after_open = &rest[start + 3..];
        let body_start = after_open.find('\n').map(|i| i + 1).unwrap_or(0);
        let body_slice = &after_open[body_start..];
        if let Some(end) = body_slice.find("```") {
            let candidate = body_slice[..end].trim();
            if !candidate.is_empty() {
                blocks.push(candidate);
            }
            rest = &body_slice[end + 3..];
        } else {
            break;
        }
    }
    blocks
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use tavern_domain::{ChatMode, NarrativeStyle, TurnPhase};

    use super::*;

    fn dummy_room() -> RoomState {
        let now = Utc::now();
        RoomState {
            channel_id: "c1".into(),
            guild_id: "g1".into(),
            scenario_id: "ember_tavern".into(),
            title: "余烬酒馆".into(),
            location: "大厅".into(),
            world_time: "21:00".into(),
            turn_number: 1,
            state_version: 1,
            turn_phase: TurnPhase::Collecting,
            chat_mode: ChatMode::Strict,
            narrative_style: NarrativeStyle::Prose,
            gm_user_id: "u1".into(),
            web_token: "tok".into(),
            clues: vec![],
            summary: String::new(),
            summary_upto_turn: 0,
            active: true,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn parses_json_after_stray_quote_and_thought_braces() {
        let raw = r#"
        思考过程：{检查3"铁钉}，下面给出裁决：
        ```json
        {"scene_title": "钟声响起", "narrative": "木门开了。", "new_clues": ["密道"]}
        ```
        "#;
        let parsed = parse_dm_reply(raw, &dummy_room());
        assert_eq!(parsed.scene_title, "钟声响起");
        assert_eq!(parsed.new_clues, vec!["密道"]);

        // Also works without markdown fences when earlier malformed `{` contains unclosed `"`
        let raw_unfenced =
            r#"前缀 {检查3"铁钉} 正文 {"scene_title":"暗格","narrative":"发现钥匙"} 尾注"#;
        let parsed2 = parse_dm_reply(raw_unfenced, &dummy_room());
        assert_eq!(parsed2.scene_title, "暗格");

        let plain = "老雷恩哼了一声。";
        let fallback = parse_dm_reply(plain, &dummy_room());
        assert_eq!(fallback.narrative, plain);
    }
}
