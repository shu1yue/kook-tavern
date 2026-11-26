use tavern_domain::{CheckSystem, DmPatchDelta, DmStructuredReply, RoomState};
use tracing::{info, warn};

/// Defensively parses LLM JSON output with 3-stage resilience:
/// 1. Strict JSON parse (raw, fenced blocks, brace scan);
/// 2. Deterministic LLM JSON repair (stray closing `]`/`}`, trailing commas, unclosed endings);
/// 3. Zero-raw-JSON-leak fallback (extracts `"narrative"` only, NEVER dumps `"private_whispers"`).
pub fn parse_dm_reply_checked(raw: &str, room: &RoomState) -> (DmStructuredReply, bool) {
    let trimmed = raw.trim();

    if let Some(parsed) = try_strict_parse(trimmed) {
        return (normalize_reply(parsed, room), true);
    }

    // Stage 2: deterministic repair for common LLM JSON typos (`]]`, trailing commas, raw newlines)
    let mut repair_candidates = Vec::new();
    repair_candidates.extend(extract_fenced_blocks(trimmed));
    repair_candidates.push(trimmed);
    if let (Some(start), Some(end)) = (trimmed.find('{'), trimmed.rfind('}'))
        && start < end
    {
        repair_candidates.push(&trimmed[start..=end]);
    }
    for (idx, _) in trimmed.char_indices().filter(|(_, c)| *c == '{').take(8) {
        if trimmed[idx..].contains("\"narrative\"") {
            repair_candidates.push(&trimmed[idx..]);
        }
    }
    for cand in repair_candidates {
        let repaired = repair_llm_json(cand);
        if let Ok(parsed) = serde_json::from_str::<DmStructuredReply>(&repaired) {
            info!("已自动修复 LLM JSON 轻微语法偏差，完整恢复全部剧情与状态字段");
            return (normalize_reply(parsed, room), true);
        }
    }

    warn!("LLM 返回内容无法解析为合法 JSON，触发安全降级保护");
    let looks_like_json = trimmed.starts_with('{')
        || trimmed.starts_with("```")
        || (trimmed.contains("\"narrative\"") && trimmed.contains('{'));
    let (fallback_title, fallback_narrative) = if looks_like_json {
        let title = extract_json_string_field(trimmed, "scene_title")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| format!("{} · 第 {} 幕", room.title, room.turn_number + 1));
        let body = extract_json_string_field(trimmed, "narrative")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| {
                "（DM 刚刚整理笔记时漏了一页，剧情状态已安全保全；可点「🎛️ 面板/设置」→「↩️ 回档上轮」重掷本轮喵！）".to_string()
            });
        (title, body)
    } else {
        (format!("{} · 第 {} 幕", room.title, room.turn_number + 1), trimmed.to_string())
    };

    (
        DmStructuredReply {
            scene_title: fallback_title, narrative: fallback_narrative,
            private_whispers: Vec::new(), state_patches: Vec::new(), new_clues: Vec::new(),
            npc_updates: Vec::new(), world_time: Some(room.world_time.clone()),
            location: Some(room.location.clone()), suggested_actions: Vec::new(),
            suggested_check: None, add_flags: Vec::new(), advance_act: false,
            trigger_ending_id: None, world_bulletin: None, reincarnation_echo: None,
            grant_inspiration: Vec::new(),
        },
        false,
    )
}

fn try_strict_parse(trimmed: &str) -> Option<DmStructuredReply> {
    if let Ok(parsed) = serde_json::from_str::<DmStructuredReply>(trimmed) {
        return Some(parsed);
    }
    for fenced in extract_fenced_blocks(trimmed) {
        if let Ok(parsed) = serde_json::from_str::<DmStructuredReply>(fenced) {
            return Some(parsed);
        }
    }
    for (idx, ch) in trimmed.char_indices() {
        if ch != '{' {
            continue;
        }
        let mut stream =
            serde_json::Deserializer::from_str(&trimmed[idx..]).into_iter::<DmStructuredReply>();
        if let Some(Ok(parsed)) = stream.next() {
            return Some(parsed);
        }
    }
    None
}

/// Repairs common LLM JSON typos outside string literals:
/// - drops stray unmatched closing brackets (`]]` or `}}` when stack top differs);
/// - strips trailing commas immediately before `]` or `}`;
/// - closes unclosed strings/brackets on truncation.
pub fn repair_llm_json(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 8);
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escape = false;
    let mut started = false;

    for ch in input.chars() {
        if !started {
            if ch == '{' {
                started = true;
                stack.push('{');
                out.push('{');
            }
            continue;
        }
        if in_string {
            if escape {
                out.push(ch);
                escape = false;
            } else {
                match ch {
                    '\\' => { escape = true; out.push('\\'); }
                    '"' => { in_string = false; out.push('"'); }
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    _ => out.push(ch),
                }
            }
            continue;
        }
        match ch {
            '"' => { in_string = true; out.push('"'); }
            '{' | '[' => { stack.push(ch); out.push(ch); }
            '}' | ']' => {
                let expected = if ch == '}' { '{' } else { '[' };
                if stack.last().copied() == Some(expected) {
                    strip_trailing_comma(&mut out);
                    stack.pop();
                    out.push(ch);
                    if stack.is_empty() { break; }
                }
                // Otherwise drop the stray mismatched closing bracket (e.g. `"]]` inside object)!
            }
            _ => out.push(ch),
        }
    }
    if in_string {
        if escape { out.pop(); }
        out.push('"');
    }
    while let Some(open) = stack.pop() {
        strip_trailing_comma(&mut out);
        out.push(if open == '{' { '}' } else { ']' });
    }
    out
}

fn strip_trailing_comma(buf: &mut String) {
    let trimmed_len = buf.trim_end().len();
    if trimmed_len > 0 && buf.as_bytes()[trimmed_len - 1] == b',' {
        buf.truncate(trimmed_len - 1);
    }
}

/// Safely extracts a top-level JSON string field (unescaping `\n`, `\"`, etc.) without leaking
/// surrounding JSON syntax or sensitive sibling fields like `private_whispers`.
fn extract_json_string_field(raw: &str, field: &str) -> Option<String> {
    let needle = format!("\"{field}\"");
    let pos = raw.find(&needle)?;
    let rest = &raw[pos + needle.len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    if !after_colon.starts_with('"') {
        return None;
    }
    let mut end_idx = None;
    let mut escape = false;
    for (i, ch) in after_colon.char_indices().skip(1) {
        if escape {
            escape = false;
        } else if ch == '\\' {
            escape = true;
        } else if ch == '"' {
            end_idx = Some(i);
            break;
        }
    }
    let slice = &after_colon[..=end_idx?];
    serde_json::from_str::<String>(slice).ok().or_else(|| {
        let escaped = slice.replace('\r', "\\r").replace('\n', "\\n").replace('\t', "\\t");
        serde_json::from_str::<String>(&escaped).ok()
    })
}

pub fn parse_dm_reply(raw: &str, room: &RoomState) -> DmStructuredReply {
    parse_dm_reply_checked(raw, room).0
}

pub fn parse_patch_delta(raw: &str) -> Option<DmPatchDelta> {
    let trimmed = raw.trim();
    if let Some(d) = try_parse_valid_delta(trimmed) {
        return Some(d);
    }
    for fenced in extract_fenced_blocks(trimmed) {
        if let Some(d) = try_parse_valid_delta(fenced) {
            return Some(d);
        }
    }
    let repaired = repair_llm_json(trimmed);
    if !repaired.is_empty()
        && let Some(d) = try_parse_valid_delta(&repaired)
    {
        return Some(d);
    }
    for (idx, ch) in trimmed.char_indices() {
        if ch != '{' {
            continue;
        }
        let mut stream =
            serde_json::Deserializer::from_str(&trimmed[idx..]).into_iter::<serde_json::Value>();
        if let Some(Ok(val)) = stream.next()
            && let Some(d) = value_to_valid_delta(val)
        {
            return Some(d);
        }
    }
    None
}

fn try_parse_valid_delta(s: &str) -> Option<DmPatchDelta> {
    let val: serde_json::Value = serde_json::from_str(s).ok()?;
    value_to_valid_delta(val)
}

fn value_to_valid_delta(val: serde_json::Value) -> Option<DmPatchDelta> {
    let obj = val.as_object()?;
    let has_known_key = obj.contains_key("state_patches")
        || obj.contains_key("location")
        || obj.contains_key("world_bulletin")
        || obj.contains_key("redacted_narrative");
    if !has_known_key {
        return None;
    }
    serde_json::from_value::<DmPatchDelta>(val).ok()
}

fn normalize_reply(mut reply: DmStructuredReply, room: &RoomState) -> DmStructuredReply {
    if reply.scene_title.trim().is_empty() {
        reply.scene_title = format!("{} · 第 {} 幕", room.title, room.turn_number + 1);
    }
    if reply.narrative.trim().is_empty() {
        reply.narrative = "（周围陷入一阵意味深长的沉默，似乎有什么正在暗处酝酿……）".to_string();
    }
    if room.check_system != CheckSystem::D20
        && let Some(ck) = reply.suggested_check.as_mut()
    {
        ck.dc = None;
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
            channel_id: "c1".into(), guild_id: "g1".into(), scenario_id: "ember_tavern".into(),
            title: "余烬酒馆".into(), location: "大厅".into(), world_time: "21:00".into(),
            turn_number: 1, state_version: 1, turn_phase: TurnPhase::Collecting,
            chat_mode: ChatMode::Strict, narrative_style: NarrativeStyle::Prose,
            gm_user_id: "u1".into(), web_token: "tok".into(), clues: vec![], npc_states: vec![],
            suggested_actions: vec![], current_act: 1, world_flags: vec![], ended_id: None,
            ended_title: None, bulletins: vec![], check_system: Default::default(),
            last_rest_turn: 0, summary: String::new(), summary_upto_turn: 0, active: true,
            created_at: now, updated_at: now,
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

        let raw_unfenced =
            r#"前缀 {检查3"铁钉} 正文 {"scene_title":"暗格","narrative":"发现钥匙"} 尾注"#;
        let parsed2 = parse_dm_reply(raw_unfenced, &dummy_room());
        assert_eq!(parsed2.scene_title, "暗格");

        let plain = "老雷恩哼了一声。";
        let fallback = parse_dm_reply(plain, &dummy_room());
        assert_eq!(fallback.narrative, plain);
    }

    #[test]
    fn repairs_deepseek_stray_closing_bracket_and_never_leaks_private_whispers() {
        // Exact production bug from Turn 2: stray `]]` after `new_clues` + trailing comma + raw newline!
        let broken = "{\"scene_title\": \"松脂药与袖袋里的图\", \"narrative\": \"林星河接过灰陶瓶。\n老雷恩冷冷瞥了你一眼。\", \"private_whispers\": [{\"player_name\": \"林星河\", \"secret\": \"三层稳压栓绝密图纸\"}], \"state_patches\": [{\"player_name\": \"林星河\", \"hp_delta\": 0, \"san_delta\": 0, \"add_items\": [\"浸湿的钟楼楼梯简图\"], \"remove_items\": []}], \"new_clues\": [\"信使呼吸渐稳\"]], \"npc_updates\": [{\"name\": \"老雷恩\", \"attitude\": \"中立\", \"status\": \"警觉\",}], \"suggested_actions\": [\"出示断钥匙\", \"听雾中动静\"]}";
        let (repaired, ok) = parse_dm_reply_checked(broken, &dummy_room());
        assert!(ok, "expected stray `]]`, raw newline, and trailing comma to repair cleanly");
        assert_eq!(repaired.scene_title, "松脂药与袖袋里的图");
        assert!(repaired.narrative.contains("老雷恩冷冷瞥了你一眼"));
        assert_eq!(repaired.private_whispers.len(), 1);
        assert_eq!(repaired.new_clues, vec!["信使呼吸渐稳"]);
        assert_eq!(repaired.npc_updates.len(), 1);
        assert_eq!(repaired.suggested_actions.len(), 2);

        // Truly unrepairable JSON MUST hit Stage 3 fallback: extract narrative only, NEVER leak `private_whispers`!
        let mangled = r#"{"scene_title": "暗夜试探", "narrative": "窗外雪花纷飞。", BAD_SYNTAX_HERE, "private_whispers": [{"player_name": "A", "secret": "绝密不能公屏显示"}]}"#;
        let (safe, was_json) = parse_dm_reply_checked(mangled, &dummy_room());
        assert!(!was_json);
        assert_eq!(safe.scene_title, "暗夜试探");
        assert_eq!(safe.narrative, "窗外雪花纷飞。");
        assert!(!safe.narrative.contains("绝密不能公屏显示"));
        assert!(!safe.narrative.contains("private_whispers"));
    }
}
