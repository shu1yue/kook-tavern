use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use tavern_adapter_kook::cards::render_private_card_json;
use tavern_core::{AppConfig, GameEngine, ScenarioCatalog};
use tavern_domain::{EngineResponse, IncomingMessage};
use tavern_infra_llm::{MockLlmProvider, generate_scenario_with_llm};
use tavern_infra_sqlite::{
    FsScenarioCatalog, SqliteRoomStore, import_sillytavern_file, write_scenario_yaml,
};

fn build_cfg(scenarios_dir: PathBuf) -> AppConfig {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    AppConfig {
        kook_bot_token: None,
        llm_base_url: "https://mock.local/v1".into(),
        llm_api_key: None,
        llm_model: "mock".into(),
        llm_summary_model: "mock".into(),
        llm_temperature: 0.7,
        llm_max_tokens: 1024,
        db_path: PathBuf::from(":memory:"),
        scenarios_dir,
        prompts_dir: root.join("prompts"),
        exports_dir: PathBuf::from("/tmp/kook_tavern_m4_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec!["g_m4".into()],
        allowed_channel_ids: vec![],
        admin_user_ids: vec!["u_admin".into()],
        export_channel_id: None,
        web_bind_addr: None,
        web_public_base_url: None,
        daily_max_llm_calls: 200,
        daily_max_cny: 20.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
        jev: Default::default(),
    }
}

fn msg(channel: &str, uid: &str, name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g_m4".into()),
        channel_id: channel.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct: false,
    }
}

fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 63) as usize] as char);
        out.push(TABLE[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { TABLE[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn make_test_png_with_chara(json_payload: &str) -> Vec<u8> {
    let mut out = vec![137u8, 80, 78, 71, 13, 10, 26, 10];
    let mut chunk_data = b"chara\0".to_vec();
    chunk_data.extend_from_slice(encode_base64(json_payload.as_bytes()).as_bytes());
    out.extend_from_slice(&(chunk_data.len() as u32).to_be_bytes());
    out.extend_from_slice(b"tEXt");
    out.extend_from_slice(&chunk_data);
    out.extend_from_slice(&0u32.to_be_bytes()); // dummy CRC (not needed for metadata extraction)
    out.extend_from_slice(&0u32.to_be_bytes());
    out.extend_from_slice(b"IEND");
    out.extend_from_slice(&0u32.to_be_bytes());
    out
}

#[tokio::test]
async fn private_me_sheet_isolates_whispers_and_renders_valid_private_card() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let cfg = build_cfg(root.join("scenarios"));
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store, catalog, Arc::new(MockLlmProvider));

    let _ = engine
        .handle_message(msg("ch_me", "u1", "甲", "/start ember_tavern"))
        .await
        .unwrap();
    let _ = engine
        .handle_button_in_guild(Some("g_m4"), "ch_me", "u1", "甲", "tavern:pickrole:ch_me:0")
        .await
        .unwrap();
    let _ = engine
        .handle_button_in_guild(Some("g_m4"), "ch_me", "u2", "乙", "tavern:pickrole:ch_me:1")
        .await
        .unwrap();

    // u1 investigates -> receives a private whisper from MockLlmProvider; u2 does NOT
    let _ = engine
        .handle_message(msg("ch_me", "u1", "甲", "检查断钥匙"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(msg("ch_me", "u2", "乙", "站在一旁警戒"))
        .await
        .unwrap();

    // u1 clicks `🪪 我的角色卡` -> PrivateNotice with stats, gear, AND own whisper!
    let me_u1 = engine
        .handle_button_in_guild(Some("g_m4"), "ch_me", "u1", "甲", "tavern:me:ch_me")
        .await
        .unwrap();
    match me_u1 {
        EngineResponse::PrivateNotice {
            user_id,
            title,
            body,
            actions,
        } => {
            assert_eq!(user_id, "u1");
            assert!(title.contains("齿轮机械师"));
            assert!(body.contains("🔍+2"));
            assert!(body.contains("精密黄铜扳手"));
            assert!(body.contains("霰弹枪")); // u1's own secret whisper
            let card_json = render_private_card_json(&title, &body, &actions);
            let v: serde_json::Value = serde_json::from_str(&card_json).unwrap();
            assert!(v.is_array());
            assert!(v[0]["modules"].as_array().unwrap().len() <= 50);
        }
        other => panic!("expected PrivateNotice for u1, got {other:?}"),
    }

    // u2 checks `/me` -> PrivateNotice MUST NOT contain u1's secret whisper!
    let me_u2 = engine
        .handle_message(msg("ch_me", "u2", "乙", "/me"))
        .await
        .unwrap();
    match me_u2 {
        EngineResponse::PrivateNotice { user_id, body, .. } => {
            assert_eq!(user_id, "u2");
            assert!(body.contains("港口赏金猎人") || body.contains("💪+2"));
            assert!(!body.contains("霰弹枪"), "u2 MUST NOT see u1's private whisper!");
        }
        other => panic!("expected PrivateNotice for u2, got {other:?}"),
    }
}

#[tokio::test]
async fn m4_gen_scenario_st_import_and_atomic_reload_with_bad_yaml_isolation() {
    let tmp_dir = std::env::temp_dir().join(format!("tavern_m4_test_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();

    let cfg = build_cfg(tmp_dir.clone());

    // 1. AI gen-scenario (--mock-llm) produces valid v2 scenario and blocks overwrite by default
    let generated =
        generate_scenario_with_llm(&cfg, "深夜高校图书馆规则怪谈", Some("campus_lib"), Some("metro"), true)
            .await
            .unwrap();
    let saved_gen = write_scenario_yaml(&tmp_dir, &generated, false).unwrap();
    assert!(saved_gen.exists());
    assert!(write_scenario_yaml(&tmp_dir, &generated, false).is_err());

    // 2. SillyTavern PNG card import (zero external image/base64 crates!)
    let st_v2_json = serde_json::json!({
        "spec": "chara_card_v2",
        "data": {
            "name": "钟塔守夜人",
            "description": "一位守护古老钟塔的沉默机关师。",
            "scenario": "午夜暴风雪中，钟塔齿轮突然逆转，你们被困在三楼机械室。",
            "first_mes": "齿轮发出刺耳的金属呻吟，钟塔指针竟然开始倒着走，门外传来缓慢而沉重的脚步声……",
            "character_book": {
                "entries": [
                    {
                        "comment": "逆转齿轮秘密",
                        "keys": ["齿轮", "倒转"],
                        "constant": false,
                        "insertion_order": 95,
                        "content": "逆转齿轮其实是为了封印塔底的古老回响。"
                    },
                    {
                        "comment": "被禁用的草稿",
                        "enabled": false,
                        "content": "不应被导入"
                    }
                ]
            }
        }
    })
    .to_string();
    let png_path = tmp_dir.join("clocktower_card.png");
    fs::write(&png_path, make_test_png_with_chara(&st_v2_json)).unwrap();
    let imported_png = import_sillytavern_file(&png_path, Some("st_clocktower"), Some("fog")).unwrap();
    assert_eq!(imported_png.id, "st_clocktower");
    assert_eq!(imported_png.theme, "fog");
    assert!(imported_png.worldbook.iter().any(|w| w.content.contains("塔底的古老回响")));
    assert!(!imported_png.worldbook.iter().any(|w| w.content.contains("不应被导入")));
    let _ = write_scenario_yaml(&tmp_dir, &imported_png, false).unwrap();

    // 3. Hot reload RBAC + live reload + bad YAML rollback isolation!
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&tmp_dir).unwrap());
    assert_eq!(catalog.list().len(), 2);
    let engine = GameEngine::new(cfg, store, catalog.clone(), Arc::new(MockLlmProvider));

    // Non-admin rejected
    let non_admin = engine
        .handle_message(msg("ch_rel", "u_normal", "普通人", "/reload"))
        .await
        .unwrap();
    match non_admin {
        EngineResponse::Notice { title, .. } => assert!(title.contains("权限不足")),
        other => panic!("expected RBAC reject, got {other:?}"),
    }

    // Drop a brand-new standalone ST World Info JSON -> import -> admin `/reload`!
    let lore_json_path = tmp_dir.join("haunted_inn.json");
    fs::write(
        &lore_json_path,
        r#"{"entries":{"0":{"comment":"客栈规矩","key":["客栈","规矩"],"constant":true,"order":100,"content":"子时之后绝不可替敲门人开窗。"}}}"#,
    )
    .unwrap();
    let imported_lore = import_sillytavern_file(&lore_json_path, Some("st_inn"), None).unwrap();
    let _ = write_scenario_yaml(&tmp_dir, &imported_lore, false).unwrap();

    let ok_reload = engine
        .handle_message(msg("ch_rel", "u_admin", "管理员", "/reload"))
        .await
        .unwrap();
    match ok_reload {
        EngineResponse::Notice { title, body, .. } => {
            assert!(title.contains("3 个剧本"));
            assert!(body.contains("st_inn"));
        }
        other => panic!("expected reload success, got {other:?}"),
    }
    assert!(catalog.get("st_inn").is_some());

    // Introduce a corrupted YAML file -> `/reload` MUST fail safely while keeping all 3 live scenarios intact!
    let broken = tmp_dir.join("broken_bad.yaml");
    fs::write(&broken, "id: ../bad_path\nname: \"\"\n").unwrap();
    let fail_reload = engine
        .handle_message(msg("ch_rel", "u_admin", "管理员", "/reload"))
        .await
        .unwrap();
    match fail_reload {
        EngineResponse::Notice { title, .. } => assert!(title.contains("安全中止")),
        other => panic!("expected safe reload abort, got {other:?}"),
    }
    assert_eq!(catalog.list().len(), 3, "existing live catalog MUST survive broken YAML");

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn gate5_red_team_regressions_admin_fail_closed_png_safety_and_reload_empty_guard() {
    // 1. `is_admin` MUST be strictly fail-closed under ALL `.env` combinations!
    let mut cfg_empty = build_cfg(PathBuf::from("/tmp"));
    cfg_empty.allowed_guild_ids.clear();
    cfg_empty.allowed_channel_ids.clear();
    cfg_empty.admin_user_ids.clear();
    assert!(!cfg_empty.is_admin("stranger"));
    assert!(!cfg_empty.is_admin(""));

    let mut cfg_wl_no_admin = cfg_empty.clone();
    cfg_wl_no_admin.allowed_guild_ids = vec!["g1".into()];
    assert!(!cfg_wl_no_admin.is_admin("stranger"));

    let mut cfg_with_admin = cfg_empty.clone();
    cfg_with_admin.admin_user_ids = vec!["boss_1".into()];
    assert!(cfg_with_admin.is_admin("boss_1"));
    assert!(!cfg_with_admin.is_admin("stranger"));

    // 2. Whitespace-padded ID & short opening_scene (5 Chinese chars = 15 UTF-8 bytes!) rejected!
    let mut sc = tavern_domain::Scenario {
        id: " pad_id ".into(),
        name: "测试".into(),
        tagline: "简介".into(),
        genre: "悬疑".into(),
        theme: "ember".into(),
        dm_style: "克制".into(),
        initial_location: "酒馆".into(),
        initial_time: "夜里".into(),
        opening_scene: "只有五个字".into(), // 15 bytes, only 5 chars!
        opening_actions: vec![],
        player_defaults: tavern_domain::StarterCharacterDefaults {
            max_hp: 10,
            max_san: 10,
            starter_items: vec![],
        },
        preset_roles: vec![],
        acts: vec![],
        endings: vec![],
        worldbook: vec![],
        map_nodes: vec![],
        map_edges: vec![],
        rules: Default::default(),
        clocks: vec![],
    };
    let errs = tavern_domain::validate_scenario(&sc).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("id")));
    assert!(errs.iter().any(|e| e.contains("opening_scene")));

    // 3. `.yml` conflict blocks writing `<id>.yaml` when overwrite=false
    let tmp_dir = std::env::temp_dir().join(format!("tavern_g5_sec_{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp_dir);
    fs::create_dir_all(&tmp_dir).unwrap();
    sc.id = "dup_yml_test".into();
    sc.opening_scene = "这是一段超过十五个字符的合法开场白描写，夜色深沉风雪交加。".into();
    fs::write(tmp_dir.join("dup_yml_test.yml"), "placeholder").unwrap();
    assert!(write_scenario_yaml(&tmp_dir, &sc, false).is_err());

    // 4. PNG parser rejects malformed compression flags & oversized chunk length without panicking!
    let mut bad_png = vec![137u8, 80, 78, 71, 13, 10, 26, 10];
    let bad_itxt = b"chara\0\x01\x09\0\0payload"; // invalid comp_method = 9
    bad_png.extend_from_slice(&(bad_itxt.len() as u32).to_be_bytes());
    bad_png.extend_from_slice(b"iTXt");
    bad_png.extend_from_slice(bad_itxt);
    bad_png.extend_from_slice(&0u32.to_be_bytes());
    assert!(tavern_infra_sqlite::st_png::extract_png_chara_json(&bad_png).is_err());

    // 5. Empty directory `/reload` MUST NOT wipe an already loaded live catalog!
    let _ = fs::remove_file(tmp_dir.join("dup_yml_test.yml"));
    let saved = write_scenario_yaml(&tmp_dir, &sc, false).unwrap();
    let cat = FsScenarioCatalog::load_from_dir(&tmp_dir).unwrap();
    assert_eq!(cat.list().len(), 1);
    fs::remove_file(&saved).unwrap();
    assert!(cat.reload().is_err());
    assert_eq!(cat.list().len(), 1, "empty dir reload MUST preserve live scenarios!");

    let _ = fs::remove_dir_all(&tmp_dir);
}
