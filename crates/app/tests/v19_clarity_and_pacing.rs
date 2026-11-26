use std::path::Path;
use std::sync::Arc;

use tavern_adapter_kook::cards::render_kook_card_json;
use tavern_core::{AppConfig, GameEngine, RoomStore, ScenarioCatalog};
use tavern_domain::{
    CheckSystem, EngineResponse, ForgeScenarioRequest, IncomingMessage, PendingAction, Scenario,
    ScenarioAct, ScenarioEnding, ScenarioMapEdge, ScenarioMapNode, StarterCharacterDefaults,
    WorldbookEntry, WorkshopDraft, should_trigger_bulletin,
};
use tavern_infra_llm::{
    MockLlmProvider, generator::finalize_forged_scenario, prompts::build_dm_messages_full,
};
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn msg(channel_id: &str, user_id: &str, user_name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g_v19".into()),
        channel_id: channel_id.into(),
        user_id: user_id.into(),
        user_name: user_name.into(),
        content: content.into(),
        is_direct: false,
    }
}

fn test_engine() -> (GameEngine, Arc<SqliteRoomStore>, Arc<FsScenarioCatalog>) {
    let mut cfg = AppConfig::from_env().unwrap();
    cfg.scenarios_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios");
    cfg.prompts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let llm = Arc::new(MockLlmProvider);
    (GameEngine::new(cfg, store.clone(), catalog.clone(), llm), store, catalog)
}

#[test]
fn final_act_auto_injects_climax_boss_and_convergence_directives_and_silences_bulletins() {
    let (engine, store, catalog) = test_engine();
    let scenario = catalog.get("ember_tavern").expect("ember_tavern");
    let prompts_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../prompts");

    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let _ = engine.handle_message(msg("ch_c", "u1", "阿白", "/start ember_tavern")).await.unwrap();
        let _ = engine.handle_message(msg("ch_c", "u1", "阿白", "/join 阿白 星象学者")).await.unwrap();
    });

    let mut room = store.get_room("ch_c").unwrap().unwrap();
    room.current_act = 3;
    room.turn_number = 20;
    room.location = "钟楼三层星仪机房".into();
    let players = store.list_players("ch_c").unwrap();
    let actions = vec![PendingAction {
        channel_id: "ch_c".into(),
        target_turn: 21,
        user_id: "u1".into(),
        character_name: "阿白".into(),
        action_text: "检查地板上的旧划痕".into(), // Deliberately does NOT mention "白鸦"
        roll_notes: vec![],
        updated_at: chrono::Utc::now(),
    }];

    // Final act must silence street bulletins!
    assert!(!should_trigger_bulletin(&scenario, &room, 21));

    let msgs = build_dm_messages_full(
        &prompts_dir, &scenario, &room, &players, &[], &actions, &[], &[], &[],
    );
    let sys = &msgs[0].content;
    let user_prompt = &msgs.last().unwrap().content;
    assert!(sys.contains("说现代大白话"), "system prompt must enforce plain Chinese");
    assert!(user_prompt.contains("单人独行规则"), "solo player rule must be injected");
    assert!(user_prompt.contains("终幕强收束指令"), "late final act must inject hard convergence");
    assert!(
        user_prompt.contains("白鸦") && user_prompt.contains("卡修斯"),
        "final act must auto-inject climax boss lore even if action didn't mention 白鸦"
    );
}

#[test]
fn workshop_forged_scenario_auto_wires_enter_nodes_boss_keywords_and_climax_beat() {
    let mut draft = WorkshopDraft::new("u1");
    draft.title = "雾港钟声".into();
    draft.premise = "查明雾港钟声失控的幕后主使".into();
    draft.act_count = 3;
    draft.check_system = CheckSystem::TwoD6;
    draft.theme = "ember".into();
    let req = ForgeScenarioRequest { draft, base_pack: None };

    // Simulate an LLM-forged scenario that forgot `enter_nodes`, `player_visible`, Boss location keyword, and `must_hit_beats`
    let raw_sc = Scenario {
        id: "fog_bell".into(),
        name: "雾港钟声".into(),
        tagline: "雾港钟楼午夜异响".into(),
        genre: "悬疑".into(),
        theme: "ember".into(),
        dm_style: "大白话悬疑".into(),
        initial_location: "港口酒馆".into(),
        initial_time: "21:00".into(),
        opening_scene: "雾港钟楼午夜突然响起异常的钟声，港口居民议论纷纷，你们聚集在港口酒馆准备调查真相。".into(),
        opening_actions: vec!["打听消息".into()],
        player_defaults: StarterCharacterDefaults {
            max_hp: 12,
            max_san: 10,
            starter_items: vec!["火折子".into()],
        },
        preset_roles: vec![tavern_domain::PresetRole {
            name: "侦探".into(), traits: vec!["眼尖".into()], bonus_items: vec![],
            stats: Default::default(), alignment: String::new(), proficient_skills: vec![], abilities: vec![],
        }],
        acts: vec![
            ScenarioAct { id: "a1".into(), title: "开端".into(), goal: "查清酒馆线索".into(), advance_flags: vec![], enter_nodes: vec![] },
            ScenarioAct { id: "a2".into(), title: "深入".into(), goal: "穿过码头".into(), advance_flags: vec![], enter_nodes: vec![] },
            ScenarioAct { id: "a3".into(), title: "终局".into(), goal: "面对钟楼幕后主使".into(), advance_flags: vec![], enter_nodes: vec![] },
        ],
        endings: vec![
            ScenarioEnding { id: "e_true".into(), title: "天明".into(), kind: "true".into(), summary: "击败幕后主使".into() },
        ],
        worldbook: vec![
            WorldbookEntry {
                id: "wb_base".into(), title: "雾港背景".into(), keywords: vec!["雾港".into()],
                always_active: true, player_visible: false, priority: 100, content: "公开港口背景".into(),
            },
            WorldbookEntry {
                id: "wb_boss".into(), title: "幕后主使真相".into(), keywords: vec!["主使".into()],
                always_active: false, player_visible: false, priority: 95, content: "幕后主使藏在钟楼顶层。".into(),
            },
        ],
        map_nodes: vec![
            ScenarioMapNode { id: "pub".into(), name: "港口酒馆".into(), x: 15.0, y: 50.0, aliases: vec![], kind: "hub".into() },
            ScenarioMapNode { id: "pier".into(), name: "迷雾码头".into(), x: 50.0, y: 50.0, aliases: vec![], kind: "mystery".into() },
            ScenarioMapNode { id: "bell_top".into(), name: "钟楼顶层".into(), x: 85.0, y: 50.0, aliases: vec![], kind: "secret".into() },
        ],
        map_edges: vec![
            ScenarioMapEdge { from: "pub".into(), to: "pier".into(), label: "石街".into(), locked: false },
            ScenarioMapEdge { from: "pier".into(), to: "bell_top".into(), label: "铁梯".into(), locked: false },
        ],
        rules: tavern_domain::ScenarioRules { bulletin_interval: 3, ..Default::default() },
    };

    let sc = finalize_forged_scenario(raw_sc, &req, None).expect("finalize forged scenario");
    assert_eq!(sc.acts[1].enter_nodes, vec!["pier".to_string()]);
    assert_eq!(sc.acts[2].enter_nodes, vec!["bell_top".to_string()]);
    assert!(sc.rules.bulletin_interval >= 5, "bulletin interval clamped >= 5");
    assert!(sc.worldbook.iter().any(|w| w.player_visible), "at least one public lore entry");
    let boss = sc.worldbook.iter().find(|w| w.id == "wb_boss").unwrap();
    assert!(boss.keywords.contains(&"钟楼顶层".to_string()), "boss entry auto-tagged with final map node");
    assert!(sc.rules.must_hit_beats.iter().any(|b| b.act == 3), "final act must_hit_beat auto-added");
}

#[tokio::test]
async fn story_briefing_command_and_live_hud_goal_render_clearly() {
    let (engine, _, _) = test_engine();
    let _ = engine.handle_message(msg("ch_s", "u1", "阿明", "/start ember_tavern")).await.unwrap();
    let _ = engine.handle_message(msg("ch_s", "u1", "阿明", "/join 阿明 机械师")).await.unwrap();

    let hud = engine.get_live_story_hud("ch_s").unwrap().unwrap();
    assert!(hud.current_goal.contains("老雷恩"), "live HUD must carry current act goal: {}", hud.current_goal);

    let story_resp = engine.handle_message(msg("ch_s", "u1", "阿明", "/story")).await.unwrap();
    match &story_resp {
        EngineResponse::Notice { title, body, .. } => {
            assert!(title.contains("大白话案情简报"));
            assert!(body.contains("这是个什么故事"));
            assert!(body.contains("目前登场了哪些人"));
            assert!(body.contains("现在要去干什么"));
            assert!(body.contains("当前主线"));
        }
        other => panic!("expected Notice for /story, got {other:?}"),
    }

    let status_resp = engine.handle_message(msg("ch_s", "u1", "阿明", "/status")).await.unwrap();
    let card = render_kook_card_json("ch_s", &status_resp);
    assert!(card.contains("tavern:story:ch_s"), "status panel should expose one-click story briefing button");
}
