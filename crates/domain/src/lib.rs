pub mod chronicle;
pub mod decision;
pub mod dice;
pub mod dnd;
pub mod dnd_ops;
pub mod dnd_prof;
pub mod guards;
pub mod lorebook;
pub mod models;
pub mod patch;
pub mod protocol;
pub mod stats;
pub mod validate;
pub mod workshop;
pub mod world;

pub use chronicle::{ChronicleEntry, WorldEcho, build_chronicle, distill_echo_text};
pub use decision::{LoreCandidate, OocVerdict, ReplyAuditVerdict};
pub use dice::{DiceRollResult, roll_dice, roll_dice_with_rng};
pub use dnd::{
    AbilityRecharge, CharacterAbility, CheckSystem, PlayerDndProfile, RestKind, RollMode,
    RuleCheckResult, SavedCharacter, perform_rule_check, perform_rule_check_with_rng,
};
pub use dnd_ops::{
    LevelUpChoice, apply_inspiration_grants, apply_level_up, apply_rest_to_players,
    award_act_milestone, check_tier_hint_for, check_tier_label_for, parse_point_buy_stats,
    reset_dnd_profile_for_new_room, validate_point_buy,
};
pub use dnd_prof::match_proficiency;
pub use guards::{
    ConsistencyNudgePlan, DmPatchDelta, SpotlightGuidance, analyze_spotlight,
    detect_consistency_nudge, detect_consistency_nudge_with_audit, merge_patch_delta,
};
pub use lorebook::{always_active_worldbook, match_dynamic_worldbook, match_worldbook};
pub use models::*;
pub use patch::{AppliedTurnEffects, apply_dm_reply, apply_story_progression};
pub use protocol::*;
pub use stats::{
    CharacterStats, CheckTier, EndingUnlock, ScenarioAct, ScenarioEnding, StatCheckResult,
    StatKind, SuggestedCheck, ending_scope_key, perform_stat_check, perform_stat_check_with_rng,
};
pub use validate::{normalize_theme, sanitize_scenario_id, validate_scenario};
pub use workshop::{
    ForgeScenarioOutput, ForgeScenarioRequest, PrivateButton, WorldbookPack, WorldbookPackSummary,
    WorkshopDraft, validate_worldbook_pack,
};
pub use world::{
    DirectorBeat, FactionAgenda, ScenarioMapEdge, ScenarioMapNode, ScenarioRules, WorldBulletin,
    due_director_beats, resolve_map_node_id, should_trigger_bulletin,
};
