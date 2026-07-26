//! 应用共享状态。

use nipa_core::{EventMsg, ServerConfig};
use sqlx::SqlitePool;
use std::sync::Arc;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<ServerConfig>,
    /// `--headless` 运行时开关（§1）。当前仅存状态，M5 起裁剪路由。
    pub headless: bool,
    pub db: SqlitePool,
    /// SSE 事件总线：业务侧 send，SSE handler subscribe。
    pub events: broadcast::Sender<EventMsg>,
    /// 刮削服务；None = [model] 未配置（capabilities.ai_scrape=false）。
    pub scrape: Option<crate::scrape::ScrapeService>,
    /// 管家服务；None = [model] 未配置。
    pub steward: Option<Arc<crate::steward::StewardService>>,
}
