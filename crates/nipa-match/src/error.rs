//! nipa-match 错误类型。
//!
//! 弹弹play 错误约定（调研文档 §2）：业务错误以 200 + `success:false, errorCode,
//! errorMessage` 返回；403 为认证失败，具体原因在响应头 `X-Error-Message`
//! （`Missing Authentication Headers` / `Invalid Timestamp` / `Invalid AppId` /
//! `Invalid Signature` / `Invalid AppSecret`）。

/// nipa-match 内部错误。
#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    /// 无凭证（`DandanAuth::None`）却发起了请求。调用方应先检查
    /// `DandanAuth::is_available`，据此整级跳过 L1（§4.1 降级路径）。
    #[error("弹弹play 无凭证（DandanAuth::None）：应跳过 L1、直接 L2")]
    NoCredentials,

    /// HTTP 客户端初始化失败（理论上不可达）。
    #[error("HTTP 客户端初始化失败: {0}")]
    Init(String),

    /// 403 认证失败。`reason` 取自响应头 `X-Error-Message`，用于诊断
    /// （时间不同步 → Invalid Timestamp；凭证被停用 → Invalid AppId 等）。
    #[error("弹弹play 认证失败 (HTTP 403, X-Error-Message: {reason})")]
    AuthRejected { reason: String },

    /// 业务错误：HTTP 200 但 `success:false`。
    #[error("弹弹play 业务错误 (errorCode={error_code}): {message}")]
    Api { error_code: i32, message: String },

    /// 上游返回其他非 2xx 状态码。
    #[error("弹弹play 返回 HTTP {status}: {message}")]
    UpstreamStatus { status: u16, message: String },

    /// 网络层错误（连接失败/超时/响应体解析失败等）。
    #[error("网络错误: {0}")]
    Network(#[from] reqwest::Error),
}
