use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tavern_domain::LoreCandidate;

#[derive(Debug, Serialize)]
pub(crate) struct BailianSystemOneRequest<'a> {
    pub model: &'a str,
    pub state: String,
    pub questions: Value,
}

#[derive(Debug, Serialize)]
pub(crate) struct GoogleDecisionsRequest<'a> {
    pub model: &'a str,
    pub input: String,
    pub questions: Value,
}

#[derive(Debug, Deserialize)]
pub(crate) struct DecisionResponse {
    #[serde(default)]
    pub answers: Option<HashMap<String, Option<ChoiceAnswer>>>,
    #[serde(default)]
    pub latency_ms: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChoiceAnswer {
    #[serde(default)]
    pub probabilities: Option<HashMap<String, Option<f64>>>,
    #[serde(default)]
    pub noul: Option<f64>,
}

fn validate_unit_prob(v: f64) -> Option<f64> {
    (v.is_finite() && (-0.01..=1.01).contains(&v)).then(|| v.clamp(0.0, 1.0))
}

pub(crate) fn extract_yes_prob(parsed: &DecisionResponse, key: &str) -> Option<f64> {
    let ans = parsed.answers.as_ref()?.get(key)?.as_ref()?;
    if let Some(probs) = ans.probabilities.as_ref() {
        if let Some(Some(p)) = probs.get("yes") {
            return validate_unit_prob(*p);
        }
        if let Some(Some(no)) = probs.get("no") {
            return validate_unit_prob(*no).map(|v| (1.0 - v).clamp(0.0, 1.0));
        }
    }
    ans.noul.and_then(validate_unit_prob)
}

pub(crate) fn clip_chars(s: &str, max_chars: usize) -> String {
    s.trim().chars().take(max_chars).collect()
}

pub(crate) fn build_ooc_state_and_questions(
    scenario_name: &str,
    location: &str,
    recent_scene: &str,
    sender_name: &str,
    message: &str,
) -> (String, Value) {
    let state = format!(
        "【正在进行的文字冒险剧本】{}（当前地点：{}）\n【最近剧情摘要】{}\n【群友最新发言】{}: {}",
        clip_chars(scenario_name, 40),
        clip_chars(location, 48),
        clip_chars(recent_scene, 360),
        clip_chars(sender_name, 24),
        clip_chars(message, 400),
    );
    let questions = json!({
        "is_ooc_chat": {
            "type": "choice",
            "instructions": "判断这条群聊发言是否属于纯粹的【场外闲聊 OOC】（与推进剧情或扮演角色无关的现实琐事、打招呼、笑声感叹、议论机器人或等我一下）？只要有一点角色行动、观察环境、对NPC说话、使用道具或推进故事的意图，一律判 no。",
            "criteria": {
                "yes": "纯粹现实场外闲聊、拿外卖/上厕所/等一下、纯笑声(哈哈/笑死/666)、讨论机器人头像或系统指令，完全没有角色行动或台词",
                "no": "操控角色行动、检查调查物品、移动地点、对NPC或队友说剧情台词、提问故事细节或任何跑团推进意图"
            }
        }
    });
    (state, questions)
}

pub(crate) fn build_lore_state_and_questions(
    location: &str,
    action_corpus: &str,
    candidates: &[LoreCandidate],
) -> (String, Value) {
    let state = format!(
        "【当前所在地点】{}\n【本轮玩家行动与最近上下文】\n{}",
        clip_chars(location, 48),
        clip_chars(action_corpus, 1000)
    );
    let mut q_map = Map::new();
    for (idx, c) in candidates.iter().enumerate() {
        q_map.insert(
            format!("lore_{idx}"),
            json!({
                "type": "choice",
                "instructions": format!(
                    "结合当前地点和玩家本轮行动，判断这条隐藏世界书设定【{}】（摘要：{}）是否与玩家此刻的行动目标、观察对象或所处危机直接相关、值得提供给 DM 参考？",
                    clip_chars(&c.title, 36),
                    clip_chars(&c.snippet, 120)
                ),
                "criteria": {
                    "yes": "玩家行动或场景直接触及该设定中的人物、物件、地点或暗线真相",
                    "no": "与本轮行动和当前场景无关，暂不需要引入"
                }
            }),
        );
    }
    (state, Value::Object(q_map))
}

pub(crate) fn build_audit_state_and_questions(
    narrative: &str,
    whispers_summary: &str,
    has_vitals_patch: bool,
    has_item_patch: bool,
) -> (String, Value, Vec<&'static str>) {
    let check_whisper = !whispers_summary.trim().is_empty();
    let mut q_map = Map::new();
    let mut req_keys = Vec::new();
    if !has_vitals_patch {
        req_keys.push("missed_vitals");
        q_map.insert(
            "missed_vitals".into(),
            json!({
                "type": "choice",
                "instructions": "正文是否明确描写了某位玩家角色在本轮实际受伤、流血、灼伤、中毒或遭受剧烈精神冲击/理智受创？（注意：“幸好没受伤/险险避开/擦肩而过”必须判 no）",
                "criteria": { "yes": "角色实际承受了肉体伤害或明显精神理智打击", "no": "未受伤，或仅仅有惊无险避开" }
            }),
        );
    }
    if !has_item_patch {
        req_keys.push("missed_item");
        q_map.insert(
            "missed_item".into(),
            json!({
                "type": "choice",
                "instructions": "正文是否明确描写了某位玩家角色新获得了、拾取了或被NPC交给了某件具体物品/钥匙/道具/药剂？（仅远远看见而未拿走判 no）",
                "criteria": { "yes": "角色明确拿到、收起或获赠了一件具体道具", "no": "没有新获得道具，或只是远远看到" }
            }),
        );
    }
    if check_whisper {
        req_keys.push("leaked_whisper");
        q_map.insert(
            "leaked_whisper".into(),
            json!({
                "type": "choice",
                "instructions": "对照【本应仅私聊保密的秘密纸条】与【公开发布的剧情正文】，判断公开正文是否不小心直接泄露出秘密纸条的核心保密情报？",
                "criteria": { "yes": "公开正文直接写出了秘密纸条里本该只告诉单人的核心秘密", "no": "公开正文未泄露私密纸条内容，保密完好" }
            }),
        );
    }
    let state = if check_whisper {
        format!(
            "【公开发布的剧情正文】\n{}\n\n【本应仅私聊保密的秘密纸条】\n{}",
            clip_chars(narrative, 1800),
            clip_chars(whispers_summary, 700)
        )
    } else {
        format!("【公开发布的剧情正文】\n{}", clip_chars(narrative, 1800))
    };
    (state, Value::Object(q_map), req_keys)
}
