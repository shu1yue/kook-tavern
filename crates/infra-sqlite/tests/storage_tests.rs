use chrono::Utc;
use rusqlite::Connection;
use tavern_core::RoomStore;
use tavern_domain::{
    ChatMode, NarrativeStyle, Player, RoomSnapshot, RoomState, TurnPhase, TurnRecord,
};
use tavern_infra_sqlite::SqliteRoomStore;

#[test]
fn upgrades_legacy_m0_schema_and_restores_m0_snapshot_without_crash() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(include_str!("../migrations/0001_init.sql"))
        .unwrap();
    conn.execute(
        r#"
        INSERT INTO rooms (
            channel_id, scenario_id, title, location, world_time, turn_number,
            chat_mode, clues_json, summary, summary_upto_turn, active, created_at, updated_at
        ) VALUES (
            'legacy_ch', 'ember_tavern', '余烬酒馆', '大厅', '21:00', 1,
            'strict', '["线索1"]', '', 0, 1, '2026-11-24T00:00:00Z', '2026-11-24T00:00:00Z'
        )
        "#,
        [],
    )
    .unwrap();
    conn.execute(
        r#"
        INSERT INTO pending_actions (
            channel_id, user_id, character_name, action_text, roll_notes_json, updated_at
        ) VALUES ('legacy_ch', 'u1', '鸭哥', '检查信使', '[]', '2026-11-24T00:01:00Z')
        "#,
        [],
    )
    .unwrap();
    // Exact legacy M0 snapshot JSON (missing state_version, turn_phase, narrative_style, web_token, target_turn, is_afk)
    let legacy_snap = r#"{"room":{"channel_id":"legacy_ch","scenario_id":"ember_tavern","title":"余烬酒馆","location":"大厅","world_time":"21:00","turn_number":0,"chat_mode":"strict","clues":[],"summary":"","summary_upto_turn":0,"active":true,"created_at":"2026-11-24T00:00:00Z","updated_at":"2026-11-24T00:00:00Z"},"players":[{"channel_id":"legacy_ch","user_id":"u1","character_name":"鸭哥","role_desc":"猎魔人","traits":[],"hp":12,"max_hp":12,"san":10,"max_san":10,"inventory":["火折子"],"status_note":"精神饱满"}],"pending_actions":[{"channel_id":"legacy_ch","user_id":"u1","character_name":"鸭哥","action_text":"检查信使","roll_notes":[],"updated_at":"2026-11-24T00:01:00Z"}]}"#;
    conn.execute(
        "INSERT INTO snapshots (channel_id, turn_number, snapshot_json, created_at) VALUES ('legacy_ch', 0, ?1, '2026-11-24T00:02:00Z')",
        [legacy_snap],
    )
    .unwrap();

    let store = SqliteRoomStore::from_connection(conn).unwrap();
    let room = store.get_room("legacy_ch").unwrap().unwrap();
    assert_eq!(room.turn_number, 1);
    assert_eq!(room.state_version, 1);
    assert_eq!(room.turn_phase, TurnPhase::Collecting);

    // Undo MUST succeed on legacy M0 snapshot and normalize pending action target_turn to 1!
    let restored = store.pop_and_restore_snapshot("legacy_ch").unwrap().unwrap();
    assert_eq!(restored.room.turn_number, 0);
    let actions = store.list_pending_actions("legacy_ch", 1).unwrap();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].character_name, "鸭哥");
    assert_eq!(actions[0].target_turn, 1);
}

#[test]
fn cas_atomic_actions_mode_isolation_and_profile_preservation() {
    let store = SqliteRoomStore::open_in_memory().unwrap();
    let now = Utc::now();
    let room = RoomState {
        channel_id: "ch1".into(),
        guild_id: "g1".into(),
        scenario_id: "ember_tavern".into(),
        title: "余烬酒馆".into(),
        location: "大厅".into(),
        world_time: "21:00".into(),
        turn_number: 0,
        state_version: 1,
        turn_phase: TurnPhase::Collecting,
        chat_mode: ChatMode::Strict,
        narrative_style: NarrativeStyle::Prose,
        gm_user_id: "u1".into(),
        web_token: "tok123".into(),
        clues: vec![],
        npc_states: vec![],
        suggested_actions: vec![],
        current_act: 1,
        world_flags: vec![],
        ended_id: None,
        ended_title: None,
        bulletins: vec![],
        clocks: vec![],
        check_system: Default::default(),
        last_rest_turn: 0,
        summary: String::new(),
        summary_upto_turn: 0,
        active: true,
        created_at: now,
        updated_at: now,
    };
    store.upsert_room(&room).unwrap();

    let player = Player {
        channel_id: "ch1".into(),
        user_id: "u1".into(),
        character_name: "小明".into(),
        role_desc: "剑客".into(),
        traits: vec!["勇猛".into()],
        hp: 12,
        max_hp: 12,
        san: 10,
        max_san: 10,
        inventory: vec![],
        status_note: String::new(),
        is_afk: false,
        stats: Default::default(),
        location: "大厅".into(),
        dnd: Default::default(),
    };
    store.upsert_player(&player).unwrap();

    // Atomic roll + action land in turn 1 and preserve rolls
    assert_eq!(
        store
            .append_roll_atomic("ch1", "u1", "小明", "🎲 2d6 = 9")
            .unwrap(),
        Some(1)
    );
    assert_eq!(
        store
            .submit_action_atomic("ch1", "u1", "小明", "推开门")
            .unwrap(),
        Some((1, false))
    );

    // Transition Collecting -> Resolving (version 1 -> 2)
    let lease = store.try_begin_resolving("ch1").unwrap().unwrap();
    assert_eq!(lease.expected_version, 2);
    assert_eq!(lease.actions[0].roll_notes.len(), 1);

    // Targeted `update_chat_mode` during Resolving MUST NOT corrupt `turn_phase` or `state_version`!
    assert!(store.update_chat_mode("ch1", ChatMode::Roleplay).unwrap());
    let mid_room = store.get_room("ch1").unwrap().unwrap();
    assert_eq!(mid_room.chat_mode, ChatMode::Roleplay);
    assert_eq!(mid_room.turn_phase, TurnPhase::Resolving);
    assert_eq!(mid_room.state_version, 2);

    // Concurrent `/join` profile edit during Resolving MUST survive `commit_turn_cas`!
    store
        .update_player_profile("ch1", "u1", "陆沉", "机械师", &["勇猛".into()], None)
        .unwrap()
        .unwrap();

    // GM mutation during Resolving MUST return RoomResolving and never race with DM commit!
    assert!(matches!(
        store
            .gm_mutate_player("ch1", "陆沉", -2, 0, None, None)
            .unwrap(),
        tavern_core::GmUpdateOutcome::RoomResolving
    ));
    assert!(matches!(
        store.gm_add_clue("ch1", "竞态线索").unwrap(),
        tavern_core::GmUpdateOutcome::RoomResolving
    ));

    // Atomic action submitted during Resolving lands safely in turn 2 buffer!
    assert_eq!(
        store
            .submit_action_atomic("ch1", "u1", "陆沉", "下一轮再拔剑")
            .unwrap(),
        Some((2, true))
    );

    let mut updated_room = lease.room.clone();
    updated_room.turn_number = 1;
    updated_room.state_version = 3;
    updated_room.turn_phase = TurnPhase::Collecting;

    let mut dm_updated_player = player.clone();
    dm_updated_player.hp = 9;
    dm_updated_player.inventory.push("铜钥匙".into());

    let snap = RoomSnapshot {
        room: room.clone(),
        players: vec![player],
        pending_actions: lease.actions.clone(),
    };
    let turn = TurnRecord {
        id: 0,
        channel_id: "ch1".into(),
        turn_number: 1,
        batched_input: "推开门".into(),
        scene_title: "第1幕".into(),
        narrative: "门开了".into(),
        state_summary: String::new(),
        location: "大厅".into(),
        world_time: "21:05".into(),
        created_at: now,
    };

    // Wrong version rejected
    assert!(!store
        .commit_turn_cas(
            999,
            &snap,
            &updated_room,
            std::slice::from_ref(&dm_updated_player),
            &turn,
            1,
            &[],
        )
        .unwrap());

    // Exact version commits cleanly, keeps chat_mode=Roleplay, keeps turn 2 buffer, AND keeps edited name!
    assert!(store
        .commit_turn_cas(
            2,
            &snap,
            &updated_room,
            std::slice::from_ref(&dm_updated_player),
            &turn,
            1,
            &[],
        )
        .unwrap());

    let after_room = store.get_room("ch1").unwrap().unwrap();
    assert_eq!(after_room.chat_mode, ChatMode::Roleplay);
    assert_eq!(after_room.turn_phase, TurnPhase::Collecting);

    let after_player = store.get_player("ch1", "u1").unwrap().unwrap();
    assert_eq!(after_player.character_name, "陆沉");
    assert_eq!(after_player.role_desc, "机械师");
    assert_eq!(after_player.hp, 9);
    assert_eq!(after_player.inventory, vec!["铜钥匙"]);

    let buffered = store.list_pending_actions("ch1", 2).unwrap();
    assert_eq!(buffered.len(), 1);
    assert_eq!(buffered[0].action_text, "下一轮再拔剑");
}
