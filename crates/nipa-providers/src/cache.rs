//! 进程内响应级 TTL 缓存（开发文档 §5 egress 层）。
//!
//! key = 方法 + 参数字符串；value = 上游 JSON。条目上限 4096，超限逐出最旧
//! （按写入时间）。搜索类 TTL 6h、详情类 24h（常量见 [`SEARCH_TTL`] / [`DETAIL_TTL`]）。
//!
//! TODO(v1.x): 换成 SQLite `api_cache` 表（响应级缓存 + TTL，跨进程重启保留，
//! 开发文档 §5/§9），本模块保留为其内存前置层。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// 搜索类响应 TTL：6 小时。
pub const SEARCH_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// 详情/章节类响应 TTL：24 小时。
pub const DETAIL_TTL: Duration = Duration::from_secs(24 * 60 * 60);
/// 条目上限，超限逐出最旧条目。
const MAX_ENTRIES: usize = 4096;

struct Entry {
    value: serde_json::Value,
    inserted_at: Instant,
    ttl: Duration,
}

/// 进程内 TTL 缓存。
#[derive(Default)]
pub struct TtlCache {
    map: Mutex<HashMap<String, Entry>>,
}

impl TtlCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 命中且未过期返回克隆值；过期条目顺手移除。
    pub async fn get(&self, key: &str) -> Option<serde_json::Value> {
        let mut map = self.map.lock().await;
        match map.get(key) {
            Some(e) if e.inserted_at.elapsed() < e.ttl => Some(e.value.clone()),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    pub async fn put(&self, key: String, value: serde_json::Value, ttl: Duration) {
        let mut map = self.map.lock().await;
        if map.len() >= MAX_ENTRIES && !map.contains_key(&key) {
            // 先清一轮过期条目；仍超限则逐出写入时间最旧的一条。
            map.retain(|_, e| e.inserted_at.elapsed() < e.ttl);
            if map.len() >= MAX_ENTRIES
                && let Some(oldest) = map
                    .iter()
                    .min_by_key(|(_, e)| e.inserted_at)
                    .map(|(k, _)| k.clone())
            {
                map.remove(&oldest);
            }
        }
        map.insert(
            key,
            Entry {
                value,
                inserted_at: Instant::now(),
                ttl,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn get_returns_cached_value_within_ttl() {
        let c = TtlCache::new();
        c.put("k".into(), serde_json::json!(1), Duration::from_secs(60))
            .await;
        assert_eq!(c.get("k").await, Some(serde_json::json!(1)));
    }

    #[tokio::test]
    async fn expired_entry_is_missed_and_removed() {
        let c = TtlCache::new();
        c.put("k".into(), serde_json::json!(1), Duration::ZERO).await;
        assert_eq!(c.get("k").await, None);
        assert!(c.map.lock().await.is_empty());
    }

    #[tokio::test]
    async fn eviction_keeps_map_bounded() {
        let c = TtlCache::new();
        for i in 0..(MAX_ENTRIES + 8) {
            c.put(
                format!("k{i}"),
                serde_json::json!(i),
                Duration::from_secs(60),
            )
            .await;
        }
        assert!(c.map.lock().await.len() <= MAX_ENTRIES);
    }
}
