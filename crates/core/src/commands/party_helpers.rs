use anyhow::Result;
use tavern_domain::{IncomingMessage, PendingAction, Player, Scenario};

use crate::engine::GameEngine;

pub fn ensure_player(
    engine: &GameEngine,
    msg: &IncomingMessage,
    scenario: &Scenario,
) -> Result<Player> {
    if let Some(existing) = engine.store.get_player(&msg.channel_id, &msg.user_id)? {
        if existing.is_afk {
            // Targeted column update ONLY — never overwrites DM stats!
            if let Some(updated) = engine
                .store
                .set_player_afk(&msg.channel_id, &msg.user_id, false)?
            {
                return Ok(updated);
            }
        }
        return Ok(existing);
    }
    let existing_players = engine.store.list_players(&msg.channel_id)?;
    let character_name = unique_character_name(&existing_players, &msg.user_name);
    let p = Player {
        channel_id: msg.channel_id.clone(),
        user_id: msg.user_id.clone(),
        character_name,
        role_desc: "酒馆旅客".to_string(),
        traits: Vec::new(),
        hp: scenario.player_defaults.max_hp,
        max_hp: scenario.player_defaults.max_hp,
        san: scenario.player_defaults.max_san,
        max_san: scenario.player_defaults.max_san,
        inventory: scenario.player_defaults.starter_items.clone(),
        status_note: "正常".to_string(),
        is_afk: false,
    };
    engine.store.upsert_player(&p)?;
    Ok(p)
}

pub fn unique_character_name(existing_players: &[Player], raw_name: &str) -> String {
    let base_name: String = raw_name
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '|' && *c != '｜')
        .take(16)
        .collect();
    let base = if base_name.is_empty() {
        "冒险者".to_string()
    } else {
        base_name
    };
    let mut candidate = base.clone();
    let mut suffix = 2usize;
    while existing_players
        .iter()
        .any(|p| p.character_name.eq_ignore_ascii_case(&candidate))
    {
        candidate = format!("{base}#{suffix}");
        suffix += 1;
    }
    candidate
}

pub fn parse_join_args(arg: &str, fallback_name: &str) -> (String, String, Vec<String>) {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return (fallback_name.to_string(), String::new(), Vec::new());
    }

    let (left, traits_part) = match trimmed.split_once('|').or_else(|| trimmed.split_once('｜')) {
        Some((l, r)) => (l.trim(), Some(r.trim())),
        None => (trimmed, None),
    };

    let mut left_parts = left.splitn(2, char::is_whitespace);
    let name = left_parts
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(fallback_name)
        .to_string();
    let role_desc = left_parts.next().map(str::trim).unwrap_or("").to_string();

    let traits = traits_part
        .map(|s| {
            s.split([',', '，', '、', '/'])
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    (name, role_desc, traits)
}

pub fn format_party_progress(all_players: &[Player], all_pending: &[PendingAction]) -> String {
    all_players
        .iter()
        .map(|p| {
            if p.is_afk {
                return format!("💤 {} (暂离)", p.character_name);
            }
            let done = all_pending
                .iter()
                .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty());
            if done {
                format!("✅ {}", p.character_name)
            } else {
                format!("⏳ {}", p.character_name)
            }
        })
        .collect::<Vec<_>>()
        .join(" ｜ ")
}
