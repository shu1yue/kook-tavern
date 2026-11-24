use std::io::Read;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use flate2::read::ZlibDecoder;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tavern_core::GameEngine;
use tokio::sync::Semaphore;
use tokio::time::{interval, sleep, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::api::KookClient;
use crate::events::handle_kook_event;

const MAX_ZLIB_BYTES: u64 = 4 * 1024 * 1024;
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_CONCURRENT_EVENTS: usize = 32;

pub async fn run_kook_bot(token: String, engine: GameEngine) -> Result<()> {
    let client = KookClient::new(token)?;
    let event_permits = Arc::new(Semaphore::new(MAX_CONCURRENT_EVENTS));
    let mut session_id: Option<String> = None;
    let mut last_sn: i64 = 0;
    let mut backoff_secs: u64 = 2;

    info!("KOOK AI 群酒馆机器人启动中...");

    loop {
        let gateway_base = match client.get_gateway_url().await {
            Ok(u) => u,
            Err(err) => {
                error!(error = %err, wait_secs = backoff_secs, "获取 KOOK Gateway 失败，稍后重试");
                if wait_backoff_or_shutdown(backoff_secs).await {
                    return Ok(());
                }
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }
        };

        let ws_url = if let Some(ref sid) = session_id
            && last_sn > 0
        {
            let sep = if gateway_base.contains('?') { '&' } else { '?' };
            format!("{gateway_base}{sep}resume=1&sn={last_sn}&session_id={sid}")
        } else {
            gateway_base
        };

        info!("正在连接 KOOK WebSocket 网关...");
        let ws_stream = match timeout(WS_CONNECT_TIMEOUT, connect_async(&ws_url)).await {
            Ok(Ok((stream, _))) => stream,
            Ok(Err(err)) => {
                error!(error = %err, wait_secs = backoff_secs, "KOOK WebSocket 连接失败");
                if wait_backoff_or_shutdown(backoff_secs).await {
                    return Ok(());
                }
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }
            Err(_) => {
                error!(wait_secs = backoff_secs, "KOOK WebSocket 建连超时（>20s）");
                if wait_backoff_or_shutdown(backoff_secs).await {
                    return Ok(());
                }
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }
        };

        info!("KOOK WebSocket 已建立连接，等待 Hello 握手...");
        let (mut write, mut read) = ws_stream.split();
        let mut ping_timer = interval(Duration::from_secs(30));
        ping_timer.tick().await;
        let mut pong_check = interval(Duration::from_secs(2));
        let mut awaiting_pong_since: Option<Instant> = None;
        let mut reset_session = false;

        loop {
            tokio::select! {
                _ = shutdown_signal() => {
                    info!("收到优雅退出信号（SIGINT/SIGTERM），关闭 KOOK 机器人");
                    return Ok(());
                }
                _ = ping_timer.tick() => {
                    let ping_payload = json!({ "s": 2, "sn": last_sn }).to_string();
                    if let Err(err) = write.send(WsMessage::Text(ping_payload.into())).await {
                        warn!(error = %err, "发送心跳 Ping 失败，准备重连");
                        break;
                    }
                    awaiting_pong_since = Some(Instant::now());
                    debug!(sn = last_sn, "已发送 KOOK 心跳 Ping");
                }
                _ = pong_check.tick() => {
                    if let Some(sent_at) = awaiting_pong_since
                        && sent_at.elapsed() > Duration::from_secs(8)
                    {
                        warn!("KOOK 心跳 Pong 超时（>8s），主动触发重连");
                        break;
                    }
                }
                frame = read.next() => {
                    let Some(frame) = frame else {
                        warn!("KOOK WebSocket 流已关闭，准备重连");
                        break;
                    };
                    let msg = match frame {
                        Ok(m) => m,
                        Err(err) => {
                            warn!(error = %err, "读取 KOOK WebSocket 帧出错");
                            break;
                        }
                    };

                    let raw_text = match msg {
                        WsMessage::Binary(bytes) => match decompress_zlib_bounded(&bytes) {
                            Ok(s) => s,
                            Err(err) => {
                                warn!(error = %err, "zlib 解压 KOOK 数据帧失败");
                                continue;
                            }
                        },
                        WsMessage::Text(t) => t.to_string(),
                        WsMessage::Ping(payload) => {
                            let _ = write.send(WsMessage::Pong(payload)).await;
                            continue;
                        }
                        WsMessage::Close(_) => {
                            warn!("KOOK 服务端主动关闭连接");
                            break;
                        }
                        _ => continue,
                    };

                    let packet: Value = match serde_json::from_str(&raw_text) {
                        Ok(v) => v,
                        Err(err) => {
                            warn!(error = %err, "解析 KOOK 信令 JSON 失败");
                            continue;
                        }
                    };

                    let signal = packet.get("s").and_then(Value::as_i64).unwrap_or(-1);
                    match signal {
                        1 => {
                            let code = packet.pointer("/d/code").and_then(Value::as_i64).unwrap_or(-1);
                            if code == 0 {
                                session_id = packet
                                    .pointer("/d/session_id")
                                    .and_then(Value::as_str)
                                    .map(String::from);
                                backoff_secs = 2;
                                info!(session_id = ?session_id, "KOOK 握手成功，机器人已上线待命！");
                            } else {
                                error!(code, "KOOK Hello 握手失败，重置会话并重试");
                                reset_session = true;
                                break;
                            }
                        }
                        3 => {
                            awaiting_pong_since = None;
                            debug!("收到 KOOK 心跳 Pong");
                        }
                        5 => {
                            warn!("收到 KOOK Reconnect(s=5) 指令，清空会话重连");
                            reset_session = true;
                            break;
                        }
                        6 => {
                            backoff_secs = 2;
                            info!("KOOK 会话 Resume ACK 成功，离线消息已同步");
                        }
                        0 => {
                            let sn = packet.get("sn").and_then(Value::as_i64).unwrap_or(0);
                            if sn > 0 && sn <= last_sn {
                                debug!(sn, last_sn, "忽略重复或乱序旧事件");
                                continue;
                            }
                            if sn > last_sn {
                                last_sn = sn;
                            }
                            if let Some(d) = packet.get("d").cloned() {
                                let engine_clone = engine.clone();
                                let client_clone = client.clone();
                                let permits = event_permits.clone();
                                tokio::spawn(async move {
                                    let Ok(_permit) = permits.acquire_owned().await else {
                                        return;
                                    };
                                    handle_kook_event(d, engine_clone, client_clone).await;
                                });
                            }
                        }
                        other => debug!(signal = other, "忽略未处理的 KOOK 信令"),
                    }
                }
            }
        }

        if reset_session {
            session_id = None;
            last_sn = 0;
        }
        if wait_backoff_or_shutdown(backoff_secs).await {
            return Ok(());
        }
        backoff_secs = (backoff_secs * 2).min(60);
    }
}

async fn wait_backoff_or_shutdown(secs: u64) -> bool {
    tokio::select! {
        _ = shutdown_signal() => {
            info!("退避等待期间收到退出信号，优雅退出");
            true
        }
        _ = sleep(Duration::from_secs(secs)) => false
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut sigterm = signal(SignalKind::terminate()).ok();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = async {
                if let Some(ref mut s) = sigterm {
                    s.recv().await;
                } else {
                    std::future::pending::<()>().await;
                }
            } => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn decompress_zlib_bounded(bytes: &[u8]) -> Result<String> {
    let mut decoder = ZlibDecoder::new(bytes).take(MAX_ZLIB_BYTES + 1);
    let mut buf = Vec::new();
    decoder.read_to_end(&mut buf)?;
    if buf.len() as u64 > MAX_ZLIB_BYTES {
        bail!("KOOK 压缩帧解压后超过 4 MiB 安全上限，已拒绝处理");
    }
    Ok(String::from_utf8(buf)?)
}
