use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tavern_adapter_web::{BroadcastEventBus, WebState};
use tavern_core::{AppConfig, BackgroundTaskRunner, BoxFuture, GameEngine, RoomStore};
use tavern_infra_llm::{build_llm_provider, generate_scenario_with_llm};
use tavern_infra_sqlite::{
    FsExportWriter, FsScenarioCatalog, SqliteRoomStore, import_sillytavern_file,
    write_scenario_yaml,
};
use tokio::sync::{Semaphore, oneshot};
use tracing::{error, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    name = "kook-tavern-dm",
    version,
    about = "KOOK AI 群酒馆 / 轻量文字冒险与跑团 DM 引擎"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 启动 KOOK 机器人并连接官方 WebSocket 网关
    Kook {
        /// 使用离线模拟 DM（仅用于不耗 API 调试 KOOK 卡片与交互）
        #[arg(long, default_value_t = false)]
        mock_llm: bool,
    },
    /// 启动本地终端多人模拟器（无需 KOOK Token 即可试玩与调试）
    Local {
        /// 模拟频道 ID
        #[arg(long, default_value = "local_tavern")]
        channel: String,
        /// 使用离线模拟 DM（无需大模型 API Key 即可体验完整流程）
        #[arg(long, default_value_t = false)]
        mock_llm: bool,
    },
    /// AI 一键生成并校验合规 v2 多幕多结局 YAML 剧本
    GenScenario {
        /// 题材脑洞描述（例如："深夜高校旧图书馆规则怪谈"）
        premise: String,
        /// 自定义英文剧本 ID（默认由 AI 自动生成并安全清洗）
        #[arg(long)]
        id: Option<String>,
        /// 指定 Web 伴生大屏氛围主题（ember / fog / metro）
        #[arg(long)]
        theme: Option<String>,
        /// 允许覆盖已存在的同名剧本文件
        #[arg(long, default_value_t = false)]
        overwrite: bool,
        /// 离线模拟生成（无需 API Key，用于本地演练）
        #[arg(long, default_value_t = false)]
        mock_llm: bool,
    },
    /// 导入 SillyTavern PNG/JSON 角色卡或世界书为本引擎 YAML 剧本
    ImportSt {
        /// SillyTavern `.png` 或 `.json` 文件路径
        input: PathBuf,
        /// 自定义英文剧本 ID（默认取文件名并安全清洗）
        #[arg(long)]
        id: Option<String>,
        /// 指定 Web 伴生大屏氛围主题（ember / fog / metro）
        #[arg(long)]
        theme: Option<String>,
        /// 允许覆盖已存在的同名剧本文件
        #[arg(long, default_value_t = false)]
        overwrite: bool,
    },
}

struct TokioBackgroundRunner {
    permits: Arc<Semaphore>,
}

impl TokioBackgroundRunner {
    fn new(max_concurrent: usize) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(max_concurrent)),
        }
    }
}

impl BackgroundTaskRunner for TokioBackgroundRunner {
    fn spawn(&self, fut: BoxFuture<'static, ()>) {
        let permits = self.permits.clone();
        tokio::spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return;
            };
            fut.await;
        });
    }
}

struct BuiltRuntime {
    engine: GameEngine,
    web_bind: Option<String>,
    web_state: WebState,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let _ = dotenvy::dotenv();
    let cfg = AppConfig::from_env()?;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,kook_tavern_dm=info,tavern_=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match cli.command.unwrap_or(Commands::Kook { mock_llm: false }) {
        Commands::Kook { mock_llm } => {
            let token = cfg.require_kook_token()?.to_string();
            let runtime = build_runtime(cfg, mock_llm)?;
            let web_handle = spawn_optional_web(runtime.web_bind, runtime.web_state);
            let res = tavern_adapter_kook::run_kook_bot(token, runtime.engine).await;
            shutdown_web(web_handle).await;
            res
        }
        Commands::Local { channel, mock_llm } => {
            let runtime = build_runtime(cfg, mock_llm)?;
            let web_handle = spawn_optional_web(runtime.web_bind, runtime.web_state);
            let res = tavern_adapter_cli::run_local_cli(runtime.engine, channel, mock_llm).await;
            shutdown_web(web_handle).await;
            res
        }
        Commands::GenScenario {
            premise,
            id,
            theme,
            overwrite,
            mock_llm,
        } => {
            let sc = generate_scenario_with_llm(
                &cfg,
                &premise,
                id.as_deref(),
                theme.as_deref(),
                mock_llm,
            )
            .await?;
            let saved = write_scenario_yaml(&cfg.scenarios_dir, &sc, overwrite)?;
            println!(
                "✅ AI 剧本生成并校验通过！\n- 剧本名：《{}》 (`{}`)\n- 主题：{}\n- 预设职业：{} 个 ｜ 幕次：{} 幕 ｜ 结局：{} 个 ｜ 世界书：{} 条\n- 已保存：{}\n\n👉 现在去 KOOK 频道发 `/reload`（无需重启服务），即可立刻 `/start {}` 开玩！",
                sc.name,
                sc.id,
                sc.theme,
                sc.preset_roles.len(),
                sc.acts.len(),
                sc.endings.len(),
                sc.worldbook.len(),
                saved.display(),
                sc.id
            );
            Ok(())
        }
        Commands::ImportSt {
            input,
            id,
            theme,
            overwrite,
        } => {
            let sc = import_sillytavern_file(&input, id.as_deref(), theme.as_deref())?;
            let saved = write_scenario_yaml(&cfg.scenarios_dir, &sc, overwrite)?;
            println!(
                "✅ SillyTavern 导入并校验通过！\n- 剧本名：《{}》 (`{}`)\n- 主题：{} ｜ 世界书：{} 条\n- 已保存：{}\n\n👉 现在去 KOOK 频道发 `/reload`（无需重启服务），即可立刻 `/start {}` 开玩！",
                sc.name,
                sc.id,
                sc.theme,
                sc.worldbook.len(),
                saved.display(),
                sc.id
            );
            Ok(())
        }
    }
}

fn spawn_optional_web(
    web_bind: Option<String>,
    web_state: WebState,
) -> Option<(oneshot::Sender<()>, tokio::task::JoinHandle<()>)> {
    let addr = web_bind?;
    let (tx, rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let shutdown = async move {
            let _ = rx.await;
        };
        if let Err(err) = tavern_adapter_web::run_web_server(&addr, web_state, shutdown).await {
            error!(addr, error = %format!("{err:#}"), "伴生 Web 看板启动或运行失败（不影响 KOOK 主流程）");
        }
    });
    Some((tx, handle))
}

async fn shutdown_web(handle: Option<(oneshot::Sender<()>, tokio::task::JoinHandle<()>)>) {
    if let Some((tx, join)) = handle {
        let _ = tx.send(());
        let _ = tokio::time::timeout(Duration::from_secs(2), join).await;
    }
}

fn build_runtime(cfg: AppConfig, mock_llm: bool) -> Result<BuiltRuntime> {
    let llm = build_llm_provider(&cfg, mock_llm)?;
    let store = Arc::new(SqliteRoomStore::open(&cfg.db_path)?);
    let recovered = store.reset_stuck_resolving_on_startup()?;
    if recovered > 0 {
        warn!(
            recovered,
            "启动恢复：已将上次异常中断卡在 Resolving 的房间安全重置为 Collecting"
        );
    }
    let scenarios = Arc::new(FsScenarioCatalog::load_from_dir(&cfg.scenarios_dir)?);
    let exporter = Arc::new(FsExportWriter::new(cfg.exports_dir.clone()));
    let bg_runner = Arc::new(TokioBackgroundRunner::new(8));
    let event_bus = Arc::new(BroadcastEventBus::new(256));
    let web_bind = cfg.web_bind_addr.clone();
    let web_state = WebState::new(store.clone(), scenarios.clone(), event_bus.sender());

    let engine = GameEngine::new(cfg, store, scenarios, llm).with_ports(
        event_bus,
        exporter,
        bg_runner,
    );
    Ok(BuiltRuntime {
        engine,
        web_bind,
        web_state,
    })
}
