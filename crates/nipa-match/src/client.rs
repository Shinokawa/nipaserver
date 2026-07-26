//! 弹弹play match 客户端（`POST /api/v2/match` / `POST /api/v2/match/batch`）。
//!
//! 合规（§4.1 / 调研文档 §4）：按需调用、避免高频——内置最小请求间隔节流
//! （默认 300ms/请求）；禁止批量抓取/下载数据库，batch 仅服务于用户媒体库扫描。

use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::auth::DandanAuth;
use crate::error::MatchError;
use crate::types::{BatchItem, BatchMatchRequestBody, BatchMatchResponse, MatchRequest, MatchResponse};

/// 官方 API 基础地址。
pub const DEFAULT_BASE_URL: &str = "https://api.dandanplay.net";
/// batch 单次上限（服务器约束：requests 最多 32 个）。
pub const BATCH_LIMIT: usize = 32;
/// 默认最小请求间隔。
pub const DEFAULT_MIN_INTERVAL: Duration = Duration::from_millis(300);

const MATCH_PATH: &str = "/api/v2/match";
const BATCH_PATH: &str = "/api/v2/match/batch";
const DEFAULT_USER_AGENT: &str = concat!("nipaserver/", env!("CARGO_PKG_VERSION"));

/// 弹弹play 文件识别客户端。
pub struct DandanClient {
    http: reqwest::Client,
    base_url: String,
    auth: DandanAuth,
    min_interval: Duration,
    /// 上次请求发出时刻；简单 Mutex<Instant> 节流（§4.1 按需调用约定）。
    last_request: Mutex<Option<Instant>>,
}

impl DandanClient {
    /// 以默认配置构造（官方地址、默认 UA、300ms 最小间隔）。
    ///
    /// 注意：`DandanAuth::None` 时客户端可构造，但任何请求都会返回
    /// [`MatchError::NoCredentials`]——调用方应先用 [`DandanAuth::is_available`]
    /// 判断并整级跳过 L1（§4.1 降级路径）。
    pub fn new(auth: DandanAuth) -> Result<Self, MatchError> {
        Self::builder(auth).build()
    }

    /// 进入构造器自定义 base_url / UA / 最小间隔。
    pub fn builder(auth: DandanAuth) -> DandanClientBuilder {
        DandanClientBuilder {
            auth,
            base_url: DEFAULT_BASE_URL.to_string(),
            user_agent: DEFAULT_USER_AGENT.to_string(),
            min_interval: DEFAULT_MIN_INTERVAL,
            timeout: Duration::from_secs(15),
        }
    }

    /// `POST /api/v2/match`：单文件识别（`matchMode` 由 `req` 决定，
    /// [`MatchRequest::new`] 默认 `hashAndFileName`）。
    ///
    /// 返回原始响应（已剔除业务错误）；用 [`crate::classify`] 归入三分类。
    pub async fn match_file(&self, req: MatchRequest) -> Result<MatchResponse, MatchError> {
        let resp: MatchResponse = self.post_json(MATCH_PATH, &req).await?;
        if !resp.success {
            return Err(MatchError::Api {
                error_code: resp.error_code,
                message: resp.error_message.unwrap_or_default(),
            });
        }
        Ok(resp)
    }

    /// `POST /api/v2/match/batch`：批量识别，只回精确命中。
    ///
    /// 单次上限 32 个；超出时内部按 32 分批**串行**请求（受同一节流约束）。
    /// 返回结果与 `reqs` 一一对应：未精确命中项 `success=false`、`match_result=None`。
    pub async fn match_batch(&self, reqs: Vec<MatchRequest>) -> Result<Vec<BatchItem>, MatchError> {
        // 空输入也先检查凭证：无凭证时统一告知调用方跳过 L1。
        if !self.auth.is_available() {
            return Err(MatchError::NoCredentials);
        }
        let mut items = Vec::with_capacity(reqs.len());
        for chunk in reqs.chunks(BATCH_LIMIT) {
            let body = BatchMatchRequestBody { requests: chunk };
            let resp: BatchMatchResponse = self.post_json(BATCH_PATH, &body).await?;
            if !resp.success {
                return Err(MatchError::Api {
                    error_code: resp.error_code,
                    message: resp.error_message.unwrap_or_default(),
                });
            }
            items.extend(resp.results);
        }
        Ok(items)
    }

    /// 发送 POST 并按弹弹play 错误约定处理响应。
    async fn post_json<B, T>(&self, path: &str, body: &B) -> Result<T, MatchError>
    where
        B: serde::Serialize + ?Sized,
        T: serde::de::DeserializeOwned,
    {
        if !self.auth.is_available() {
            return Err(MatchError::NoCredentials);
        }
        self.throttle().await;

        let mut builder = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .json(body);
        // 签名 Path 即 API 路径本身（以 / 开头、不含 query），与 base_url 无关。
        for (name, value) in self.auth.headers(path) {
            builder = builder.header(name, value);
        }
        let resp = builder.send().await?;

        let status = resp.status();
        if status == reqwest::StatusCode::FORBIDDEN {
            // 具体原因在 X-Error-Message 头（Missing Authentication Headers /
            // Invalid Timestamp / Invalid AppId / Invalid Signature / Invalid AppSecret）。
            let reason = resp
                .headers()
                .get("X-Error-Message")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("(无 X-Error-Message 头)")
                .to_string();
            return Err(MatchError::AuthRejected { reason });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let message: String = body.chars().take(200).collect();
            return Err(MatchError::UpstreamStatus {
                status: status.as_u16(),
                message,
            });
        }
        Ok(resp.json().await?)
    }

    /// 最小间隔节流：距上次请求不足 `min_interval` 时 sleep 补足。
    ///
    /// 持锁至请求间隔确认完成，天然串行化并发调用方（本 crate 请求量极小，
    /// 不需要 nipa-providers 那样的令牌桶突发容量）。
    async fn throttle(&self) {
        if self.min_interval.is_zero() {
            return;
        }
        let mut last = self.last_request.lock().await;
        if let Some(prev) = *last {
            let next_allowed = prev + self.min_interval;
            let now = Instant::now();
            if next_allowed > now {
                tokio::time::sleep_until(next_allowed).await;
            }
        }
        *last = Some(Instant::now());
    }
}

/// [`DandanClient`] 构造器。
pub struct DandanClientBuilder {
    auth: DandanAuth,
    base_url: String,
    user_agent: String,
    min_interval: Duration,
    timeout: Duration,
}

impl DandanClientBuilder {
    /// 覆盖 API 基础地址（测试注入 wiremock；末尾 `/` 会被剔除）。
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        let url = url.into();
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    /// 覆盖 User-Agent（默认 `nipaserver/<version>`）。
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// 覆盖最小请求间隔（`Duration::ZERO` 关闭节流；默认 300ms）。
    pub fn min_interval(mut self, interval: Duration) -> Self {
        self.min_interval = interval;
        self
    }

    /// 覆盖整体请求超时（默认 15s）。
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn build(self) -> Result<DandanClient, MatchError> {
        let http = reqwest::Client::builder()
            .user_agent(self.user_agent)
            .timeout(self.timeout)
            .build()
            .map_err(|e| MatchError::Init(e.to_string()))?;
        Ok(DandanClient {
            http,
            base_url: self.base_url,
            auth: self.auth,
            min_interval: self.min_interval,
            last_request: Mutex::new(None),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn none_auth_short_circuits_without_network() {
        // base_url 指向不可达地址：若未短路会得到 Network 错误而非 NoCredentials。
        let client = DandanClient::builder(DandanAuth::None)
            .base_url("http://127.0.0.1:9")
            .min_interval(Duration::ZERO)
            .build()
            .unwrap();
        let err = client
            .match_file(MatchRequest::new("f", "0", 1))
            .await
            .unwrap_err();
        assert!(matches!(err, MatchError::NoCredentials), "实为 {err:?}");
        let err = client.match_batch(vec![]).await.unwrap_err();
        assert!(matches!(err, MatchError::NoCredentials), "实为 {err:?}");
    }

    #[tokio::test]
    async fn throttle_enforces_min_interval() {
        let client = DandanClient::builder(DandanAuth::None)
            .min_interval(Duration::from_millis(50))
            .build()
            .unwrap();
        let start = Instant::now();
        for _ in 0..3 {
            client.throttle().await;
        }
        // 第 2、3 次各需等 50ms。
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    #[tokio::test]
    async fn zero_interval_disables_throttle() {
        let client = DandanClient::builder(DandanAuth::None)
            .min_interval(Duration::ZERO)
            .build()
            .unwrap();
        let start = Instant::now();
        for _ in 0..10 {
            client.throttle().await;
        }
        assert!(start.elapsed() < Duration::from_millis(50));
    }
}
