use anyhow::Result;
use serde::Serialize;
use tavern_domain::{
    CharacterStats, ChronicleEntry, NpcState, RoomState, TurnPhase, WorldBulletin,
    build_chronicle, ending_scope_key,
};

use crate::WebState;
use crate::map_view::{ScenarioMapView, build_scenario_map_view};

/// Strictly sanitized public read-only room view (NEVER exposes `web_token`, `gm_user_id`,
/// `guild_id`, `user_id`, or private `whispers`!).
#[derive(Debug, Serialize)]
pub struct RoomPublicView {
    pub title: String,
    pub scenario_id: String,
    pub theme: &'static str,
    pub location: String,
    pub world_time: String,
    pub turn_number: u32,
    pub turn_phase: &'static str,
    pub chat_mode: &'static str,
    pub narrative_style: &'static str,
    pub check_system: &'static str,
    pub active: bool,
    pub current_act: u32,
    pub acts: Vec<ActPublicView>,
    pub world_flags: Vec<String>,
    pub ended_id: Option<String>,
    pub ended_title: Option<String>,
    pub ending_gallery: Vec<EndingGalleryItem>,
    pub world_echoes: Vec<EchoPublicView>,
    pub bulletins: Vec<WorldBulletin>,
    pub chronicle: Vec<ChronicleEntry>,
    pub map: ScenarioMapView,
    pub summary: String,
    pub summary_upto_turn: u32,
    pub opening_scene: String,
    pub clues: Vec<String>,
    pub npc_states: Vec<NpcState>,
    pub players: Vec<PlayerPublicView>,
    pub turns: Vec<TurnPublicView>,
}

#[derive(Debug, Serialize)]
pub struct EchoPublicView {
    pub ending_title: String,
    pub ending_kind: String,
    pub echo_text: String,
    pub turn_count: u32,
}

#[derive(Debug, Serialize)]
pub struct ActPublicView {
    pub index: u32,
    pub title: String,
    pub goal: String,
}

#[derive(Debug, Serialize)]
pub struct EndingGalleryItem {
    pub kind: String,
    pub title: String,
    pub unlocked: bool,
    pub turn_count: Option<u32>,
}

#[derive(Debug, Serialize)]
pub struct AbilityPublicView {
    pub name: String,
    pub uses_left: u32,
    pub max_uses: u32,
    pub recharge: &'static str,
}

#[derive(Debug, Serialize)]
pub struct PlayerPublicView {
    pub character_name: String,
    pub role_desc: String,
    pub traits: Vec<String>,
    pub stats: CharacterStats,
    pub level: u32,
    pub inspiration: bool,
    pub alignment: String,
    pub proficient_skills: Vec<String>,
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
    pub roll_notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TurnPublicView {
    pub turn_number: u32,
    pub scene_title: String,
    pub location: String,
    pub world_time: String,
    pub batched_input: String,
    pub narrative: String,
    pub state_summary: String,
}

fn sanitize_theme(raw: &str) -> &'static str {
    match raw.trim().to_ascii_lowercase().as_str() {
        "fog" => "fog",
        "metro" => "metro",
        _ => "ember",
    }
}

pub fn build_room_public_view(state: &WebState, channel_id: &str, room: RoomState) -> Result<RoomPublicView> {
    let players = state.store.list_players(channel_id)?;
    let next_turn = if room.turn_phase == TurnPhase::Resolving {
        room.turn_number + 2
    } else {
        room.turn_number + 1
    };
    let pending = state
        .store
        .list_pending_actions(channel_id, next_turn)
        .unwrap_or_default();
    let turns = state
        .store
        .list_recent_turns(channel_id, 30)
        .unwrap_or_default();
    let scenario = state.scenarios.get(&room.scenario_id);

    let theme = scenario
        .as_ref()
        .map(|s| sanitize_theme(&s.theme))
        .unwrap_or("ember");
    let opening_scene = scenario
        .as_ref()
        .map(|s| s.opening_scene.trim().to_string())
        .unwrap_or_default();
    let acts: Vec<ActPublicView> = scenario
        .as_ref()
        .map(|s| {
            s.acts
                .iter()
                .enumerate()
                .map(|(i, a)| {
                    let index = (i + 1) as u32;
                    let locked = index > room.current_act && room.ended_id.is_none();
                    ActPublicView {
                        index,
                        title: if locked {
                            "未解锁幕次".to_string()
                        } else {
                            a.title.clone()
                        },
                        goal: if locked {
                            "推进当前主线后解锁".to_string()
                        } else {
                            a.goal.clone()
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let scope_key = ending_scope_key(Some(&room.guild_id), &room.channel_id);
    let unlocks = state
        .store
        .list_ending_unlocks(&scope_key)
        .unwrap_or_default();
    let ending_gallery: Vec<EndingGalleryItem> = scenario
        .as_ref()
        .map(|s| {
            s.endings
                .iter()
                .map(|e| {
                    if let Some(u) = unlocks
                        .iter()
                        .find(|u| u.scenario_id == s.id && u.ending_id == e.id)
                    {
                        EndingGalleryItem {
                            kind: e.kind.clone(),
                            title: e.title.clone(),
                            unlocked: true,
                            turn_count: Some(u.turn_count),
                        }
                    } else {
                        EndingGalleryItem {
                            kind: e.kind.clone(),
                            title: "？？？".to_string(),
                            unlocked: false,
                            turn_count: None,
                        }
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let map = build_scenario_map_view(
        scenario.as_ref(),
        &room,
        &players,
        &pending,
        &turns,
        &room.npc_states,
    );
    let chronicle = build_chronicle(&turns, &room.bulletins, room.ended_title.as_deref());
    let before_ts = room.created_at.to_rfc3339();
    let world_echoes: Vec<EchoPublicView> = state
        .store
        .list_world_echoes(&scope_key, &room.scenario_id, Some(&before_ts), 3)
        .unwrap_or_default()
        .into_iter()
        .map(|e| EchoPublicView {
            ending_title: e.ending_title,
            ending_kind: e.ending_kind,
            echo_text: e.echo_text,
            turn_count: e.turn_count,
        })
        .collect();

    let player_views = players
        .into_iter()
        .map(|p| {
            let p_act = pending.iter().find(|a| a.user_id == p.user_id);
            let action_ready = p_act.is_some_and(|a| !a.action_text.trim().is_empty());
            let roll_notes = p_act.map(|a| a.roll_notes.clone()).unwrap_or_default();
            let loc = if p.location.trim().is_empty() {
                room.location.clone()
            } else {
                p.location
            };
            let abilities = p
                .dnd
                .abilities
                .into_iter()
                .map(|a| AbilityPublicView {
                    name: a.name,
                    uses_left: a.uses_left,
                    max_uses: a.max_uses,
                    recharge: a.recharge.short_label(),
                })
                .collect();
            PlayerPublicView {
                character_name: p.character_name,
                role_desc: p.role_desc,
                traits: p.traits,
                stats: p.stats,
                level: p.dnd.level,
                inspiration: p.dnd.inspiration,
                alignment: p.dnd.alignment,
                proficient_skills: p.dnd.proficient_skills,
                abilities,
                hp: p.hp,
                max_hp: p.max_hp,
                san: p.san,
                max_san: p.max_san,
                inventory: p.inventory,
                status_note: p.status_note,
                location: loc,
                is_afk: p.is_afk,
                action_ready,
                roll_notes,
            }
        })
        .collect();

    let turn_views = turns
        .into_iter()
        .map(|t| TurnPublicView {
            turn_number: t.turn_number,
            scene_title: t.scene_title,
            location: t.location,
            world_time: t.world_time,
            batched_input: t.batched_input,
            narrative: t.narrative,
            state_summary: t.state_summary,
        })
        .collect();

    Ok(RoomPublicView {
        title: room.title,
        scenario_id: room.scenario_id,
        theme,
        location: room.location,
        world_time: room.world_time,
        turn_number: room.turn_number,
        turn_phase: room.turn_phase.as_str(),
        chat_mode: room.chat_mode.as_str(),
        narrative_style: room.narrative_style.as_str(),
        check_system: room.check_system.short_badge(),
        active: room.active,
        current_act: room.current_act,
        acts,
        world_flags: room.world_flags,
        ended_id: room.ended_id,
        ended_title: room.ended_title,
        ending_gallery,
        world_echoes,
        bulletins: room.bulletins,
        chronicle,
        map,
        summary: room.summary,
        summary_upto_turn: room.summary_upto_turn,
        opening_scene,
        clues: room.clues,
        npc_states: room.npc_states,
        players: player_views,
        turns: turn_views,
    })
}
