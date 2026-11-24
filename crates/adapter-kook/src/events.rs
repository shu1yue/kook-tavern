use std::sync::OnceLock;

use regex::Regex;
use serde_json::Value;
use tavern_core::GameEngine;
use tavern_domain::{CardTheme, EngineResponse, IncomingMessage};
use tracing::{error, warn};

use crate::api::KookClient;
use crate::cards::chunk_kmarkdown;
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
    let channel_id = if is_direct {
        author_id.clone()
    } else {
        target_id
    };
    if channel_id.is_empty() || author_id.is_empty() {
        return;
    }

    let guild_id = d
        .pointer("/extra/guild_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let user_name = d
        .pointer("/extra/author/nickname")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| d.pointer("/extra/author/username").and_then(Value::as_str))
        .unwrap_or("冒险者")
        .to_string();

    let raw_content = d.get("content").and_then(Value::as_str).unwrap_or("");
    let incoming = IncomingMessage {
        guild_id,
        channel_id: channel_id.clone(),
        user_id: author_id,
        user_name,
        content: strip_kook_mentions(raw_content),
        is_direct,
    };

    match engine.handle_message(incoming).await {
        Ok(resp) => send_reply(&client, &channel_id, is_direct, resp).await,
        Err(err) => {
            error!(channel_id, is_direct, error = %format!("{err:#}"), "游戏引擎处理消息失败");
            let err_resp = EngineResponse::Notice {
                title: "❌ DM 刚刚打了个盹（处理失败）".to_string(),
                body: "服务或大模型接口刚刚出现短暂异常，房间状态安全无损，请稍后重试 `/go`（详情已记录服务器日志）。"
                    .to_string(),
                theme: CardTheme::Danger,
                show_actions: true,
            };
            send_reply(&client, &channel_id, is_direct, err_resp).await;
        }
    }
}

async fn handle_button_event(d: &Value, engine: GameEngine, client: KookClient) {
    let channel_id = d
        .pointer("/extra/body/target_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
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

    if channel_id.is_empty() || value.is_empty() {
        return;
    }
    match engine
        .handle_button(&channel_id, &user_id, &user_name, value)
        .await
    {
        Ok(resp) => client.dispatch_response(&channel_id, resp).await,
        Err(err) => {
            error!(channel_id, error = %format!("{err:#}"), "处理 KOOK 按钮事件失败");
            let err_resp = EngineResponse::Notice {
                title: "❌ 处理操作时出了点状况".to_string(),
                body: "操作暂时失败，房间状态安全无损，请稍后重试（详情已记录服务器日志）。"
                    .to_string(),
                theme: CardTheme::Danger,
                show_actions: false,
            };
            client.dispatch_response(&channel_id, err_resp).await;
        }
    }
}

async fn send_reply(client: &KookClient, channel_id: &str, is_direct: bool, resp: EngineResponse) {
    if matches!(resp, EngineResponse::Ignore) {
        return;
    }
    if is_direct {
        let text = render_plain_fallback(&resp, true);
        for chunk in chunk_kmarkdown(&text, 3500) {
            if let Err(err) = client.send_direct_kmarkdown(channel_id, &chunk).await {
                warn!(user_id = channel_id, error = %err, "回复 KOOK 私信失败");
                break;
            }
        }
    } else {
        client.dispatch_response(channel_id, resp).await;
    }
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
