//! nipa-download：BT 下载（librqbit）+ Mikan RSS 订阅（开发文档 §7）。
//!
//! 当前为 M0 stub，**刻意不引入 librqbit**（重依赖，M4 接入；
//! 版本须与 NipaPlay 客户端对齐同一大版本，§7.1）。
//!
//! TODO(M4):
//! - librqbit Session（DHT、限速、会话持久化）；session 为唯一事实源，
//!   torrents 表只做投影缓存（启动全量对账重建）；
//! - "下载完成→自动入库"幂等化（按 info_hash + 文件清单判断）；
//! - Mikan RSS：个人聚合 + 单番剧，feed-rs 解析 enclosure，30min 轮询；
//! - 规则过滤（字幕组、分辨率、正则排除）、镜像域名（过 egress 校验）。

use serde::{Deserialize, Serialize};

/// 下载任务状态占位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Downloading,
    Seeding,
    Completed,
    Error,
}

/// 添加下载请求占位。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddDownloadRequest {
    /// magnet 链接或 .torrent URL。
    pub source: String,
    pub save_path: Option<String>,
}

/// 订阅过滤规则占位（同番剧多字幕组按优先级取一，AutoBangumi 思路 §7.2）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubscriptionFilter {
    pub subgroup_priority: Vec<String>,
    pub resolution: Option<String>,
    pub exclude_regex: Option<String>,
}
