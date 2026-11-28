use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
use tavern_core::{GameEngine, OocPrecheck};
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage};
use tracing::{error, warn};

use crate::api::KookClient;
use crate::cards::chunk_kmarkdown;
use crate::dispatcher::InPlaceSyncMode;
use crate::plain::render_plain_fallback;

pub async fn handle_kook_event(d: Value, engine: GameEngine, client: KookClient) {
    let msg_type = d.get("type").and_then(Value::as_i64).unwrap_or(0);
    let channel_type = d
        .get("channel_type")
        .and_then(Value::as_str)
        .unwrap_or("GROUP");

    if msg_type == 255 {
        let extra_type = d.pointer("/extra/type").and_then(Value::as_str).unwrap_or("");
        if extra_type == "message_btn_click" {
            handle_button_event(&d, engine, client).await;
        }
        return;
    }

    let is_bot = d
        .pointer("/extra/author/bot")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if is_bot || !matches!(msg_type, 1 | 9) {
        return;
    }

    let is_direct = channel_type.eq_ignore_ascii_case("PERSON");
    let author_id = d
        .get("author_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let target_id = d
        .get("target_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let reply_target = if is_direct {
        author_id.clone()
    } else {
        target_id.clone()
    };
    let channel_id = if is_direct {
        format!("dm:{author_id}")
    } else {
        target_id
    };
    if reply_target.is_empty() || author_id.is_empty() {
        return;
    }

    let guild_id = d
        .pointer("/extra/guild_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    if !is_direct
        && let Some(ref gid) = guild_id
    {
        client.cache_channel_guild(&channel_id, gid);
    }

    let user_name = d
        .pointer("/extra/author/nickname")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| d.pointer("/extra/author/username").and_then(Value::as_str))
        .unwrap_or("冒险者")
        .to_string();

    let raw_content = d.get("content").and_then(Value::as_str).unwrap_or("");
    let cleaned = strip_kook_mentions(raw_content);

    let mut will_resolve = !is_direct
        && engine.will_likely_resolve_turn(
            guild_id.as_deref(),
            &channel_id,
            &author_id,
            &cleaned,
            is_direct,
        );
    let mut prechecked_ooc = false;
    if will_resolve && engine.has_decision_gate() {
        match engine
            .precheck_roleplay_ooc(
                guild_id.as_deref(),
                &channel_id,
                &author_id,
                &user_name,
                &cleaned,
                is_direct,
            )
            .await
        {
            OocPrecheck::IgnoreOoc => return,
            OocPrecheck::Allow => {
                prechecked_ooc = true;
                // Re-verify after await in case room phase/mode changed while Jev was in-flight
                will_resolve = engine.will_likely_resolve_turn(
                    guild_id.as_deref(),
                    &channel_id,
                    &author_id,
                    &cleaned,
                    is_direct,
                );
            }
            OocPrecheck::NotApplicable => {}
        }
    }

    let placeholder_id = if will_resolve {
        client.send_thinking_placeholder(&reply_target).await
    } else {
        None
    };

    let sync_mode = if is_direct {
        InPlaceSyncMode::PublicCard
    } else {
        message_sync_mode(&cleaned)
    };
    let incoming = IncomingMessage {
        guild_id,
        channel_id: channel_id.clone(),
        user_id: author_id.clone(),
        user_name,
        content: cleaned,
        is_direct,
    };

    let result = engine
        .handle_message_prechecked(incoming, prechecked_ooc)
        .await;
    if let Some(pid) = placeholder_id {
        client.delete_channel_message(&pid).await;
    }

    match result {
        Ok(resp) => {
            if is_direct {
                send_direct_reply(&client, &reply_target, resp).await;
            } else {
                client
                    .dispatch_response_live(&reply_target, Some(&author_id), sync_mode, resp, &engine)
                    .await;
            }
        }
        Err(err) => {
            error!(channel_id, is_direct, error = %format!("{err:#}"), "游戏引擎处理消息失败");
            let err_resp = EngineResponse::Notice {
                title: "❌ DM 刚刚打了个盹（上游接口波动）".to_string(),
                body: "大模型接口刚刚偶发波动或返回空内容，**你的行动和房间进度完好无损**！直接点下方 **「⚡ 立刻结算」**（或发 `/go`）重试即可喵～"
                    .to_string(),
                theme: CardTheme::Danger,
                show_actions: !is_direct,
            };
            if is_direct {
                send_direct_reply(&client, &reply_target, err_resp).await;
            } else {
                client.dispatch_response(&reply_target, err_resp).await;
            }
        }
    }
}

async fn handle_button_event(d: &Value, engine: GameEngine, client: KookClient) {
    let channel_id = d
        .pointer("/extra/body/target_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let guild_id = d
        .pointer("/extra/body/guild_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);
    let user_id = d
        .pointer("/extra/body/user_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let user_name = d
        .pointer("/extra/body/user_info/nickname")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| d.pointer("/extra/body/user_info/username").and_then(Value::as_str))
        .unwrap_or("冒险者")
        .to_string();
    let value = d
        .pointer("/extra/body/value")
        .and_then(Value::as_str)
        .unwrap_or("");

    if channel_id.is_empty()
        || user_id.is_empty()
        || value.is_empty()
        || !value.starts_with("tavern:")
    {
        return;
    }

    // KOOK `message_btn_click` omits `guild_id`; resolve authoritatively via cached channel/view
    // so clickable start buttons in brand-new channels still respect `ALLOWED_GUILD_IDS`.
    let resolved_guild = match guild_id {
        Some(g) => Some(g),
        None => client.resolve_channel_guild(&channel_id).await,
    };

    let placeholder_id = if engine.will_button_likely_resolve_turn(
        resolved_guild.as_deref(),
        &channel_id,
        &user_id,
        value,
    ) {
        client.send_thinking_placeholder(&channel_id).await
    } else {
        None
    };

    let result = engine
        .handle_button_in_guild(
            resolved_guild.as_deref(),
            &channel_id,
            &user_id,
            &user_name,
            value,
        )
        .await;
    if let Some(pid) = placeholder_id {
        client.delete_channel_message(&pid).await;
    }

    let sync_mode = button_sync_mode(value);
    match result {
        Ok(resp) => {
            client
                .dispatch_response_live(&channel_id, Some(&user_id), sync_mode, resp, &engine)
                .await;
        }
        Err(err) => {
            error!(channel_id, error = %format!("{err:#}"), "处理 KOOK 按钮事件失败");
            let err_resp = EngineResponse::Notice {
                title: "❌ DM 刚刚打了个盹（上游接口波动）".to_string(),
                body: "大模型接口刚刚偶发波动或返回空内容，**你的行动和房间进度完好无损**！直接点下方 **「⚡ 立刻结算」** 重试即可喵～"
                    .to_string(),
                theme: CardTheme::Danger,
                show_actions: true,
            };
            client.dispatch_response(&channel_id, err_resp).await;
        }
    }
}

async fn send_direct_reply(client: &KookClient, user_id: &str, resp: EngineResponse) {
    if matches!(resp, EngineResponse::Ignore) {
        return;
    }
    let text = render_plain_fallback(&resp, true);
    for chunk in chunk_kmarkdown(&text, 3500) {
        if let Err(err) = client.send_direct_kmarkdown(user_id, &chunk).await {
            warn!(user_id, error = %err, "回复 KOOK 私信失败");
            break;
        }
    }
}

fn button_sync_mode(value: &str) -> InPlaceSyncMode {
    let action = value.split(':').nth(1).unwrap_or("");
    match action {
        "pickrole" | "quickjoin" | "pick" | "afk" => InPlaceSyncMode::SilentOnSuccess,
        "check" | "roll" | "skill" | "lv" | "leave" | "toggle_mode" | "toggle_style"
        | "toggle_pause" => InPlaceSyncMode::TempToast,
        _ => InPlaceSyncMode::PublicCard,
    }
}

fn message_sync_mode(content: &str) -> InPlaceSyncMode {
    let t = content.trim();
    if t.starts_with('>') || t.starts_with('》') {
        return InPlaceSyncMode::SilentOnSuccess;
    }
    if let Some(rest) = t.strip_prefix('/').or_else(|| t.strip_prefix('／'))
        .or_else(|| t.strip_prefix('!')).or_else(|| t.strip_prefix('！'))
        .or_else(|| t.strip_prefix('.')).or_else(|| t.strip_prefix('。'))
        .or_else(|| t.strip_prefix('、'))
    {
        let mut parts = rest.split_whitespace();
        let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
        let has_arg = parts.next().is_some();
        return match cmd.as_str() {
            "join" | "card" | "车卡" | "入团" | "afk" | "暂离" | "摸鱼" => {
                InPlaceSyncMode::SilentOnSuccess
            }
            "act" | "a" | "行动" | "assist" | "协助" => {
                if has_arg {
                    InPlaceSyncMode::SilentOnSuccess
                } else {
                    InPlaceSyncMode::TempToast
                }
            }
            "roll" | "r" | "掷骰" | "check" | "ck" | "检定" | "属性"
            | "skill" | "sk" | "技能" | "绝活"
            | "levelup" | "lv" | "升级" | "成长" | "mode" | "模式" | "style" | "风格" | "档位"
            | "pause" | "等人" | "飙戏" => {
                InPlaceSyncMode::TempToast
            }
            _ => InPlaceSyncMode::PublicCard,
        };
    }
    InPlaceSyncMode::SilentOnSuccess
}

fn mention_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\(met\)[^()]*\(met\)").expect("valid mention regex"))
}

pub fn strip_kook_mentions(input: &str) -> String {
    mention_regex().replace_all(input, "").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_kook_mentions_cleanly() {
        assert_eq!(strip_kook_mentions("(met)123456(met) /act 推开门"), "/act 推开门");
        assert_eq!(strip_kook_mentions("普通消息"), "普通消息");
    }
}
