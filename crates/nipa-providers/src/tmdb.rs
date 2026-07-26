//! TMDB API v3 客户端（调研 `docs/research/metadata-sources.md` §1）。
//!
//! - 认证：v4 read access token 走 `Authorization: Bearer`（官方推荐，v3 key
//!   走 URL 会留在日志/代理记录中）；
//! - 元数据统一 `language=zh-CN`；
//! - 节流：自限 10 req/s（CDN 层上限 ~50 req/s，留足余量）；
//! - 缓存：搜索 6h、详情/整季 24h（TODO(v1.x) 换 SQLite api_cache，见 cache.rs）。

use std::sync::Arc;
use std::time::Duration;

use crate::cache::{DETAIL_TTL, SEARCH_TTL, TtlCache};
use crate::error::{ProviderError, read_json};
use crate::throttle::Throttle;

/// TMDB 默认 API base（v3）。
pub const DEFAULT_TMDB_BASE_URL: &str = "https://api.themoviedb.org/3";
const TMDB_RATE_PER_SEC: f64 = 10.0;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// TMDB 媒体类型（决定走 /search/tv 还是 /search/movie 等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Tv,
    Movie,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Tv => "tv",
            MediaType::Movie => "movie",
        }
    }
}

/// TMDB HTTP 客户端。经由 `Arc` 在工具间共享（共享节流桶与缓存）。
pub struct TmdbClient {
    http: reqwest::Client,
    base_url: String,
    throttle: Throttle,
    cache: TtlCache,
}

impl TmdbClient {
    /// `token`：TMDB v4 API Read Access Token。为空时返回 Err（不 panic），
    /// 上层据此只注册 Bangumi 工具（无 key 降级，开发文档 §4.3）。
    pub fn new(token: &str, base_url: Option<String>) -> Result<Arc<Self>, ProviderError> {
        let token = token.trim();
        if token.is_empty() {
            return Err(ProviderError::MissingConfig(
                "TMDB API read access token 未配置".into(),
            ));
        }
        let mut headers = reqwest::header::HeaderMap::new();
        let mut auth = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
            .map_err(|_| ProviderError::MissingConfig("TMDB token 含非法字符".into()))?;
        auth.set_sensitive(true);
        headers.insert(reqwest::header::AUTHORIZATION, auth);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| ProviderError::Init(e.to_string()))?;

        Ok(Arc::new(Self {
            http,
            base_url: base_url
                .unwrap_or_else(|| DEFAULT_TMDB_BASE_URL.to_string())
                .trim_end_matches('/')
                .to_string(),
            throttle: Throttle::new(TMDB_RATE_PER_SEC),
            cache: TtlCache::new(),
        }))
    }

    /// 缓存优先的 GET。`cache_key` = 方法 + 参数。
    async fn get_cached(
        &self,
        cache_key: String,
        path: &str,
        query: &[(&str, String)],
        ttl: Duration,
    ) -> Result<serde_json::Value, ProviderError> {
        if let Some(hit) = self.cache.get(&cache_key).await {
            return Ok(hit);
        }
        self.throttle.acquire().await;
        let url = format!("{}{}", self.base_url, path);
        let resp = self.http.get(&url).query(query).send().await?;
        let value = read_json(resp).await?;
        self.cache.put(cache_key, value.clone(), ttl).await;
        Ok(value)
    }

    /// `GET /search/tv?query=&language=zh-CN[&first_air_date_year=]`
    pub async fn search_tv(
        &self,
        query: &str,
        year: Option<i32>,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut params = vec![
            ("query", query.to_string()),
            ("language", "zh-CN".to_string()),
        ];
        if let Some(y) = year {
            params.push(("first_air_date_year", y.to_string()));
        }
        let key = format!("tmdb:search_tv:{query}:{year:?}");
        self.get_cached(key, "/search/tv", &params, SEARCH_TTL)
            .await
    }

    /// `GET /search/movie?query=&language=zh-CN[&year=]`
    pub async fn search_movie(
        &self,
        query: &str,
        year: Option<i32>,
    ) -> Result<serde_json::Value, ProviderError> {
        let mut params = vec![
            ("query", query.to_string()),
            ("language", "zh-CN".to_string()),
        ];
        if let Some(y) = year {
            params.push(("year", y.to_string()));
        }
        let key = format!("tmdb:search_movie:{query}:{year:?}");
        self.get_cached(key, "/search/movie", &params, SEARCH_TTL)
            .await
    }

    /// `GET /tv/{id}?language=zh-CN&append_to_response=external_ids`
    pub async fn tv_detail(&self, id: i64) -> Result<serde_json::Value, ProviderError> {
        self.detail(MediaType::Tv, id).await
    }

    /// `GET /movie/{id}?language=zh-CN&append_to_response=external_ids`
    pub async fn movie_detail(&self, id: i64) -> Result<serde_json::Value, ProviderError> {
        self.detail(MediaType::Movie, id).await
    }

    pub async fn detail(
        &self,
        media_type: MediaType,
        id: i64,
    ) -> Result<serde_json::Value, ProviderError> {
        let params = vec![
            ("language", "zh-CN".to_string()),
            ("append_to_response", "external_ids".to_string()),
        ];
        let kind = media_type.as_str();
        let key = format!("tmdb:detail:{kind}:{id}");
        self.get_cached(key, &format!("/{kind}/{id}"), &params, DETAIL_TTL)
            .await
    }

    /// `GET /tv/{series_id}/season/{season}?language=zh-CN`（整季所有 episode）。
    pub async fn season_episodes(
        &self,
        series_id: i64,
        season: i32,
    ) -> Result<serde_json::Value, ProviderError> {
        let params = vec![("language", "zh-CN".to_string())];
        let key = format!("tmdb:season:{series_id}:{season}");
        self.get_cached(
            key,
            &format!("/tv/{series_id}/season/{season}"),
            &params,
            DETAIL_TTL,
        )
        .await
    }
}
