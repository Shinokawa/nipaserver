//! Provider 层统一错误类型。
//!
//! 工具层映射约定（契约 §3 / 开发文档 §4.2）：
//! - `MissingConfig` → `ToolError::Fatal`（配置缺失，重试无意义）；
//! - 其余（上游 4xx/5xx、超时、解析失败）→ `ToolError::RespondToModel`，
//!   让模型换搜索词/换数据源自行绕路。

/// nipa-providers 内部错误。
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// 必需配置缺失（如 TMDB token 为空）。构造函数返回 Err，不 panic。
    #[error("配置缺失: {0}")]
    MissingConfig(String),

    /// HTTP 客户端初始化失败（理论上不可达）。
    #[error("HTTP 客户端初始化失败: {0}")]
    Init(String),

    /// 上游返回非 2xx。
    #[error("上游返回 HTTP {status}: {message}")]
    UpstreamStatus { status: u16, message: String },

    /// 网络层错误（连接失败/超时/响应体解析失败等）。
    #[error("网络错误: {0}")]
    Network(#[from] reqwest::Error),
}

/// 校验响应状态码并解析 JSON body。
pub(crate) async fn read_json(
    resp: reqwest::Response,
) -> Result<serde_json::Value, ProviderError> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let message: String = body.chars().take(200).collect();
        return Err(ProviderError::UpstreamStatus {
            status: status.as_u16(),
            message,
        });
    }
    Ok(resp.json().await?)
}
