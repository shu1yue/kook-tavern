use std::collections::HashSet;

use serde::Serialize;
use tavern_domain::{
    NpcState, PendingAction, Player, RoomState, Scenario, ScenarioMapEdge, TurnRecord,
    resolve_map_node_id,
};

#[derive(Debug, Serialize)]
pub struct ScenarioMapView {
    pub active_node_id: Option<String>,
    pub nodes: Vec<MapNodePublicView>,
    pub edges: Vec<ScenarioMapEdge>,
    pub player_pins: Vec<PlayerPinView>,
    pub npc_pins: Vec<NpcPinView>,
}

#[derive(Debug, Serialize)]
pub struct MapNodePublicView {
    pub id: String,
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub kind: String,
    pub visited: bool,
}

#[derive(Debug, Serialize)]
pub struct PlayerPinView {
    pub character_name: String,
    pub role_desc: String,
    pub node_id: String,
    pub hp: i32,
    pub max_hp: i32,
    pub is_afk: bool,
    pub action_ready: bool,
}

#[derive(Debug, Serialize)]
pub struct NpcPinView {
    pub name: String,
    pub attitude: String,
    pub status: String,
    pub node_id: String,
}

pub fn build_scenario_map_view(
    scenario: Option<&Scenario>,
    room: &RoomState,
    players: &[Player],
    pending: &[PendingAction],
    turns: &[TurnRecord],
    npcs: &[NpcState],
) -> ScenarioMapView {
    let Some(sc) = scenario.filter(|s| !s.map_nodes.is_empty()) else {
        return ScenarioMapView {
            active_node_id: None,
            nodes: Vec::new(),
            edges: Vec::new(),
            player_pins: Vec::new(),
            npc_pins: Vec::new(),
        };
    };

    let fallback_id = resolve_map_node_id(&sc.map_nodes, &sc.initial_location)
        .unwrap_or_else(|| sc.map_nodes[0].id.clone());
    let active_node_id = resolve_map_node_id(&sc.map_nodes, &room.location)
        .unwrap_or_else(|| fallback_id.clone());

    let mut visited = HashSet::new();
    visited.insert(fallback_id.clone());
    visited.insert(active_node_id.clone());
    for t in turns {
        if let Some(id) = resolve_map_node_id(&sc.map_nodes, &t.location) {
            visited.insert(id);
        }
    }

    let mut player_pins = Vec::with_capacity(players.len());
    for p in players {
        let p_loc = if p.location.trim().is_empty() {
            room.location.as_str()
        } else {
            p.location.as_str()
        };
        let node_id =
            resolve_map_node_id(&sc.map_nodes, p_loc).unwrap_or_else(|| active_node_id.clone());
        visited.insert(node_id.clone());
        let action_ready = pending
            .iter()
            .any(|a| a.user_id == p.user_id && !a.action_text.trim().is_empty());
        player_pins.push(PlayerPinView {
            character_name: p.character_name.clone(),
            role_desc: p.role_desc.clone(),
            node_id,
            hp: p.hp,
            max_hp: p.max_hp,
            is_afk: p.is_afk,
            action_ready,
        });
    }

    let mut npc_pins = Vec::with_capacity(npcs.len());
    for n in npcs {
        let node_id = resolve_map_node_id(&sc.map_nodes, &n.location)
            .or_else(|| resolve_map_node_id(&sc.map_nodes, &n.status))
            .unwrap_or_else(|| active_node_id.clone());
        npc_pins.push(NpcPinView {
            name: n.name.clone(),
            attitude: n.attitude.clone(),
            status: n.status.clone(),
            node_id,
        });
    }

    let ended = room.ended_id.is_some();
    let secret_unvisited: HashSet<&str> = sc
        .map_nodes
        .iter()
        .filter(|n| !ended && n.kind.eq_ignore_ascii_case("secret") && !visited.contains(&n.id))
        .map(|n| n.id.as_str())
        .collect();

    let nodes = sc
        .map_nodes
        .iter()
        .map(|n| {
            let is_visited = visited.contains(&n.id);
            let masked_name = if secret_unvisited.contains(n.id.as_str()) {
                "？？？".to_string()
            } else {
                n.name.clone()
            };
            MapNodePublicView {
                id: n.id.clone(),
                name: masked_name,
                x: n.x,
                y: n.y,
                kind: n.kind.clone(),
                visited: is_visited,
            }
        })
        .collect();

    let edges = sc
        .map_edges
        .iter()
        .map(|e| {
            let mut edge = e.clone();
            let touches_secret = secret_unvisited.contains(edge.from.as_str())
                || secret_unvisited.contains(edge.to.as_str());
            let neither_visited = !visited.contains(&edge.from) && !visited.contains(&edge.to);
            if !ended && edge.locked && (touches_secret || neither_visited) {
                edge.label = "未解封通道".to_string();
            }
            edge
        })
        .collect();

    ScenarioMapView {
        active_node_id: Some(active_node_id),
        nodes,
        edges,
        player_pins,
        npc_pins,
    }
}
