//! Bangumi API 客户端（api.bgm.tv，调研 `docs/research/metadata-sources.md` §2）。
//!
//! - 公开数据（搜索/条目/章节）无需认证；
//! - **User-Agent 硬性要求**：`AimesSoft/nipaserver/0.1 (https://github.com/AimesSoft/nipaserver)`
//!   （默认 UA 会被封禁，禁止 `Bangumi/1.0`、`database` 之类）；
//! - 节流：自限 1 req/s（官方建议 1-2 req/s）；
//! - 缓存：搜索 6h、详情/章节 24h（TODO(v1.x) 换 SQLite api_cache，见 cache.rs）。

use std::sync::Arc;
use std::time::Duration;

use crate::cache::{DETAIL_TTL, SEARCH_TTL, TtlCache};
use crate::error::{ProviderError, read_json};
use crate::throttle::Throttle;

/// Bangumi 默认 API base。
pub const DEFAULT_BANGUMI_BASE_URL: &str = "https://api.bgm.tv";
/// 默认 User-Agent（Bangumi 硬性要求：开发者 ID + 应用名 + 项目主页）。
pub const DEFAULT_BANGUMI_USER_AGENT: &str =
    "AimesSoft/nipaserver/0.1 (https://github.com/AimesSoft/nipaserver)";
const BANGUMI_RATE_PER_SEC: f64 = 1.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Bangumi HTTP 客户端。经由 `Arc` 在工具间共享（共享节流桶与缓存）。
pub struct BangumiClient {
    http: reqwest::Client,
    base_url: String,
    throttle: Throttle,
    cache: TtlCache,
}

impl BangumiClient {
    /// `base_url` / `user_agent` 传 `None` 用默认值。
    pub fn new(
        base_url: Option<String>,
        user_agent: Option<String>,
    ) -> Result<Arc<Self>, ProviderError> {
        let ua = user_agent.unwrap_or_else(|| DEFAULT_BANGUMI_USER_AGENT.to_string());
        if ua.trim().is_empty() {
            return Err(ProviderError::MissingConfig(
                "Bangumi User-Agent 不得为空（默认 UA 会被封禁）".into(),
            ));
        }
        let http = reqwest::Client::builder()
            .user_agent(ua)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| ProviderError::Init(e.to_string()))?;

        Ok(Arc::new(Self {
            http,
            base_url: base_url
                .unwrap_or_else(|| DEFAULT_BANGUMI_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            throttle: Throttle::new(BANGUMI_RATE_PER_SEC),
            cache: TtlCache::new(),
        }))
    }

    /// `POST /v0/search/subjects`，body `{keyword, filter:{type:[2]}}`（type 2=动画）。
    /// `air_date` 过滤（如 `[">=2020-07-01", "<2020-10-01"]`）用于按播出时间消歧。
    pub async fn search_subjects(
        &self,
        keyword: &str,
        air_date: Option<Vec<String>>,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut filter = serde_json::json!({ "type": [2] });
        if let Some(dates) = &air_date {
            filter["air_date"] = serde_json::json!(dates);
        }
        let body = serde_json::json!({ "keyword": keyword, "filter": filter });

        let key = format!("bgm:search_subjects:{keyword}:{air_date:?}");
        if let Some(hit) = self.cache.get(&key).await {
            return Ok(hit);
        }
        self.throttle.acquire().await;
        let url = format!("{}/v0/search/subjects", self.base_url);
        let resp = self.http.post(&url).json(&body).send().await?;
        let value = read_json(resp).await?;
        self.cache.put(key, value.clone(), SEARCH_TTL).await;
        Ok(value)
    }

    /// `GET /v0/subjects/{id}`（条目详情：name/name_cn/summary/date/...）。
    pub async fn subject_detail(&self, id: i64) -> Result<serde_json::Value, ProviderError> {
        let key = format!("bgm:subject_detail:{id}");
        if let Some(hit) = self.cache.get(&key).await {
            return Ok(hit);
        }
        self.throttle.acquire().await;
        let url = format!("{}/v0/subjects/{id}", self.base_url);
        let resp = self.http.get(&url).send().await?;
        let value = read_json(resp).await?;
        self.cache.put(key, value.clone(), DETAIL_TTL).await;
        Ok(value)
    }

    /// `GET /v0/episodes?subject_id=&type=0`（type 0=本篇；每话含 sort/ep/name/name_cn/airdate）。
    pub async fn subject_episodes(
        &self,
        subject_id: i64,
    ) -> Result<serde_json::Value, ProviderError> {
        let key = format!("bgm:subject_episodes:{subject_id}");
        if let Some(hit) = self.cache.get(&key).await {
            return Ok(hit);
        }
        self.throttle.acquire().await;
        let url = format!("{}/v0/episodes", self.base_url);
        let resp = self
            .http
            .get(&url)
            .query(&[
                ("subject_id", subject_id.to_string()),
                ("type", "0".to_string()),
            ])
            .send()
            .await?;
        let value = read_json(resp).await?;
        self.cache.put(key, value.clone(), DETAIL_TTL).await;
        Ok(value)
    }
}
