//! nipa-providers：元数据 provider 客户端 + egress 出站层（开发文档 §5、§8.4）。
//!
//! 已实现（M2a）：
//! - TMDB 客户端（Bearer token、language=zh-CN、append_to_response=external_ids）；
//! - Bangumi 客户端（UA 硬性要求、POST /v0/search/subjects、/v0/episodes）；
//! - egress 层：每 provider 令牌桶节流（TMDB 10 req/s、Bangumi 1 req/s）+
//!   进程内 TTL 缓存（搜索 6h / 详情 24h，上限 4096 条）；
//! - 五个只读 agent 工具（实现 `nipa_agent::Tool`），经 [`build_tools`] 组装。
//!
//! TODO(v1.x)：
//! - SQLite 响应级缓存（api_cache 表 + TTL，替换/前置进程内缓存）；
//! - SSRF 防护（禁 RFC1918/loopback、协议限 https——当前上游域名固定，
//!   base_url 仅测试可配，风险面可控）；
//! - TMDB 图片（include_image_language=zh,null）与署名义务；
//! - search_douban 可选编译 feature，默认关（§4.2）；
//! - AniList GraphQL（30 req/min）。

mod cache;
mod error;
mod throttle;

pub mod bangumi;
pub mod tmdb;
pub mod tools;

use serde::{Deserialize, Serialize};

pub use bangumi::{BangumiClient, DEFAULT_BANGUMI_BASE_URL, DEFAULT_BANGUMI_USER_AGENT};
pub use error::ProviderError;
pub use tmdb::{DEFAULT_TMDB_BASE_URL, MediaType, TmdbClient};
pub use tools::build_tools;

/// 元数据源标识（§9 item_ids.provider）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Tmdb,
    Bangumi,
    Dandanplay,
    Anilist,
    Imdb,
}

/// provider 搜索结果占位。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub provider: ProviderKind,
    pub external_id: String,
    pub title: String,
    pub original_title: Option<String>,
    pub year: Option<i32>,
}

/// egress 出站策略占位（§8.4 SSRF 防护）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// 是否允许 RFC1918/loopback 目标（默认 false，用户显式允许除外）。
    pub allow_private_targets: bool,
}
