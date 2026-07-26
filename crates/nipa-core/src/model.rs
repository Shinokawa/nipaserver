//! 领域类型占位，字段对齐开发文档 §9 数据库 schema。
//!
//! TODO(M1): 补齐 sqlx FromRow 派生与查询层（放在 server 侧或 feature 门控，
//! 避免 nipa-core 无条件引入 sqlx）。

use serde::{Deserialize, Serialize};

/// 用户角色（§9 users.role：admin | guest）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserRole {
    Admin,
    Guest,
}

/// 用户（§9 users 表）。v1 最小两角色：单管理员 + 只读访客。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub role: UserRole,
    /// argon2id hash。序列化时永不外泄。
    #[serde(skip_serializing)]
    pub password_hash: String,
}

/// 媒体库（§9 libraries 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: i64,
    pub name: Option<String>,
    pub path: String,
    /// 库类型（如 anime / movie / tv），v1 先自由字符串。
    pub kind: Option<String>,
    /// 库级选项（JSON），如整理策略开关。
    pub options: Option<serde_json::Value>,
}

/// 物理文件状态（§9 media_files.status）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaFileStatus {
    Pending,
    Matched,
    AiMatched,
    NeedsReview,
    Failed,
    Ignored,
}

/// 物理文件（§9 media_files 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaFile {
    pub id: i64,
    pub library_id: i64,
    /// 一律 '/' 分隔的规范化相对路径；非 UTF-8 路径 lossy 存储。
    pub rel_path: String,
    /// 非 UTF-8 路径的原始字节（对应 raw_path BLOB）。
    pub raw_path: Option<Vec<u8>>,
    pub size: Option<i64>,
    pub mtime: Option<i64>,
    /// sha256(size|mtime)[:16]，仅用于变更检测（是否需重算 hash），
    /// 不作为识别缓存 key（§4.1 L0）。
    pub fingerprint: Option<String>,
    /// 前 16MB MD5（弹弹play hash）；L0 缓存主键 = (size, dandan_hash)。
    pub dandan_hash: Option<String>,
    /// ffprobe 摘要（JSON）。
    pub ffprobe: Option<serde_json::Value>,
    pub status: Option<MediaFileStatus>,
}

/// 逻辑条目类型（§9 items.kind）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Series,
    Season,
    Episode,
    Movie,
}

/// 逻辑条目（§9 items 表，海报墙实体，series→season→episode 树）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    /// series 级冗余，保证中间节点归属明确。
    pub library_id: i64,
    pub kind: ItemKind,
    pub parent_id: Option<i64>,
    pub title: Option<String>,
    pub original_title: Option<String>,
    pub year: Option<i64>,
    pub season_no: Option<i64>,
    pub episode_no: Option<i64>,
    pub overview: Option<String>,
    pub rating: Option<f64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub added_at: Option<i64>,
    /// 软删除：文件消失先标记，宽限期（默认 7 天）后清理（防 NAS 掉线误删）。
    pub deleted_at: Option<i64>,
}

/// 外部 ID 挂载（§9 item_ids 表；§4.5 条目合并的关键约束载体）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemExternalId {
    pub item_id: i64,
    /// tmdb | bangumi | dandanplay | imdb
    pub provider: String,
    pub external_id: String,
}

/// 文件↔条目关联（§9 file_item 表：合集文件与多版本支持）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileItemLink {
    pub file_id: i64,
    pub item_id: i64,
    /// 合集文件时如 "1-2"；常规为 None。
    pub episode_range: Option<String>,
}

/// 刮削任务状态（§9 scrape_tasks.state）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrapeState {
    Queued,
    Running,
    Done,
    NeedsReview,
    Failed,
}

/// 刮削任务（§9 scrape_tasks 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeTask {
    pub id: i64,
    pub file_id: Option<i64>,
    /// 管线层级：l0_cache | l1_hash | l2_agent（§4.1）。
    pub tier: Option<String>,
    pub state: Option<ScrapeState>,
    /// submit_result 的结构化结论（§4.2 schema）。
    pub result: Option<serde_json::Value>,
    /// high | medium | low
    pub confidence: Option<String>,
    /// agent 对话记录。保留策略见 §9 注释（needs_review 与最近 N 条全量，其余摘要）。
    pub transcript: Option<serde_json::Value>,
    pub model: Option<String>,
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    pub created_at: Option<i64>,
}

/// 用户改正记录（§9 scrape_corrections 表；同目录后续文件 few-shot 复用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapeCorrection {
    pub id: i64,
    pub dir_path: Option<String>,
    pub pattern: Option<String>,
    pub item_id: Option<i64>,
}

/// 观看进度（§9 watch_history 表；进度绑定文件——多版本文件时长不同）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchProgress {
    pub user_id: i64,
    pub item_id: i64,
    pub file_id: Option<i64>,
    pub position_ms: Option<i64>,
    pub duration_ms: Option<i64>,
    pub updated_at: Option<i64>,
}

/// 种子投影（§9 torrents 表；librqbit session 为唯一事实源，此表仅投影缓存 §7.1）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentRecord {
    pub id: i64,
    pub info_hash: Option<String>,
    pub name: Option<String>,
    pub state: Option<String>,
    pub save_path: Option<String>,
    pub added_at: Option<i64>,
}

/// RSS 订阅（§9 subscriptions 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subscription {
    pub id: i64,
    pub rss_url: Option<String>,
    pub title: Option<String>,
    /// 过滤规则（字幕组、分辨率、正则排除）。
    pub filters: Option<serde_json::Value>,
    pub enabled: bool,
    pub last_check: Option<i64>,
}
