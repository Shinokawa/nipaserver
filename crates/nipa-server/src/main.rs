//! NipaServer 主程序：配置加载 → tracing → SQLite/迁移 → axum 装配 → 优雅停机。
//!
//! 里程碑 M0 骨架（开发文档 §12）。

mod api;
mod api_library;
mod api_userdata;
mod db;
mod images;
mod ingest;
mod scan;
mod scrape;
mod state;
mod steward;
mod userdata;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use nipa_core::{EventMsg, ServerConfig};
use tokio::sync::broadcast;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

/// NipaServer——围绕 AI Agent 刮削的媒体服务器。
#[derive(Debug, Parser)]
#[command(name = "nipa-server", version)]
struct Cli {
    /// 无头纯扫描器模式（同一二进制的运行时开关，§1）。
    /// TODO(M5): 无头模式下裁剪 WebUI 路由与播放端点；当前仅存入状态。
    #[arg(long)]
    headless: bool,
}

fn load_config() -> anyhow::Result<ServerConfig> {
    // 优先级：env > file > default（§11）。
    let path = std::env::var("NIPA_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./nipaserver.toml"));
    let (mut config, from_file) = ServerConfig::load_file(&path)
        .with_context(|| format!("加载配置 {} 失败", path.display()))?;
    config.apply_env_overrides().context("应用环境变量覆盖失败")?;
    // tracing 尚未初始化，先用 eprintln 提示配置来源。
    if !from_file {
        eprintln!(
            "config: {} 不存在，使用默认配置（env 覆盖仍生效）",
            path.display()
        );
    }
    Ok(config)
}

fn init_tracing(config: &ServerConfig) {
    // RUST_LOG 优先，其次配置文件 log.filter，默认 info。
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.log.filter.clone()));
    tracing_subscriber::fmt().with_env_filter(filter).init();
    // TODO(§11): 分级滚动日志文件（WebUI 日志页数据源）。
}

/// 优雅停机信号：Ctrl-C 或 SIGTERM（§11）。
async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("安装 Ctrl-C 处理器失败");
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("安装 SIGTERM 处理器失败")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("收到停机信号，开始优雅停机");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = load_config()?;
    init_tracing(&config);

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        headless = cli.headless,
        data_dir = %config.server.data_dir.display(),
        "NipaServer 启动"
    );

    // SQLite（WAL、外键）+ 迁移。
    let pool = db::open(&config.server.data_dir).await?;

    // SSE 事件总线（§2.1 SQ/EQ 风格；§8.1 /events）。
    let (events_tx, _) = broadcast::channel::<EventMsg>(256);

    // 心跳任务：每 30s 广播一条 heartbeat（M0 stub；后续由 scanner/agent 推真实事件）。
    {
        let events_tx = events_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(30));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                let ts = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                // 无订阅者时 send 返回 Err，属正常情况，忽略。
                let _ = events_tx.send(EventMsg::Heartbeat { ts });
            }
        });
    }

    // 元数据源工具（§5）：TMDB 无 token 时为 None（仅 Bangumi 工具），
    // Bangumi 免认证但 UA 必须合规。
    let tmdb = if config.providers.tmdb_token.trim().is_empty() {
        tracing::warn!("providers.tmdb_token 未配置，search_tmdb 等工具不可用");
        None
    } else {
        match nipa_providers::TmdbClient::new(&config.providers.tmdb_token, None) {
            Ok(c) => Some(c),
            Err(e) => {
                tracing::warn!(error = %e, "TMDB 客户端初始化失败");
                None
            }
        }
    };
    let bangumi_ua = if config.providers.bangumi_user_agent.trim().is_empty() {
        None
    } else {
        Some(config.providers.bangumi_user_agent.clone())
    };
    let bangumi = nipa_providers::BangumiClient::new(None, bangumi_ua)
        .context("Bangumi 客户端初始化失败")?;
    let mut tools = nipa_providers::build_tools(tmdb, bangumi);

    // ffmpeg 探测（§6.3）：可用时给 agent 加 probe_media/extract_subtitle
    // 工具（路径限定在已配置库的根内，§8.4）。缺失时 evidence 走降级形态。
    let ffmpeg_paths = nipa_stream::FfmpegLocator::detect();
    match &ffmpeg_paths {
        Some(p) => {
            let roots: Vec<std::path::PathBuf> =
                sqlx::query_as::<_, (String,)>("SELECT path FROM libraries")
                    .fetch_all(&pool)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(p,)| std::path::PathBuf::from(p))
                    .collect();
            tools.extend(nipa_stream::build_stream_tools(p, roots));
            tracing::info!(version = %p.ffmpeg_version, "ffmpeg 就绪，媒体探测工具已启用");
        }
        None => tracing::warn!("未找到 ffmpeg/ffprobe，L2 evidence 降级为文件名+目录形态（§6.3）"),
    }

    // 弹弹play L1（§4.1）：启动时从分发服务器拉 appSecret；失败自动降级 L2-only。
    let dandan: Option<Arc<nipa_match::DandanClient>> = if config.providers.dandanplay_l1 {
        match nipa_match::fetch_app_secret(concat!("nipaserver/", env!("CARGO_PKG_VERSION"))).await
        {
            Some(secret) => {
                let auth = nipa_match::DandanAuth::Signature {
                    app_id: nipa_match::NIPA_APP_ID.to_string(),
                    app_secret: secret,
                };
                match nipa_match::DandanClient::new(auth) {
                    Ok(c) => {
                        tracing::info!("弹弹play L1 就绪（签名模式）");
                        Some(Arc::new(c))
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "弹弹play 客户端初始化失败，降级 L2-only");
                        None
                    }
                }
            }
            None => {
                tracing::warn!("appSecret 获取失败（分发服务器不可达），降级 L2-only");
                None
            }
        }
    } else {
        None
    };

    // 刮削服务（[model] 未配置时为 None，capabilities.ai_scrape=false）。
    let scrape =
        scrape::ScrapeService::start(&config.model, tools.clone(), pool.clone(), events_tx.clone());

    // 管家：worker 只读工具 + 管家专属工具（docs/06-管家设计.md）。
    let steward_tools = steward::tools::build_steward_tools(
        pool.clone(),
        events_tx.clone(),
        scrape.clone(),
        tools,
        api::SCRAPE_SYSTEM_PROMPT,
    );
    let steward = steward::StewardService::new(
        &config.model,
        pool.clone(),
        events_tx.clone(),
        steward_tools,
    )
    .map(Arc::new);

    // 管家巡检（主动唤醒；docs/06 §2）。stop 经 patrol_cancel 传播。
    let patrol_cancel = tokio_util::sync::CancellationToken::new();
    if let Some(s) = &steward {
        steward::patrol::spawn_patrol(
            s.clone(),
            pool.clone(),
            events_tx.clone(),
            patrol_cancel.clone(),
        );
    }

    let state = state::AppState {
        config: Arc::new(config.clone()),
        headless: cli.headless,
        db: pool.clone(),
        events: events_tx,
        scrape: scrape.clone(),
        steward: steward.clone(),
        dandan,
        ffmpeg_available: ffmpeg_paths.is_some(),
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("nipaserver/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("构建 HTTP 客户端失败")?,
    };

    // WebUI 静态伺服（rust-embed 是 M5 发布形态；开发期直接伺服 dist 目录，
    // SPA fallback 到 index.html）。无 dist 时仅 API。
    let webui_dist = std::path::Path::new("webui/app/dist");
    let mut app = api::router(state);
    if webui_dist.is_dir() && !cli.headless {
        let serve = tower_http::services::ServeDir::new(webui_dist)
            .fallback(tower_http::services::ServeFile::new(webui_dist.join("index.html")));
        app = app.fallback_service(serve);
        tracing::info!("WebUI 已挂载（webui/app/dist）");
    }
    let app = app.layer(TraceLayer::new_for_http());

    let ip: IpAddr = config
        .server
        .bind
        .parse()
        .with_context(|| format!("无效监听地址: {}", config.server.bind))?;
    let addr = SocketAddr::new(ip, config.server.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("绑定 {addr} 失败"))?;
    tracing::info!(%addr, "HTTP 服务监听中");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP 服务异常退出")?;

    // 停机收尾（§11）。TODO(M3/M4): kill 转码 ffmpeg 子进程、librqbit session flush。
    patrol_cancel.cancel();
    if let Some(s) = &scrape {
        s.shutdown();
    }
    if let Some(s) = &steward {
        s.shutdown();
    }
    if let Err(e) = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE);")
        .execute(&pool)
        .await
    {
        tracing::warn!(error = %e, "WAL checkpoint 失败");
    }
    pool.close().await;
    tracing::info!("已退出");
    Ok(())
}
