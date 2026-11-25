use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tavern_adapter_kook::cards::{render_kook_card_json, render_private_card_json};
use tavern_core::{
    AppConfig, BoxFuture, GameEngine, JevConfig, LlmProvider, RoomStore, ScenarioCatalog,
    StRemoteImporter, WorldbookCatalog,
};
use tavern_domain::{
    CharacterStats, CheckSystem, DmTurnOutput, EngineResponse, ForgeScenarioOutput,
    ForgeScenarioRequest, IncomingMessage, LlmUsage, PendingAction, Player, RoomState, Scenario,
    SummaryOutput, TurnRecord, WorkshopDraft, WorldEcho, validate_point_buy,
    validate_worldbook_pack,
};
use tavern_infra_llm::{
    MockLlmProvider, generator::finalize_forged_scenario, validate_allowlisted_import_url,
};
use tavern_infra_sqlite::{FsScenarioCatalog, FsWorldbookCatalog, SqliteRoomStore};

fn unique_temp_dir(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("kook_tavern_v15_{tag}_{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_builtin_scenarios_to(dst: &Path) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    for entry in fs::read_dir(root.join("scenarios")).unwrap() {
        let p = entry.unwrap().path();
        if p.is_file() && p.extension().and_then(|e| e.to_str()) == Some("yaml") {
            fs::copy(&p, dst.join(p.file_name().unwrap())).unwrap();
        }
    }
}

fn v15_config(scenarios_dir: PathBuf) -> AppConfig {
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
        exports_dir: PathBuf::from("/tmp/kook_tavern_v15_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec!["g_ws".into()],
        allowed_channel_ids: vec![],
        admin_user_ids: vec!["u_admin".into()],
        export_channel_id: None,
        web_bind_addr: None,
        web_public_base_url: None,
        daily_max_llm_calls: 0,
        daily_max_cny: 0.0,
        price_cache_hit_cny_per_m: 0.2,
        price_cache_miss_cny_per_m: 2.0,
        price_output_cny_per_m: 3.0,
        jev: JevConfig::default(),
    }
}

fn msg(chan: &str, uid: &str, name: &str, content: &str) -> IncomingMessage {
    IncomingMessage {
        guild_id: Some("g_ws".into()),
        channel_id: chan.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct: false,
    }
}

fn assert_valid_kook_card(raw: &str) {
    let v: serde_json::Value = serde_json::from_str(raw).expect("valid card json");
    let modules = v[0]["modules"].as_array().expect("modules");
    assert!(modules.len() <= 50, "KOOK max 50 modules, got {}", modules.len());
    for m in modules {
        if m["type"] == "action-group" {
            let elems = m["elements"].as_array().expect("elements");
            assert!(elems.len() <= 4, "KOOK max 4 buttons per row, got {}", elems.len());
        }
    }
}

struct FakeStImporter {
    scenario: Scenario,
}

impl StRemoteImporter for FakeStImporter {
    fn import_from_url<'a>(&'a self, url: &'a str) -> BoxFuture<'a, anyhow::Result<Scenario>> {
        let sc = self.scenario.clone();
        Box::pin(async move {
            validate_allowlisted_import_url(url)?;
            Ok(sc)
        })
    }
}

#[test]
fn all_six_open_source_worldbook_packs_load_and_validate() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let catalog = FsWorldbookCatalog::load_from_dir(&root.join("worldbooks")).unwrap();
    let packs = catalog.list();
    assert_eq!(packs.len(), 6, "expected 6 curated worldbook packs");
    for p in &packs {
        validate_worldbook_pack(p).unwrap_or_else(|e| panic!("pack {} invalid: {e:?}", p.id));
        assert!(p.entries.len() >= 6, "pack {} has too few entries", p.id);
        assert_eq!(p.starter_hooks.len(), 3, "pack {} should have 3 starter hooks", p.id);
        assert!(!p.factions.is_empty(), "pack {} should have factions", p.id);
    }
}

#[tokio::test]
async fn end_to_end_worldbook_store_guided_workshop_forge_publish_and_play() {
    let tmp_dir = unique_temp_dir("flow");
    copy_builtin_scenarios_to(&tmp_dir);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    let cfg = v15_config(tmp_dir.clone());
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&tmp_dir).unwrap());
    let worldbooks = Arc::new(FsWorldbookCatalog::load_from_dir(&root.join("worldbooks")).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), scenarios.clone(), Arc::new(MockLlmProvider))
        .with_worldbooks(worldbooks);

    // 1. Open Worldbook Store via 2-part pinned workshop button `tavern:store`
    let store_resp = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u1", "作者喵", "tavern:store")
        .await
        .unwrap();
    assert_valid_kook_card(&render_kook_card_json("ch_wk", &store_resp));
    assert!(matches!(store_resp, EngineResponse::WorldbookStoreMenu { packs, .. } if packs.len() == 6));

    // 2. Inspect Worldbook Detail card & click 1-click starter hook `hook1:srd_sword_coast_fantasy`
    let det = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u1", "作者喵", "tavern:store:ch_wk:srd_sword_coast_fantasy")
        .await
        .unwrap();
    assert_valid_kook_card(&render_kook_card_json("ch_wk", &det));
    assert!(matches!(det, EngineResponse::WorldbookDetail { .. }));

    let desk1 = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u1", "作者喵", "tavern:ws:ch_wk:hook1:srd_sword_coast_fantasy")
        .await
        .unwrap();
    assert_valid_kook_card(&render_kook_card_json("ch_wk", &desk1));
    match &desk1 {
        EngineResponse::WorkshopDesk { draft, base_pack, .. } => {
            assert_eq!(draft.worldbook_id.as_deref(), Some("srd_sword_coast_fantasy"));
            assert_eq!(draft.check_system, CheckSystem::D20);
            assert_eq!(draft.act_count, 5);
            assert!(draft.premise.contains("灯塔核心"));
            assert!(base_pack.is_some());
        }
        other => panic!("expected WorkshopDesk, got {other:?}"),
    }

    // 3. Customize to 6-act epic + title + custom lore
    let _ = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u1", "作者喵", "tavern:ws:ch_wk:acts:6")
        .await
        .unwrap();
    let _ = engine
        .handle_message(msg("ch_wk", "u1", "作者喵", "/workshop title 晨潮港灯塔谜案"))
        .await
        .unwrap();
    let _ = engine
        .handle_message(msg(
            "ch_wk",
            "u1",
            "作者喵",
            "/workshop lore 铜锚密令 | 铜锚,议会,密令 | 港督私下签发过允许海怪上岸的密令，藏在灯塔夹层",
        ))
        .await
        .unwrap();

    // 4. Forge with AI -> validates & previews 6-act D&D module fused with base + custom lore!
    assert!(engine.will_button_likely_resolve_turn(Some("g_ws"), "ch_wk", "u1", "tavern:ws:ch_wk:forge"));
    let forged = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u1", "作者喵", "tavern:ws:ch_wk:forge")
        .await
        .unwrap();
    assert_valid_kook_card(&render_kook_card_json("ch_wk", &forged));
    let forged_id = match forged {
        EngineResponse::WorkshopDesk { draft, .. } => {
            let sc = draft.forged_scenario.expect("forged scenario present");
            assert_eq!(sc.name, "晨潮港灯塔谜案");
            assert_eq!(sc.acts.len(), 6);
            assert_eq!(sc.rules.check_system, CheckSystem::D20);
            assert!(sc.worldbook.iter().any(|w| w.title.contains("晨潮港")));
            assert!(sc.worldbook.iter().any(|w| w.title.contains("铜锚密令")));
            sc.id
        }
        other => panic!("expected forged WorkshopDesk, got {other:?}"),
    };

    // 5. Immediate 2nd forge hits 60s cooldown (even after `/workshop clear`!)
    let _ = engine.handle_message(msg("ch_wk", "u1", "作者喵", "/workshop idea 这是一个用来试探冷却的新脑洞描述")).await.unwrap();
    let cd = engine.handle_message(msg("ch_wk", "u1", "作者喵", "/workshop forge")).await.unwrap();
    assert!(matches!(cd, EngineResponse::Notice { title, .. } if title.contains("冷却中")));

    // 6. Publish a freshly forged draft from u2 -> immediately playable on a tabletop channel!
    let _ = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u2", "创作者B", "tavern:ws:ch_wk:hook2:cyber_kowloon_2099")
        .await
        .unwrap();
    let forged_u2 = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u2", "创作者B", "tavern:ws:ch_wk:forge")
        .await
        .unwrap();
    let pub_id = match forged_u2 {
        EngineResponse::WorkshopDesk { draft, .. } => draft.forged_scenario.unwrap().id,
        other => panic!("expected forged u2 desk, got {other:?}"),
    };
    assert_ne!(pub_id, forged_id);

    let pub_resp = engine
        .handle_button_in_guild(Some("g_ws"), "ch_wk", "u2", "创作者B", "tavern:ws:ch_wk:publish")
        .await
        .unwrap();
    assert!(matches!(pub_resp, EngineResponse::Notice { title, .. } if title.contains("已成功上架")));
    assert!(scenarios.get(&pub_id).is_some());
    assert!(!scenarios.is_builtin(&pub_id));

    // Start the community scenario immediately on Table 1!
    let start_resp = engine
        .handle_message(msg("ch_t1", "u2", "创作者B", &format!("/start {pub_id}")))
        .await
        .unwrap();
    assert!(matches!(start_resp, EngineResponse::StoryTurn { .. }));

    // 7. Security: Non-admin CANNOT delete community scenario; admin CANNOT delete built-in; admin CAN delete community!
    let deny_del = engine.handle_message(msg("ch_wk", "u2", "创作者B", &format!("/workshop delete {pub_id}"))).await.unwrap();
    assert!(matches!(deny_del, EngineResponse::Notice { title, .. } if title.contains("仅管理员")));

    let deny_builtin = engine.handle_message(msg("ch_wk", "u_admin", "服主", "/workshop delete ember_tavern")).await.unwrap();
    assert!(matches!(deny_builtin, EngineResponse::Notice { body, .. } if body.contains("官方内置")));

    let ok_del = engine.handle_message(msg("ch_wk", "u_admin", "服主", &format!("/workshop delete {pub_id}"))).await.unwrap();
    assert!(matches!(ok_del, EngineResponse::Notice { title, .. } if title.contains("已下架")));
    assert!(scenarios.get(&pub_id).is_none());

    let _ = fs::remove_dir_all(&tmp_dir);
}

#[tokio::test]
async fn private_me_quick_buttons_and_ssrf_protected_st_import() {
    let tmp_dir = unique_temp_dir("me_btn");
    copy_builtin_scenarios_to(&tmp_dir);
    let cfg = v15_config(tmp_dir.clone());
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&tmp_dir).unwrap());
    let sample_imported = {
        let mut sc = scenarios.get("ember_tavern").unwrap();
        sc.id = "ember_tavern".into(); // Deliberately collides with built-in to test protection!
        sc.name = "导入测试本".into();
        sc
    };
    let engine = GameEngine::new(cfg, store.clone(), scenarios.clone(), Arc::new(MockLlmProvider))
        .with_st_importer(Arc::new(FakeStImporter { scenario: sample_imported }));

    // 1. Start sunforge_keep, pick Paladin, check `/me` -> has clickable signature skill buttons!
    let _ = engine.handle_message(msg("ch_m", "u1", "圣骑", "/start sunforge_keep")).await.unwrap();
    let _ = engine
        .handle_button_in_guild(Some("g_ws"), "ch_m", "u1", "圣骑", "tavern:pickrole:ch_m:0")
        .await
        .unwrap();
    let me1 = engine.handle_message(msg("ch_m", "u1", "圣骑", "/me")).await.unwrap();
    match me1 {
        EngineResponse::PrivateNotice { title, body, actions, .. } => {
            assert_eq!(actions.len(), 2);
            assert_eq!(actions[0].value, "tavern:skill:ch_m:1");
            let card = render_private_card_json(&title, &body, &actions);
            assert_valid_kook_card(&card);
            assert!(card.contains("tavern:skill:ch_m:1"));
        }
        other => panic!("expected PrivateNotice, got {other:?}"),
    }

    // Click the private `/me` skill button directly!
    let sk = engine
        .handle_button_in_guild(Some("g_ws"), "ch_m", "u1", "圣骑", "tavern:skill:ch_m:1")
        .await
        .unwrap();
    assert!(matches!(sk, EngineResponse::Notice { title, .. } if title.contains("圣光护佑")));

    // Advance act -> `/me` gains clickable levelup buttons (`tavern:lv:ch_m:hp`, `tavern:lv:ch_m:charge`)!
    let _ = engine.handle_message(msg("ch_m", "u1", "圣骑", "推进进入下一幕")).await.unwrap();
    let me2 = engine.handle_message(msg("ch_m", "u1", "圣骑", "/me")).await.unwrap();
    match me2 {
        EngineResponse::PrivateNotice { actions, .. } => {
            assert!(actions.iter().any(|a| a.value == "tavern:lv:ch_m:hp"));
            assert!(actions.iter().any(|a| a.value == "tavern:lv:ch_m:charge"));
        }
        other => panic!("expected PrivateNotice, got {other:?}"),
    }
    let lv = engine
        .handle_button_in_guild(Some("g_ws"), "ch_m", "u1", "圣骑", "tavern:lv:ch_m:hp")
        .await
        .unwrap();
    assert!(matches!(lv, EngineResponse::Notice { title, .. } if title.contains("成长强化完成")));

    // 2. Remote ST import: non-admin rejected, SSRF URL rejected, allowlisted URL succeeds & never overwrites built-in!
    let deny_u1 = engine
        .handle_message(msg("ch_m", "u1", "圣骑", "/workshop import https://raw.githubusercontent.com/a/b.json"))
        .await
        .unwrap();
    assert!(matches!(deny_u1, EngineResponse::Notice { title, .. } if title.contains("仅管理员")));

    let ssrf = engine
        .handle_message(msg("ch_m", "u_admin", "服主", "/workshop import https://169.254.169.254/latest/meta-data"))
        .await
        .unwrap();
    assert!(matches!(ssrf, EngineResponse::Notice { title, .. } if title.contains("远程导入失败")));

    let ok_imp = engine
        .handle_message(msg("ch_m", "u_admin", "服主", "/workshop import https://raw.githubusercontent.com/org/repo/main/card.json"))
        .await
        .unwrap();
    assert!(matches!(ok_imp, EngineResponse::Notice { title, .. } if title.contains("导入并上架成功")));
    // Built-in `ember_tavern` untouched; imported renamed to `ember_tavern_ws`!
    assert_eq!(scenarios.get("ember_tavern").unwrap().name, "余烬酒馆与迷雾委托");
    assert_eq!(scenarios.get("ember_tavern_ws").unwrap().name, "导入测试本");

    let _ = fs::remove_dir_all(&tmp_dir);
}

struct FailingValidationLlm;

impl LlmProvider for FailingValidationLlm {
    fn generate_turn<'a>(
        &'a self, _: &'a Scenario, _: &'a RoomState, _: &'a [Player], _: &'a [TurnRecord],
        _: &'a [PendingAction], _: &'a [WorldEcho], _: bool,
    ) -> BoxFuture<'a, anyhow::Result<DmTurnOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn summarize_turns<'a>(
        &'a self, _: &'a Scenario, _: &'a str, _: &'a [TurnRecord],
    ) -> BoxFuture<'a, anyhow::Result<SummaryOutput>> {
        Box::pin(async { anyhow::bail!("unused") })
    }
    fn forge_scenario<'a>(
        &'a self, _: &'a ForgeScenarioRequest,
    ) -> BoxFuture<'a, anyhow::Result<ForgeScenarioOutput>> {
        Box::pin(async {
            Ok(ForgeScenarioOutput::ValidationFailed {
                error: "mock invalid json".into(),
                usage: LlmUsage {
                    prompt_tokens: 1500,
                    completion_tokens: 900,
                    cache_hit_tokens: 0,
                    cache_miss_tokens: 1500,
                },
            })
        })
    }
}

#[tokio::test]
async fn gate9_red_team_regressions_dos_cooldown_budget_and_menu_order() {
    let tmp_dir = unique_temp_dir("gate9");
    copy_builtin_scenarios_to(&tmp_dir);
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&tmp_dir).unwrap());

    // 1. P0-1: Extreme raw LLM stats (<= -4 / i32::MIN / 99) MUST terminate cleanly & pass point-buy!
    let mut wild_sc = scenarios.get("ember_tavern").unwrap();
    wild_sc.id = "000_extreme_stats".into();
    wild_sc.worldbook.clear();
    wild_sc.preset_roles[0].stats = CharacterStats {
        body: -10,
        agility: 99,
        investigate: i32::MIN,
        social: 3,
        insight: 3,
    };
    let mut draft = WorkshopDraft::new("u1");
    draft.premise = "极端异常数值与空白世界书测试脑洞".into();
    let req = ForgeScenarioRequest { draft, base_pack: None };
    let finalized = finalize_forged_scenario(wild_sc, &req, None).unwrap();
    assert!(validate_point_buy(&finalized.preset_roles[0].stats).is_ok());
    // P1-3: Blank worldbook auto-gains an `always_active: true` core entry!
    assert!(finalized.worldbook.iter().any(|w| w.always_active));

    // P1-1: Community scenario with "000_..." ID NEVER evicts/outranks official built-ins in `list()`!
    let _ = scenarios.publish_community(&finalized, false).unwrap();
    let listed = scenarios.list();
    assert!(scenarios.is_builtin(&listed[0].id));
    assert_eq!(listed.last().unwrap().id, "000_extreme_stats");

    // 2. P0-2: ValidationFailed STILL records usage AND keeps 60s cooldown locked (no retry drain!)
    let cfg = v15_config(tmp_dir.clone());
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let eng_fail = GameEngine::new(cfg.clone(), store.clone(), scenarios.clone(), Arc::new(FailingValidationLlm));
    let _ = eng_fail.handle_message(msg("ch_g9", "u_drain", "测试员", "/workshop idea 一段合法的测试主线脑洞描写")).await.unwrap();
    let r1 = eng_fail.handle_message(msg("ch_g9", "u_drain", "测试员", "/workshop forge")).await.unwrap();
    assert!(matches!(r1, EngineResponse::Notice { title, .. } if title.contains("未通过校验")));
    assert_eq!(store.get_today_usage().unwrap().calls, 1);
    let r2 = eng_fail.handle_message(msg("ch_g9", "u_drain", "测试员", "/workshop forge")).await.unwrap();
    assert!(matches!(r2, EngineResponse::Notice { title, .. } if title.contains("冷却中")));

    // 3. P0-4: Unauthorized guild & cooldown/empty-draft NEVER trigger thinking placeholder!
    assert!(!eng_fail.will_button_likely_resolve_turn(Some("g_evil"), "ch_g9", "u_drain", "tavern:ws:ch_g9:forge"));
    assert!(!eng_fail.will_button_likely_resolve_turn(Some("g_ws"), "ch_g9", "u_drain", "tavern:ws:ch_g9:forge"));
    assert!(!eng_fail.will_button_likely_resolve_turn(Some("g_ws"), "ch_g9", "u_empty", "tavern:ws:ch_g9:forge"));

    // 4. P1-1: Single forged draft publishes at most ONCE (cannot spam 32 slots!)
    let eng_ok = GameEngine::new(cfg, store.clone(), scenarios.clone(), Arc::new(MockLlmProvider));
    let _ = eng_ok.handle_message(msg("ch_g9", "u_pub", "作者", "/workshop idea 一段测试单次上架的完整主线脑洞")).await.unwrap();
    let _ = eng_ok.handle_message(msg("ch_g9", "u_pub", "作者", "/workshop forge")).await.unwrap();
    let p1 = eng_ok.handle_message(msg("ch_g9", "u_pub", "作者", "/workshop publish")).await.unwrap();
    assert!(matches!(p1, EngineResponse::Notice { title, .. } if title.contains("已成功上架")));
    let p2 = eng_ok.handle_message(msg("ch_g9", "u_pub", "作者", "/workshop publish")).await.unwrap();
    assert!(matches!(p2, EngineResponse::Notice { title, .. } if title.contains("还没有可上架")));

    // 5. v1.6.1: Panel `reset_ask` (2-step RBAC confirm) + `reset_confirm` + `leave` buttons
    let _ = eng_ok.handle_message(msg("ch_end", "u_gm", "房主", "/start ember_tavern")).await.unwrap();
    let _ = eng_ok.handle_button_in_guild(Some("g_ws"), "ch_end", "u_p2", "路人", "tavern:pickrole:ch_end:0").await.unwrap();
    let deny_ask = eng_ok.handle_button_in_guild(Some("g_ws"), "ch_end", "u_p2", "路人", "tavern:reset_ask:ch_end").await.unwrap();
    assert!(matches!(deny_ask, EngineResponse::PrivateNotice { title, actions, .. } if title.contains("仅房主") && actions.iter().any(|a| a.value == "tavern:leave:ch_end")));
    let leave_ok = eng_ok.handle_button_in_guild(Some("g_ws"), "ch_end", "u_p2", "路人", "tavern:leave:ch_end").await.unwrap();
    assert!(matches!(leave_ok, EngineResponse::Notice { title, .. } if title.contains("已退出")));
    let gm_ask = eng_ok.handle_button_in_guild(Some("g_ws"), "ch_end", "u_gm", "房主", "tavern:reset_ask:ch_end").await.unwrap();
    assert!(matches!(gm_ask, EngineResponse::PrivateNotice { actions, .. } if actions.iter().any(|a| a.value == "tavern:reset_confirm:ch_end")));
    let gm_reset = eng_ok.handle_button_in_guild(Some("g_ws"), "ch_end", "u_gm", "房主", "tavern:reset_confirm:ch_end").await.unwrap();
    assert!(matches!(gm_reset, EngineResponse::Notice { title, .. } if title.contains("结团清空")));
    assert!(store.get_room("ch_end").unwrap().is_none());

    let _ = fs::remove_dir_all(&tmp_dir);
}
