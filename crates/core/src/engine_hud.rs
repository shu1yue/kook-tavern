use anyhow::Result;
use tavern_domain::EngineResponse;

use crate::engine::GameEngine;

impl GameEngine {
    /// Reconstructs a saved `StoryTurn` from SQLite so a restarted bot can seamlessly archive
    /// or rehydrate the active channel card without losing button cleanup across restarts.
    pub fn rebuild_story_turn_for_archive(
        &self,
        channel_id: &str,
        turn_number: u32,
    ) -> Result<Option<EngineResponse>> {
        let Some(room) = self.store.get_room(channel_id)? else {
            return Ok(None);
        };
        let sc = self.scenarios.get(&room.scenario_id);
        let act_badge = sc.as_ref().and_then(|s| {
            s.acts
                .get(room.current_act.saturating_sub(1) as usize)
                .map(|a| format!("第 {}/{} 幕 · {}", room.current_act, s.acts.len(), a.title))
        });
        let suggested = if room.turn_number == turn_number {
            room.suggested_actions.clone()
        } else {
            Vec::new()
        };
        if turn_number == 0 {
            let Some(s) = sc else { return Ok(None) };
            return Ok(Some(EngineResponse::StoryTurn {
                channel_id: room.channel_id,
                turn_number: 0,
                scene_title: format!("{} · 序幕", s.name),
                location: s.initial_location,
                world_time: s.initial_time,
                act_badge,
                narrative: s.opening_scene,
                state_changes: Vec::new(),
                whisper_hints: Vec::new(),
                suggested_actions: suggested,
                suggested_check: None,
                world_bulletin: None,
                ending_reached: None,
                direct_messages: Vec::new(),
            }));
        }
        let turns = self
            .store
            .list_turns_range(channel_id, turn_number - 1, turn_number)?;
        let Some(t) = turns.into_iter().find(|r| r.turn_number == turn_number) else {
            return Ok(None);
        };
        let state_changes = if t.state_summary.trim().is_empty() {
            Vec::new()
        } else {
            t.state_summary
                .split('；')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
                .collect()
        };
        Ok(Some(EngineResponse::StoryTurn {
            channel_id: room.channel_id,
            turn_number: t.turn_number,
            scene_title: t.scene_title,
            location: t.location,
            world_time: t.world_time,
            act_badge,
            narrative: t.narrative,
            state_changes,
            whisper_hints: Vec::new(),
            suggested_actions: suggested,
            suggested_check: None,
            world_bulletin: room
                .bulletins
                .into_iter()
                .rev()
                .find(|b| b.turn_number == turn_number),
            ending_reached: None,
            direct_messages: Vec::new(),
        }))
    }
}
