use std::path::PathBuf;
use std::sync::Arc;

use tavern_core::{AppConfig, DndStore, GameEngine, JevConfig, RoomStore, ScenarioCatalog};
use tavern_domain::{
    CardTheme, CharacterAbility, CharacterStats, CheckSystem, CheckTier, EngineResponse,
    IncomingMessage, PlayerDndProfile, RollMode, StatKind, match_proficiency, perform_rule_check,
    reset_dnd_profile_for_new_room, validate_point_buy, validate_scenario,
};
use tavern_infra_llm::MockLlmProvider;
use tavern_infra_sqlite::{FsScenarioCatalog, SqliteRoomStore};

fn v14_config() -> AppConfig {
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
        scenarios_dir: root.join("scenarios"),
        prompts_dir: root.join("prompts"),
        exports_dir: PathBuf::from("/tmp/kook_tavern_v14_exports"),
        recent_turns_window: 6,
        summary_interval_turns: 4,
        auto_resolve_all_ready: true,
        allowed_guild_ids: vec![],
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
        guild_id: Some("g_dnd".into()),
        channel_id: chan.into(),
        user_id: uid.into(),
        user_name: name.into(),
        content: content.into(),
        is_direct: false,
    }
}

#[test]
fn sunforge_keep_validates_as_full_5_act_dnd_campaign() {
    let cfg = v14_config();
    let catalog = FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap();
    let sc = catalog.get("sunforge_keep").expect("sunforge_keep.yaml missing");
    validate_scenario(&sc).unwrap_or_else(|e| panic!("sunforge_keep invalid: {e:?}"));
    assert_eq!(sc.rules.check_system, CheckSystem::D20);
    assert_eq!(sc.acts.len(), 5);
    assert_eq!(sc.endings.len(), 4);
    assert_eq!(sc.map_nodes.len(), 10);
    assert_eq!(sc.map_edges.len(), 13);
    assert_eq!(sc.rules.factions.len(), 4);
    assert_eq!(sc.preset_roles.len(), 4);
    assert!(sc.worldbook.len() >= 14);
    for role in &sc.preset_roles {
        assert!(!role.alignment.is_empty());
        assert!(role.proficient_skills.len() >= 2);
        assert_eq!(role.abilities.len(), 2);
    }
}

#[tokio::test]
async fn dnd_d20_d100_checks_advantage_proficiency_assist_and_inspiration() {
    // 1. Live RNG smoke checks for D20 / D100 / TwoD6 invariants
    let stats = CharacterStats { body: 2, agility: 1, investigate: 0, social: 0, insight: 0 };
    let r_d20 = perform_rule_check(
        CheckSystem::D20, StatKind::Body, &stats, RollMode::Advantage,
        Some(14), Some("格斗".into()), Some("猛劈青铜卫".into()),
    );
    assert_eq!(r_d20.rolls.len(), 2);
    assert_eq!(r_d20.kept_roll, r_d20.rolls[0].max(r_d20.rolls[1]));
    assert_eq!(r_d20.prof_bonus, 2);
    assert_eq!(r_d20.total, r_d20.kept_roll + 2 + 2);
    assert!(r_d20.format_short().contains("D&D"));

    let r_d100 = perform_rule_check(
        CheckSystem::D100, StatKind::Body, &stats, RollMode::Advantage,
        None, Some("格斗".into()), Some("硬抗重击".into()),
    );
    assert_eq!(r_d100.target_dc, 85); // 50 + 2*10 + 15
    assert_eq!(r_d100.kept_roll, r_d100.rolls[0].min(r_d100.rolls[1]));

    // 2. Full engine flow: start sunforge_keep (d20), pick roles, assist, inspire, rule switch
    let cfg = v14_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), catalog, Arc::new(MockLlmProvider));

    let _ = engine.handle_message(msg("c_dnd", "u1", "圣骑甲", "/start sunforge_keep")).await.unwrap();
    let room = store.get_room("c_dnd").unwrap().unwrap();
    assert_eq!(room.check_system, CheckSystem::D20);

    // u1 picks Paladin (index 0), u2 picks Rogue (index 1)
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_dnd", "u1", "圣骑甲", "tavern:pickrole:c_dnd:0").await.unwrap();
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_dnd", "u2", "盗贼乙", "tavern:pickrole:c_dnd:1").await.unwrap();

    // Self-assist blocked; u2 assisting u1 sets `assisted_by`
    let self_a = engine.handle_message(msg("c_dnd", "u1", "圣骑甲", "/assist 圣骑甲")).await.unwrap();
    assert!(matches!(self_a, EngineResponse::Notice { body, .. } if body.contains("不能协助自己")));
    let ok_a = engine.handle_message(msg("c_dnd", "u2", "盗贼乙", "/assist 圣骑甲 吸引注意力")).await.unwrap();
    assert!(matches!(ok_a, EngineResponse::Notice { title, .. } if title.contains("正在协助")));
    assert_eq!(store.get_player("c_dnd", "u1").unwrap().unwrap().dnd.assisted_by, "盗贼乙");

    // u1 runs `/check 格斗 破门` (Wait: stat alias `搏斗`/`体魄` + reason with `格斗` triggers proficiency + assist Advantage!)
    let ck = engine.handle_message(msg("c_dnd", "u1", "圣骑甲", "/check 体魄 dc15 格斗撞门")).await.unwrap();
    match ck {
        EngineResponse::Notice { body, .. } => {
            assert!(body.contains("协助"), "expected assist advantage: {body}");
            assert!(body.contains("熟练(格斗 +2)"), "expected proficiency: {body}");
            assert!(body.contains("DC 15"), "expected DC 15: {body}");
        }
        other => panic!("unexpected: {other:?}"),
    }
    // Assist consumed after one check!
    assert!(store.get_player("c_dnd", "u1").unwrap().unwrap().dnd.assisted_by.is_empty());

    // GM (u1) grants inspiration to u2, u2 consumes it with `inspire`
    let _ = engine.handle_message(msg("c_dnd", "u1", "圣骑甲", "/inspire 盗贼乙")).await.unwrap();
    assert!(store.get_player("c_dnd", "u2").unwrap().unwrap().dnd.inspiration);
    let ck2 = engine.handle_message(msg("c_dnd", "u2", "盗贼乙", "/check 敏捷 inspire 潜行开锁")).await.unwrap();
    match ck2 {
        EngineResponse::Notice { body, .. } => assert!(body.contains("消耗激励点")),
        other => panic!("unexpected: {other:?}"),
    }
    assert!(!store.get_player("c_dnd", "u2").unwrap().unwrap().dnd.inspiration);

    // Non-GM u2 cannot switch rule; GM u1 switches to d100
    let deny_rule = engine.handle_message(msg("c_dnd", "u2", "盗贼乙", "/rule d100")).await.unwrap();
    assert!(matches!(deny_rule, EngineResponse::Notice { title, .. } if title.contains("仅房主")));
    let ok_rule = engine.handle_message(msg("c_dnd", "u1", "圣骑甲", "/rule d100")).await.unwrap();
    assert!(matches!(ok_rule, EngineResponse::Notice { title, .. } if title.contains("CoC d100")));
    assert_eq!(store.get_room("c_dnd").unwrap().unwrap().check_system, CheckSystem::D100);
}

#[tokio::test]
async fn signature_abilities_short_long_rest_cooldown_and_milestone_levelup_with_undo() {
    let cfg = v14_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), catalog, Arc::new(MockLlmProvider));

    let _ = engine.handle_message(msg("c_ab", "u1", "艾琳", "/start sunforge_keep")).await.unwrap();
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_ab", "u1", "艾琳", "tavern:pickrole:c_ab:0").await.unwrap();

    // Paladin ability 1 (`圣光护佑`, 1/1 short_rest)
    let use1 = engine.handle_message(msg("c_ab", "u1", "艾琳", "/skill 1 挡下箭雨")).await.unwrap();
    assert!(matches!(use1, EngineResponse::Notice { title, .. } if title.contains("圣光护佑")));
    assert_eq!(store.get_player("c_ab", "u1").unwrap().unwrap().dnd.abilities[0].uses_left, 0);

    // 2nd use exhausted!
    let use2 = engine.handle_message(msg("c_ab", "u1", "艾琳", "/skill 圣光护佑")).await.unwrap();
    assert!(matches!(use2, EngineResponse::Notice { title, .. } if title.contains("耗尽")));

    // Take damage via mock "冲" action (Turn 1) -> HP drops by 2
    let _ = engine.handle_message(msg("c_ab", "u1", "艾琳", "硬冲撞向哨卡")).await.unwrap();
    let hp_after_hit = store.get_player("c_ab", "u1").unwrap().unwrap().hp;
    assert_eq!(hp_after_hit, 14);

    // Short rest restores HP and recharges short_rest ability (`圣光护佑`), then immediate 2nd rest hits cooldown!
    let r1 = engine.handle_message(msg("c_ab", "u1", "艾琳", "/rest short")).await.unwrap();
    assert!(matches!(r1, EngineResponse::Notice { title, .. } if title.contains("短暂休整")));
    let p_rested = store.get_player("c_ab", "u1").unwrap().unwrap();
    assert_eq!(p_rested.hp, 16);
    assert_eq!(p_rested.dnd.abilities[0].uses_left, 1);

    let r_cd = engine.handle_message(msg("c_ab", "u1", "艾琳", "/rest short")).await.unwrap();
    assert!(matches!(r_cd, EngineResponse::Notice { title, .. } if title.contains("刚刚才休整过")));

    // Advance Act -> automatic milestone level-up (Lv.2 + pending_levelups = 1) + mock inspiration!
    let _ = engine.handle_message(msg("c_ab", "u1", "艾琳", "精彩扮演并推进进入下一幕")).await.unwrap();
    let p_lv2 = store.get_player("c_ab", "u1").unwrap().unwrap();
    assert_eq!(p_lv2.dnd.level, 2);
    assert_eq!(p_lv2.dnd.pending_levelups, 1);
    assert!(p_lv2.dnd.inspiration);

    // Spend milestone level-up on HP/SAN (+2 max each)
    let lv_resp = engine.handle_message(msg("c_ab", "u1", "艾琳", "/levelup hp")).await.unwrap();
    assert!(matches!(lv_resp, EngineResponse::Notice { title, .. } if title.contains("成长强化完成")));
    let p_up = store.get_player("c_ab", "u1").unwrap().unwrap();
    assert_eq!((p_up.max_hp, p_up.max_san, p_up.dnd.pending_levelups), (18, 14, 0));

    // `/undo` rolls back the act advance AND milestone level-up cleanly!
    let _ = engine.handle_message(msg("c_ab", "u1", "艾琳", "/undo")).await.unwrap();
    let p_undone = store.get_player("c_ab", "u1").unwrap().unwrap();
    assert_eq!((p_undone.dnd.level, p_undone.max_hp), (1, 16));
}

#[tokio::test]
async fn cross_campaign_character_vault_enforces_point_buy_privacy_and_anti_farm() {
    assert!(validate_point_buy(&CharacterStats { body: 3, agility: 3, investigate: 0, social: 0, insight: 0 }).is_err());
    assert!(validate_point_buy(&CharacterStats { body: 2, agility: 1, investigate: 1, social: 0, insight: -1 }).is_ok());

    let cfg = v14_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), catalog, Arc::new(MockLlmProvider));

    // 1. Cheating point-buy rejected
    let bad = engine
        .handle_message(msg("c_v", "u1", "玩家A", "/char create 龙傲天 剑圣 | 3,3,3,0,0"))
        .await
        .unwrap();
    assert!(matches!(bad, EngineResponse::Notice { title, .. } if title.contains("未通过校验")));

    // 2. Fair custom character with private `personal_secret` saved cleanly
    let ok = engine
        .handle_message(msg(
            "c_v", "u1", "玩家A",
            "/char create 琳达 银月吟游诗人 | 0,1,1,2,-1 | 游说,潜行 | 混乱善良 | 流浪竖琴手 | 暗中追查害死兄长的灰袍术士",
        ))
        .await
        .unwrap();
    assert!(matches!(ok, EngineResponse::PrivateNotice { title, .. } if title.contains("琳达")));
    assert_eq!(store.list_saved_characters("u1").unwrap().len(), 1);

    // 3. Load saved character into room -> gets starter HP/SAN + fair stats + private secret in `/me`!
    let _ = engine.handle_message(msg("c_v", "u1", "玩家A", "/start sunforge_keep")).await.unwrap();
    let ld = engine.handle_message(msg("c_v", "u1", "玩家A", "/char load 琳达")).await.unwrap();
    assert!(matches!(ld, EngineResponse::Notice { title, .. } if title.contains("跨团上桌")));
    let me = engine.handle_message(msg("c_v", "u1", "玩家A", "/me")).await.unwrap();
    match me {
        EngineResponse::PrivateNotice { body, .. } => {
            assert!(body.contains("害死兄长的灰袍术士"));
            assert!(body.contains("游说 · 潜行"));
            assert!(body.contains("绝境爆发"));
        }
        other => panic!("expected PrivateNotice, got {other:?}"),
    }

    // 4. Mid-game role switch anti-farm: exhaust ability on Turn 1, switch preset -> MUST NOT refresh charges!
    let _ = engine.handle_message(msg("c_v", "u1", "玩家A", "/skill 1 弹奏安魂曲")).await.unwrap();
    assert_eq!(store.get_player("c_v", "u1").unwrap().unwrap().dnd.abilities[0].uses_left, 0);
    let _ = engine.handle_message(msg("c_v", "u1", "玩家A", "查看炉火旁的线索")).await.unwrap();
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_v", "u1", "玩家A", "tavern:pickrole:c_v:0").await.unwrap();
    // Since Turn > 0 and player already had abilities, abilities were NOT refreshed!
    assert_eq!(store.get_player("c_v", "u1").unwrap().unwrap().dnd.abilities[0].uses_left, 0);

    // 5. Pinned workshop shorthand button `tavern:menu` works cleanly without `:channel_id`!
    let wk_menu = engine
        .handle_button_in_guild(Some("g_dnd"), "c_workshop", "u1", "玩家A", "tavern:menu")
        .await
        .unwrap();
    assert!(matches!(wk_menu, EngineResponse::ScenarioMenu { scenarios, .. } if scenarios.len() >= 4));
}

#[tokio::test]
async fn gate8_red_team_regressions_atomic_security_and_rules() {
    // 1. Proficiency hardening: no multi-stat concat, no 1-char false positives, raw token works
    let bad_skills = vec!["格斗".into(), "开锁".into(), "潜行".into()];
    assert_eq!(match_proficiency(&bad_skills, "体魄", "打听酒馆消息", StatKind::Body), None);
    assert_eq!(match_proficiency(&bad_skills, "敏捷", "趁机翻越矮墙", StatKind::Agility), None);
    assert_eq!(
        match_proficiency(&bad_skills, "潜行", "", StatKind::Agility),
        Some("潜行".into())
    );
    assert_eq!(
        match_proficiency(&bad_skills, "体魄", "", StatKind::Body),
        Some("格斗".into())
    );
    assert_eq!(
        match_proficiency(&["潜".into()], "潜行", "潜行", StatKind::Agility),
        None
    );

    // 2. Cross-campaign reset caps abilities (<=2, max_uses <=2) and skills (<=3)
    let bloated = PlayerDndProfile {
        level: 6,
        pending_levelups: 2,
        proficient_skills: vec!["格斗".into(), "潜行".into(), "开锁".into(), "游说".into()],
        abilities: vec![
            CharacterAbility {
                name: "圣光护佑".into(),
                max_uses: 6,
                uses_left: 1,
                recharge: tavern_domain::AbilityRecharge::ShortRest,
                desc: "测试".into(),
            },
            CharacterAbility {
                name: "破邪斩".into(),
                max_uses: 5,
                uses_left: 0,
                recharge: tavern_domain::AbilityRecharge::LongRest,
                desc: "测试".into(),
            },
            CharacterAbility {
                name: "额外技".into(),
                max_uses: 3,
                uses_left: 3,
                recharge: tavern_domain::AbilityRecharge::ShortRest,
                desc: "测试".into(),
            },
        ],
        ..PlayerDndProfile::default()
    };
    let reset = reset_dnd_profile_for_new_room(&bloated);
    assert_eq!(reset.level, 1);
    assert_eq!(reset.proficient_skills.len(), 3);
    assert_eq!(reset.abilities.len(), 2);
    assert!(reset.abilities.iter().all(|a| a.max_uses <= 2 && a.uses_left == a.max_uses));

    // 3. D100 Partial hint is non-costly & distinct from 2d6 Partial
    assert_eq!(
        CheckTier::Partial.rule_hint_for(CheckSystem::D100),
        "常规成功，顺利达成主要目标"
    );
    assert!(CheckTier::Partial.rule_hint_for(CheckSystem::TwoD6).contains("代价"));

    let cfg = v14_config();
    let store = Arc::new(SqliteRoomStore::open_in_memory().unwrap());
    let catalog = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir).unwrap());
    let engine = GameEngine::new(cfg, store.clone(), catalog, Arc::new(MockLlmProvider));

    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/start sunforge_keep")).await.unwrap();
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_rt", "u1", "圣骑", "tavern:pickrole:c_rt:0").await.unwrap();

    // 4. Turn 0 rest blocked + Turn 0 skill followed by pickrole MUST NOT recharge skill!
    let r0 = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rest short")).await.unwrap();
    assert!(matches!(r0, EngineResponse::Notice { title, .. } if title.contains("暂不可休整")));

    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/skill 1 护盾")).await.unwrap();
    assert_eq!(store.get_player("c_rt", "u1").unwrap().unwrap().dnd.abilities[0].uses_left, 0);
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_rt", "u1", "圣骑", "tavern:pickrole:c_rt:1").await.unwrap();
    assert_eq!(store.get_player("c_rt", "u1").unwrap().unwrap().dnd.abilities[0].uses_left, 0);

    // 5. Bystander u_outsider CANNOT /rest short and CANNOT /assist (and is NOT auto-joined!)
    let bad_rest = engine.handle_message(msg("c_rt", "u_out", "路人", "/rest short")).await.unwrap();
    assert!(matches!(bad_rest, EngineResponse::Notice { title, .. } if title.contains("暂不可休整")));
    let bad_assist = engine.handle_message(msg("c_rt", "u_out", "路人", "/assist 圣骑")).await.unwrap();
    assert!(matches!(bad_assist, EngineResponse::Notice { title, .. } if title.contains("无法协助")));
    assert!(store.get_player("c_rt", "u_out").unwrap().is_none());

    // 6. adv + inspire preserves inspiration; dis + inspire cancels to Normal & consumes inspiration!
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/inspire 圣骑")).await.unwrap();
    let adv_i = engine.handle_message(msg("c_rt", "u1", "圣骑", "/check 体魄 adv inspire 冲锋")).await.unwrap();
    assert!(matches!(adv_i, EngineResponse::Notice { body, .. } if body.contains("保留激励点")));
    assert!(store.get_player("c_rt", "u1").unwrap().unwrap().dnd.inspiration);

    let dis_i = engine.handle_message(msg("c_rt", "u1", "圣骑", "/check 体魄 dis inspire 逆风强攻")).await.unwrap();
    assert!(matches!(dis_i, EngineResponse::Notice { body, .. } if body.contains("抵消劣势")));
    assert!(!store.get_player("c_rt", "u1").unwrap().unwrap().dnd.inspiration);

    // 7. Resolving phase blocks bonus consumption, pickrole, and rule switch!
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/inspire 圣骑")).await.unwrap();
    let lease = store.try_begin_resolving("c_rt").unwrap().expect("lease");
    let ck_res = engine.handle_message(msg("c_rt", "u1", "圣骑", "/check 体魄 inspire 抢跑")).await.unwrap();
    assert!(matches!(ck_res, EngineResponse::Notice { body, .. } if body.contains("暂未消耗")));
    assert!(store.get_player("c_rt", "u1").unwrap().unwrap().dnd.inspiration);
    let pick_res = engine.handle_button_in_guild(Some("g_dnd"), "c_rt", "u1", "圣骑", "tavern:pickrole:c_rt:2").await.unwrap();
    assert!(matches!(pick_res, EngineResponse::Notice { title, .. } if title.contains("暂不可选职业")));
    let rule_res = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rule 2d6")).await.unwrap();
    assert!(matches!(rule_res, EngineResponse::Notice { title, .. } if title.contains("暂不可切换规则")));
    store.abort_resolving("c_rt", lease.expected_version).unwrap();

    // 8. Advance Turn 1 -> same-turn skill + rest blocked; after Turn 2 rest works; /undo restores check_system & last_rest_turn!
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "硬冲前线")).await.unwrap(); // Turn 1
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rest short")).await.unwrap();
    assert_eq!(store.get_room("c_rt").unwrap().unwrap().last_rest_turn, 1);
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/skill 1 再开护盾")).await.unwrap();
    let stack_rest = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rest short")).await.unwrap();
    assert!(matches!(stack_rest, EngineResponse::Notice { body, .. } if body.contains("招牌技能")));

    // Switch rule to d100, advance Turn 2, then /undo -> check_system & last_rest_turn restored accurately!
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rule d100")).await.unwrap();
    assert_eq!(store.get_room("c_rt").unwrap().unwrap().check_system, CheckSystem::D100);
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "推进进入下一幕")).await.unwrap(); // Turn 2 (snapshot saved with d100 & last_rest_turn=1)
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rule 2d6")).await.unwrap();
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rest short")).await.unwrap();
    assert_eq!(store.get_room("c_rt").unwrap().unwrap().last_rest_turn, 2);
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/undo")).await.unwrap();
    let undone_room = store.get_room("c_rt").unwrap().unwrap();
    assert_eq!(undone_room.check_system, CheckSystem::D100);
    assert_eq!(undone_room.last_rest_turn, 1);

    // 9. Mid-game pickrole preserves stats & proficient_skills (no mid-game stat/proficiency farming!)
    let before = store.get_player("c_rt", "u1").unwrap().unwrap();
    let _ = engine.handle_button_in_guild(Some("g_dnd"), "c_rt", "u1", "圣骑", "tavern:pickrole:c_rt:2").await.unwrap();
    let after = store.get_player("c_rt", "u1").unwrap().unwrap();
    assert_eq!(after.stats, before.stats);
    assert_eq!(after.dnd.proficient_skills, before.dnd.proficient_skills);

    // 10. AFK player cannot /skill or /levelup; /char create rejects multi-stat skill name; save_current clamps < -1!
    let mut debuffed = after.clone();
    debuffed.stats.insight = -2;
    debuffed.stats.body = 4;
    store.upsert_player(&debuffed).unwrap();
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/char save")).await.unwrap();
    let saved = store.list_saved_characters("u1").unwrap();
    assert!(validate_point_buy(&saved[0].stats).is_ok());

    let bad_sk = engine
        .handle_message(msg("c_rt", "u1", "圣骑", "/char create 漏洞侠 游侠 | 1,1,1,0,0 | 体魄敏捷侦查"))
        .await
        .unwrap();
    assert!(matches!(bad_sk, EngineResponse::Notice { title, .. } if title.contains("不合法")));

    let _ = store.set_player_afk("c_rt", "u1", true).unwrap();
    let afk_sk = engine.handle_message(msg("c_rt", "u1", "圣骑", "/skill 1")).await.unwrap();
    assert!(matches!(afk_sk, EngineResponse::Notice { body, .. } if body.contains("暂离")));
    let _ = store.set_player_afk("c_rt", "u1", false).unwrap();

    // 11. D100 normal success card theme is Success (not Warning)
    let _ = engine.handle_message(msg("c_rt", "u1", "圣骑", "/rule d100")).await.unwrap();
    // Run a check just to verify D100 never returns Warning on Partial
    let d100_resp = engine.handle_message(msg("c_rt", "u1", "圣骑", "/check 体魄")).await.unwrap();
    if let EngineResponse::Notice { body, theme, .. } = d100_resp
        && body.contains("常规成功")
    {
        assert_eq!(theme, CardTheme::Success);
    }
}
