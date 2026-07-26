//! nipa-download：BT 下载（librqbit）+ Mikan RSS 订阅（开发文档 §7）。
//!
//! librqbit Session 是唯一事实源；server 层的 `torrents` 表只是可重建投影。
//! 本 crate 不依赖 axum/sqlx，以便未来回流 NipaPlay 客户端。

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Session, SessionOptions,
    SessionPersistenceConfig, TorrentStatsState,
};
use regex::Regex;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::{Host, Url};

const MAX_TORRENT_BYTES: usize = 8 * 1024 * 1024;
const MAX_FEED_BYTES: usize = 2 * 1024 * 1024;

/// 下载任务状态占位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Queued,
    Downloading,
    Paused,
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

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("invalid source: {0}")]
    InvalidSource(String),
    #[error("unsafe outbound URL: {0}")]
    UnsafeUrl(String),
    #[error("response exceeds {0} bytes")]
    ResponseTooLarge(usize),
    #[error("torrent engine error: {0}")]
    Torrent(#[source] anyhow::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("feed parse error: {0}")]
    Feed(String),
    #[error("invalid filter regex: {0}")]
    FilterRegex(#[from] regex::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// 一条 Session 事实快照。`session_id` 只作运行时诊断，API 以 info_hash 定位。
#[derive(Debug, Clone, Serialize)]
pub struct DownloadSnapshot {
    pub session_id: usize,
    pub info_hash: String,
    pub name: String,
    pub state: DownloadState,
    pub progress_bytes: u64,
    pub total_bytes: u64,
    pub uploaded_bytes: u64,
    pub error: Option<String>,
    /// 稳定地描述完成文件集，用于自动入库幂等键。
    pub manifest_hash: Option<String>,
}

/// BT 下载服务。会话持久化与下载内容都放在 server data_dir 中。
pub struct DownloadService {
    session: Arc<Session>,
    output_dir: PathBuf,
}

impl DownloadService {
    pub async fn start(data_dir: &Path) -> Result<Arc<Self>, DownloadError> {
        let output_dir = data_dir.join("downloads");
        let persistence_dir = data_dir.join("rqbit-session");
        std::fs::create_dir_all(&output_dir)?;
        std::fs::create_dir_all(&persistence_dir)?;
        let opts = SessionOptions {
            fastresume: true,
            persistence: Some(SessionPersistenceConfig::Json {
                folder: Some(persistence_dir),
            }),
            ..Default::default()
        };
        let session = Session::new_with_opts(output_dir.clone(), opts)
            .await
            .map_err(DownloadError::Torrent)?;
        Ok(Arc::new(Self {
            session,
            output_dir,
        }))
    }

    pub fn output_dir(&self) -> &Path {
        &self.output_dir
    }

    pub fn list(&self) -> Vec<DownloadSnapshot> {
        self.session
            .with_torrents(|iter| iter.map(|(id, handle)| snapshot(id, handle)).collect())
    }

    pub fn get(&self, info_hash: &str) -> Option<DownloadSnapshot> {
        self.list().into_iter().find(|t| t.info_hash == info_hash)
    }

    pub async fn add(&self, req: AddDownloadRequest) -> Result<DownloadSnapshot, DownloadError> {
        let source = req.source.trim();
        let add = if source.starts_with("magnet:") {
            AddTorrent::from_url(source.to_owned())
        } else {
            let target = resolve_public_http_url(source).await?;
            let bytes = bounded_get(target, MAX_TORRENT_BYTES).await?;
            AddTorrent::from_bytes(bytes)
        };
        let output_folder = match req.save_path {
            Some(path) => Some(self.validate_save_path(&path)?),
            None => None,
        };
        let response = self
            .session
            .add_torrent(
                add,
                Some(AddTorrentOptions {
                    output_folder,
                    overwrite: false,
                    ..Default::default()
                }),
            )
            .await
            .map_err(DownloadError::Torrent)?;
        let handle = match response {
            AddTorrentResponse::Added(_, h) | AddTorrentResponse::AlreadyManaged(_, h) => h,
            AddTorrentResponse::ListOnly(_) => {
                return Err(DownloadError::InvalidSource(
                    "unexpected list-only response".into(),
                ));
            }
        };
        Ok(snapshot(handle.id(), &handle))
    }

    fn validate_save_path(&self, value: &str) -> Result<String, DownloadError> {
        let candidate = Path::new(value)
            .canonicalize()
            .map_err(|e| DownloadError::InvalidSource(format!("save_path: {e}")))?;
        let root = self.output_dir.canonicalize()?;
        if !candidate.is_dir() || !candidate.starts_with(&root) {
            return Err(DownloadError::InvalidSource(
                "save_path must be an existing directory under the download root".into(),
            ));
        }
        Ok(candidate.to_string_lossy().into_owned())
    }

    pub async fn pause(&self, info_hash: &str) -> Result<DownloadSnapshot, DownloadError> {
        let handle = self.handle(info_hash)?;
        self.session
            .pause(&handle)
            .await
            .map_err(DownloadError::Torrent)?;
        Ok(snapshot(handle.id(), &handle))
    }

    pub async fn resume(&self, info_hash: &str) -> Result<DownloadSnapshot, DownloadError> {
        let handle = self.handle(info_hash)?;
        self.session
            .unpause(&handle)
            .await
            .map_err(DownloadError::Torrent)?;
        Ok(snapshot(handle.id(), &handle))
    }

    pub async fn delete(&self, info_hash: &str, delete_files: bool) -> Result<(), DownloadError> {
        let parsed = librqbit::api::TorrentIdOrHash::try_from(info_hash)
            .map_err(|e| DownloadError::InvalidSource(e.to_string()))?;
        self.session
            .delete(parsed, delete_files)
            .await
            .map_err(DownloadError::Torrent)
    }

    fn handle(&self, info_hash: &str) -> Result<Arc<librqbit::ManagedTorrent>, DownloadError> {
        let parsed = librqbit::api::TorrentIdOrHash::try_from(info_hash)
            .map_err(|e| DownloadError::InvalidSource(e.to_string()))?;
        self.session
            .get(parsed)
            .ok_or_else(|| DownloadError::InvalidSource("torrent not found".into()))
    }

    pub async fn fetch_feed(&self, url: &str) -> Result<Vec<FeedItem>, DownloadError> {
        let target = resolve_public_http_url(url).await?;
        let body = bounded_get(target, MAX_FEED_BYTES).await?;
        parse_feed(&body)
    }

    pub async fn stop(&self) {
        self.session.stop().await;
    }
}

fn snapshot(id: usize, handle: &Arc<librqbit::ManagedTorrent>) -> DownloadSnapshot {
    let stats = handle.stats();
    let state = if stats.finished {
        if matches!(stats.state, TorrentStatsState::Live) {
            DownloadState::Seeding
        } else {
            DownloadState::Completed
        }
    } else {
        match stats.state {
            TorrentStatsState::Initializing => DownloadState::Queued,
            TorrentStatsState::Live => DownloadState::Downloading,
            TorrentStatsState::Paused => DownloadState::Paused,
            TorrentStatsState::Error => DownloadState::Error,
        }
    };
    let manifest_hash = stats.finished.then(|| {
        handle
            .with_metadata(|metadata| {
                let mut files: Vec<_> = metadata
                    .file_infos
                    .iter()
                    .map(|f| (f.relative_filename.to_string_lossy().into_owned(), f.len))
                    .collect();
                files.sort();
                let mut hasher = Sha256::new();
                for (path, len) in files {
                    hasher.update(path.as_bytes());
                    hasher.update([0]);
                    hasher.update(len.to_le_bytes());
                }
                hex::encode(hasher.finalize())
            })
            .unwrap_or_default()
    });
    DownloadSnapshot {
        session_id: id,
        info_hash: handle.info_hash().as_string(),
        name: handle
            .name()
            .unwrap_or_else(|| handle.info_hash().as_string()),
        state,
        progress_bytes: stats.progress_bytes,
        total_bytes: stats.total_bytes,
        uploaded_bytes: stats.uploaded_bytes,
        error: stats.error,
        manifest_hash,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedItem {
    pub entry_key: String,
    pub title: String,
    pub enclosure_url: String,
}

pub fn parse_feed(bytes: &[u8]) -> Result<Vec<FeedItem>, DownloadError> {
    let feed = feed_rs::parser::parse(bytes).map_err(|e| DownloadError::Feed(e.to_string()))?;
    let mut result = Vec::new();
    for entry in feed.entries {
        let title = entry
            .title
            .as_ref()
            .map(|t| t.content.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| entry.id.clone());
        let enclosure = entry
            .media
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|c| c.url.as_ref())
            .map(ToString::to_string)
            .chain(
                entry
                    .links
                    .iter()
                    .filter(|l| l.rel.as_deref() == Some("enclosure"))
                    .map(|l| l.href.clone()),
            )
            .find(|u| {
                u.starts_with("magnet:") || u.starts_with("http://") || u.starts_with("https://")
            });
        if let Some(enclosure_url) = enclosure {
            let entry_key = if entry.id.trim().is_empty() {
                enclosure_url.clone()
            } else {
                entry.id
            };
            result.push(FeedItem {
                entry_key,
                title,
                enclosure_url,
            });
        }
    }
    Ok(result)
}

/// 过滤并在同一集的多字幕组候选中选优先级最高的一个。
pub fn select_feed_items(
    items: Vec<FeedItem>,
    filter: &SubscriptionFilter,
) -> Result<Vec<FeedItem>, DownloadError> {
    let exclude = filter
        .exclude_regex
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(Regex::new)
        .transpose()?;
    let resolution = filter.resolution.as_deref().map(str::to_lowercase);
    let mut groups: BTreeMap<String, (usize, FeedItem)> = BTreeMap::new();
    for item in items {
        let lower = item.title.to_lowercase();
        if exclude.as_ref().is_some_and(|r| r.is_match(&item.title))
            || resolution.as_ref().is_some_and(|r| !lower.contains(r))
        {
            continue;
        }
        let priority = filter
            .subgroup_priority
            .iter()
            .position(|g| lower.contains(&g.to_lowercase()))
            .unwrap_or(filter.subgroup_priority.len());
        // 去掉常见 [字幕组]/[分辨率] 块，保留剧名与集数作分组键。
        let key = group_key(&item.title, filter);
        match groups.get(&key) {
            Some((old_priority, _)) if *old_priority <= priority => {}
            _ => {
                groups.insert(key, (priority, item));
            }
        }
    }
    Ok(groups.into_values().map(|(_, item)| item).collect())
}

fn group_key(title: &str, filter: &SubscriptionFilter) -> String {
    let mut key = title.to_lowercase();
    for group in &filter.subgroup_priority {
        key = key.replace(&group.to_lowercase(), "");
    }
    for token in ["1080p", "2160p", "720p", "4k", "torrent"] {
        key = key.replace(token, "");
    }
    key.chars()
        .filter(|c| !matches!(c, '[' | ']' | '(' | ')' | '【' | '】') && !c.is_whitespace())
        .collect()
}

struct ValidatedTarget {
    url: Url,
    /// 域名与已完成安全检查的解析结果。请求时固定，避免 DNS rebinding/TOCTOU。
    pinned: Option<(String, Vec<std::net::SocketAddr>)>,
}

async fn bounded_get(target: ValidatedTarget, max_bytes: usize) -> Result<Vec<u8>, DownloadError> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(Policy::none())
        .user_agent("NipaServer/0.1");
    if let Some((domain, addrs)) = &target.pinned {
        builder = builder.resolve_to_addrs(domain, addrs);
    }
    let client = builder.build()?;
    let mut response = client.get(target.url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|len| len > max_bytes as u64)
    {
        return Err(DownloadError::ResponseTooLarge(max_bytes));
    }
    let mut result = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if result.len().saturating_add(chunk.len()) > max_bytes {
            return Err(DownloadError::ResponseTooLarge(max_bytes));
        }
        result.extend_from_slice(&chunk);
    }
    Ok(result)
}

/// 校验 RSS / .torrent 的服务器端出站目标。禁止私网、环回、链路本地与无跳转。
pub async fn validate_public_http_url(input: &str) -> Result<Url, DownloadError> {
    resolve_public_http_url(input)
        .await
        .map(|target| target.url)
}

async fn resolve_public_http_url(input: &str) -> Result<ValidatedTarget, DownloadError> {
    let url = Url::parse(input).map_err(|e| DownloadError::InvalidSource(e.to_string()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(DownloadError::UnsafeUrl(input.to_string()));
    }
    let host = url
        .host()
        .ok_or_else(|| DownloadError::UnsafeUrl(input.to_string()))?;
    let mut pinned = None;
    match host {
        Host::Ipv4(ip) if !is_public_ip(IpAddr::V4(ip)) => {
            return Err(DownloadError::UnsafeUrl(input.to_string()));
        }
        Host::Ipv6(ip) if !is_public_ip(IpAddr::V6(ip)) => {
            return Err(DownloadError::UnsafeUrl(input.to_string()));
        }
        Host::Domain(domain) => {
            if domain.eq_ignore_ascii_case("localhost") || domain.ends_with(".localhost") {
                return Err(DownloadError::UnsafeUrl(input.to_string()));
            }
            let port = url.port_or_known_default().unwrap_or(443);
            let addrs: Vec<_> = tokio::net::lookup_host((domain, port))
                .await
                .map_err(|e| DownloadError::UnsafeUrl(e.to_string()))?
                .collect();
            if addrs.is_empty() || addrs.iter().any(|a| !is_public_ip(a.ip())) {
                return Err(DownloadError::UnsafeUrl(input.to_string()));
            }
            pinned = Some((domain.to_string(), addrs));
        }
        _ => {}
    }
    Ok(ValidatedTarget { url, pinned })
}

fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_multicast()
        || ip.is_unspecified()
        || ip == Ipv4Addr::BROADCAST
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b))
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && matches!(b, 18 | 19))
        || (a == 198 && b == 51 && ip.octets()[2] == 100)
        || (a == 203 && b == 0 && ip.octets()[2] == 113))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    if let Some(v4) = ip.to_ipv4_mapped() {
        return is_public_v4(v4);
    }
    !(ip.is_unspecified()
        || ip.is_loopback()
        || ip.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] == 0x2001 && segments[1] == 0x0db8))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mikan_enclosure() {
        let xml = br#"<?xml version="1.0"?><rss version="2.0"><channel><title>x</title>
          <item><guid>episode-1</guid><title>[A] Show 01 [1080P]</title>
          <enclosure url="https://example.com/one.torrent" type="application/x-bittorrent" length="3"/></item>
          </channel></rss>"#;
        let items = parse_feed(xml).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].entry_key, "episode-1");
        assert_eq!(items[0].enclosure_url, "https://example.com/one.torrent");
    }

    #[test]
    fn filters_resolution_exclusion_and_subgroup_priority() {
        let items = vec![
            FeedItem {
                entry_key: "a".into(),
                title: "[Low] Show 01 [1080P]".into(),
                enclosure_url: "magnet:?xt=a".into(),
            },
            FeedItem {
                entry_key: "b".into(),
                title: "[Best] Show 01 [1080P]".into(),
                enclosure_url: "magnet:?xt=b".into(),
            },
            FeedItem {
                entry_key: "c".into(),
                title: "[Best] Show 02 [720P]".into(),
                enclosure_url: "magnet:?xt=c".into(),
            },
            FeedItem {
                entry_key: "d".into(),
                title: "[Best] Show 03 [1080P] CHS".into(),
                enclosure_url: "magnet:?xt=d".into(),
            },
        ];
        let filter = SubscriptionFilter {
            subgroup_priority: vec!["Best".into(), "Low".into()],
            resolution: Some("1080p".into()),
            exclude_regex: Some("CHS$".into()),
        };
        let selected = select_feed_items(items, &filter).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].entry_key, "b");
    }

    #[tokio::test]
    async fn rejects_private_and_non_http_urls() {
        assert!(
            validate_public_http_url("http://127.0.0.1/a")
                .await
                .is_err()
        );
        assert!(validate_public_http_url("http://[::1]/a").await.is_err());
        assert!(
            validate_public_http_url("file:///etc/passwd")
                .await
                .is_err()
        );
        assert!(
            validate_public_http_url("http://user:pass@example.com/a")
                .await
                .is_err()
        );
    }
}
