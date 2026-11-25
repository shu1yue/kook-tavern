use anyhow::Result;
use serde::Serialize;
use tavern_domain::{CharacterStats, RoomState, TurnPhase, WorldBulletin};

use crate::WebState;
use crate::map_view::{ScenarioMapView, build_scenario_map_view};
use crate::views::{AbilityPublicView, ActPublicView};

/// Per-player live floating HUD payload:
/// - Contains THIS authenticated player's private whispers (`me.whispers`), NEVER teammates' whispers;
/// - Strictly spoiler-safe: glossary ONLY exposes `always_active == true` worldbook entries;
/// - Never leaks `gm_user_id`, `guild_id`, `user_id`, or other players' private tokens.
#[derive(Debug, Serialize)]
pub struct PlayerHudView {
    pub title: String,
    pub scenario_id: String,
    pub genre: String,
    pub tagline: String,
    pub theme: &'static str,
    pub location: String,
    pub world_time: String,
    pub turn_number: u32,
    pub turn_phase: &'static str,
    pub check_system: &'static str,
    pub active: bool,
    pub current_act: u32,
    pub acts: Vec<ActPublicView>,
    pub world_flags: Vec<String>,
    pub suggested_actions: Vec<String>,
    pub ended_id: Option<String>,
    pub ended_title: Option<String>,
    pub dashboard_path: String,
    pub me: PlayerSelfView,
    pub party: Vec<TeammateHudView>,
    pub npcs: Vec<NpcDossierView>,
    pub clues: Vec<String>,
    pub glossary: Vec<GlossaryEntryView>,
    pub bulletins: Vec<WorldBulletin>,
    pub map: ScenarioMapView,
    pub recent_turns: Vec<TurnDigestView>,
}

#[derive(Debug, Serialize)]
pub struct PlayerSelfView {
    pub character_name: String,
    pub role_desc: String,
    pub traits: Vec<String>,
    pub stats: CharacterStats,
    pub level: u32,
    pub inspiration: bool,
    pub alignment: String,
    pub proficient_skills: Vec<String>,
    pub pending_levelups: u32,
    pub abilities: Vec<AbilityPublicView>,
    pub hp: i32,
    pub max_hp: i32,
    pub san: i32,
    pub max_san: i32,
    pub inventory: Vec<String>,
    pub status_note: String,
    pub location: String,
    pub is_afk: bool,
    pub action_ready: bool,
    pub pending_action_text: String,
    pub roll_notes: Vec<String>,
    pub whispers: Vec<WhisperItemView>,
}

#[derive(Debug, Serialize)]
pub struct WhisperItemView {
    pub turn_number: u32,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct TeammateHudView {
    pub is_me: bool,
    pub character_name: String,
    pub role_desc: String,
    pub traits: Vec<String>,
    pub hp: i32,
    pub max_hp: i32,
    pub san: i32,
    pub max_san: i32,
    pub location: String,
    pub status_note: String,
    pub is_afk: bool,
    pub action_ready: bool,
    pub roll_notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct NpcDossierView {
    pub name: String,
    pub attitude: String,
    pub attitude_tier: &'static str,
    pub status: String,
    pub related_clues: Vec<String>,
    pub related_whispers: Vec<String>,
    pub related_bulletins: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct GlossaryEntryView {
    pub title: String,
    pub keywords: Vec<String>,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct TurnDigestView {
    pub turn_number: u32,
    pub scene_title: String,
    pub location: String,
    pub world_time: String,
    pub state_summary: String,
    pub narrative_excerpt: String,
}

pub fn build_player_hud_view(
    state: &WebState,
    channel_id: &str,
    user_id: &str,
    room: RoomState,
) -> Result<Option<PlayerHudView>> {
    let players = state.store.list_players(channel_id)?;
    let Some(me_player) = players.iter().find(|p| p.user_id == user_id).cloned() else {
        return Ok(None);
    };
    let next_turn = if room.turn_phase == TurnPhase::Resolving {
        room.turn_number + 2
    } else {
        room.turn_number + 1
    };
    let pending = state.store.list_pending_actions(channel_id, next_turn).unwrap_or_default();
    let turns = state.store.list_recent_turns(channel_id, 6).unwrap_or_default();
    let whispers_raw = state.store.list_player_whispers(channel_id, user_id, 20).unwrap_or_default();
    let scenario = state.scenarios.get(&room.scenario_id);

    let theme = match scenario.as_ref().map(|s| s.theme.trim().to_ascii_lowercase()).as_deref() {
        Some("fog") => "fog",
        Some("metro") => "metro",
        _ => "ember",
    };
    let (genre, tagline) = scenario
        .as_ref()
        .map(|s| (s.genre.clone(), s.tagline.clone()))
        .unwrap_or_default();

    let acts: Vec<ActPublicView> = scenario
        .as_ref()
        .map(|s| {
            s.acts.iter().enumerate().map(|(i, a)| {
                let index = (i + 1) as u32;
                let locked = index > room.current_act && room.ended_id.is_none();
                ActPublicView {
                    index,
                    title: if locked { "未解锁幕次".into() } else { a.title.clone() },
                    goal: if locked { "推进当前主线后解锁".into() } else { a.goal.clone() },
                }
            }).collect()
        })
        .unwrap_or_default();

    // Spoiler-safe public glossary: ONLY `always_active == true` entries!
    let glossary: Vec<GlossaryEntryView> = scenario
        .as_ref()
        .map(|s| {
            s.worldbook.iter().filter(|w| w.always_active).map(|w| GlossaryEntryView {
                title: w.title.clone(),
                keywords: w.keywords.clone(),
                content: w.content.trim().to_string(),
            }).collect()
        })
        .unwrap_or_default();

    let whispers: Vec<WhisperItemView> = whispers_raw
        .into_iter()
        .map(|w| WhisperItemView { turn_number: w.turn_number, content: w.content })
        .collect();

    let npcs: Vec<NpcDossierView> = room.npc_states.iter().map(|npc| NpcDossierView {
        name: npc.name.clone(),
        attitude_tier: classify_attitude(&npc.attitude),
        attitude: npc.attitude.clone(),
        status: npc.status.clone(),
        related_clues: room.clues.iter().filter(|c| mentions_npc(c, &npc.name)).cloned().collect(),
        related_whispers: whispers.iter()
            .filter(|w| mentions_npc(&w.content, &npc.name))
            .map(|w| format!("第{}轮：{}", w.turn_number, w.content))
            .collect(),
        related_bulletins: room.bulletins.iter()
            .filter(|b| mentions_npc(&b.headline, &npc.name) || mentions_npc(&b.body, &npc.name) || mentions_npc(&b.faction, &npc.name))
            .map(|b| format!("{} — {}", b.headline, b.body))
            .collect(),
    }).collect();

    let map = build_scenario_map_view(scenario.as_ref(), &room, &players, &pending, &turns, &room.npc_states);

    let party: Vec<TeammateHudView> = players.iter().map(|p| {
        let p_act = pending.iter().find(|a| a.user_id == p.user_id);
        let loc = if p.location.trim().is_empty() { room.location.clone() } else { p.location.clone() };
        TeammateHudView {
            is_me: p.user_id == user_id,
            character_name: p.character_name.clone(), role_desc: p.role_desc.clone(),
            traits: p.traits.clone(), hp: p.hp, max_hp: p.max_hp, san: p.san, max_san: p.max_san,
            location: loc, status_note: p.status_note.clone(), is_afk: p.is_afk,
            action_ready: p_act.is_some_and(|a| !a.action_text.trim().is_empty()),
            roll_notes: p_act.map(|a| a.roll_notes.clone()).unwrap_or_default(),
        }
    }).collect();

    let my_act = pending.iter().find(|a| a.user_id == user_id);
    let my_loc = if me_player.location.trim().is_empty() { room.location.clone() } else { me_player.location.clone() };
    let me = PlayerSelfView {
        character_name: me_player.character_name, role_desc: me_player.role_desc,
        traits: me_player.traits, stats: me_player.stats, level: me_player.dnd.level,
        inspiration: me_player.dnd.inspiration, alignment: me_player.dnd.alignment,
        proficient_skills: me_player.dnd.proficient_skills,
        pending_levelups: me_player.dnd.pending_levelups,
        abilities: me_player.dnd.abilities.into_iter().map(|a| AbilityPublicView {
            name: a.name, uses_left: a.uses_left, max_uses: a.max_uses,
            recharge: a.recharge.short_label(),
        }).collect(),
        hp: me_player.hp, max_hp: me_player.max_hp, san: me_player.san, max_san: me_player.max_san,
        inventory: me_player.inventory, status_note: me_player.status_note, location: my_loc,
        is_afk: me_player.is_afk,
        action_ready: my_act.is_some_and(|a| !a.action_text.trim().is_empty()),
        pending_action_text: my_act.map(|a| a.action_text.trim().to_string()).unwrap_or_default(),
        roll_notes: my_act.map(|a| a.roll_notes.clone()).unwrap_or_default(),
        whispers,
    };

    let recent_turns: Vec<TurnDigestView> = turns.into_iter().rev().map(|t| {
        let excerpt: String = t.narrative.chars().take(180).collect();
        let suffix = if t.narrative.chars().count() > 180 { "…" } else { "" };
        TurnDigestView {
            turn_number: t.turn_number, scene_title: t.scene_title, location: t.location,
            world_time: t.world_time, state_summary: t.state_summary,
            narrative_excerpt: format!("{excerpt}{suffix}"),
        }
    }).collect();

    let dashboard_path = format!("/room/{}?token={}", room.channel_id.replace(':', "%3A"), room.web_token);

    Ok(Some(PlayerHudView {
        title: room.title, scenario_id: room.scenario_id, genre, tagline, theme,
        location: room.location, world_time: room.world_time, turn_number: room.turn_number,
        turn_phase: room.turn_phase.as_str(), check_system: room.check_system.short_badge(),
        active: room.active, current_act: room.current_act, acts, world_flags: room.world_flags,
        suggested_actions: room.suggested_actions,
        ended_id: room.ended_id, ended_title: room.ended_title, dashboard_path,
        me, party, npcs, clues: room.clues, glossary, bulletins: room.bulletins, map, recent_turns,
    }))
}

fn classify_attitude(att: &str) -> &'static str {
    let a = att.trim();
    if a.contains("信任") || a.contains("友善") || a.contains("盟友") || a.contains("感激") || a.contains("好感") || a.contains("亲近") {
        "ally"
    } else if a.contains("敌对") || a.contains("仇恨") || a.contains("敌意") || a.contains("暴怒") || a.contains("追杀") {
        "hostile"
    } else if a.contains("警惕") || a.contains("戒备") || a.contains("怀疑") || a.contains("冷淡") || a.contains("不信任") || a.contains("试探") {
        "wary"
    } else {
        "neutral"
    }
}

fn mentions_npc(text: &str, npc_name: &str) -> bool {
    let name = npc_name.trim();
    if name.is_empty() {
        return false;
    }
    if text.contains(name) {
        return true;
    }
    name.split(['（', '）', '(', ')', '·', ' ', '「', '」', '/', '・'])
        .map(str::trim)
        .filter(|tok| tok.chars().count() >= 2)
        .any(|tok| text.contains(tok))
}
