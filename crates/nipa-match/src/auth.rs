//! 弹弹play 开放平台认证（§4.1；调研文档 §2）。
//!
//! 两种认证模式任选其一，所有请求都需携带认证头；无凭证时开放平台返回 403
//! （`X-Error-Message: Missing Authentication Headers`）。

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

/// 弹弹play 认证模式（§4.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum DandanAuth {
    /// 凭证模式（官方标注适合服务器端）：请求头 `X-AppId` + `X-AppSecret`。
    Credentials { app_id: String, app_secret: String },
    /// 签名模式（官方推荐客户端用）：请求头 `X-AppId` + `X-Timestamp` + `X-Signature`。
    ///
    /// `X-Signature = base64(sha256(AppId + Timestamp + Path + AppSecret))`，
    /// SHA-256 取**原始字节**做 Base64（非 hex）；Timestamp 为 UTC Unix 秒；
    /// Path 以 `/` 开头、不含协议/域名/query。
    Signature { app_id: String, app_secret: String },
    /// 无凭证：调用方应据此**整级跳过 L1、直接走 L2**（§4.1 降级路径，M1 验收场景）。
    None,
}

impl DandanAuth {
    /// 是否具备可用凭证。`false` 时上层应跳过 L1。
    pub fn is_available(&self) -> bool {
        !matches!(self, DandanAuth::None)
    }

    /// 为 `path`（以 `/` 开头、不含 query）生成认证头列表 `(header_name, value)`。
    ///
    /// `DandanAuth::None` 返回空列表（调用方通常不应走到这里，见 [`Self::is_available`]）。
    pub(crate) fn headers(&self, path: &str) -> Vec<(&'static str, String)> {
        match self {
            DandanAuth::Credentials { app_id, app_secret } => vec![
                ("X-AppId", app_id.clone()),
                ("X-AppSecret", app_secret.clone()),
            ],
            DandanAuth::Signature { app_id, app_secret } => {
                let timestamp = unix_now();
                let signature = compute_signature(app_id, timestamp, path, app_secret);
                vec![
                    ("X-AppId", app_id.clone()),
                    ("X-Timestamp", timestamp.to_string()),
                    ("X-Signature", signature),
                ]
            }
            DandanAuth::None => Vec::new(),
        }
    }
}

/// 当前 UTC Unix 时间戳（秒）。
fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 签名算法（调研文档 §2 原文）：
/// `base64( sha256( AppId + Timestamp + Path + AppSecret ) )`。
///
/// 四段按顺序直接字符串拼接（区分大小写）；SHA-256 结果取**原始字节**做标准
/// Base64（含 padding），不是 hex 字符串。`timestamp` 为 UTC Unix 秒；
/// `path` 以 `/` 开头，不含协议、域名与 `?` 查询参数。
pub fn compute_signature(app_id: &str, timestamp: i64, path: &str, app_secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(app_id.as_bytes());
    hasher.update(timestamp.to_string().as_bytes());
    hasher.update(path.as_bytes());
    hasher.update(app_secret.as_bytes());
    BASE64.encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    // 期望值由独立实现生成并交叉验证：
    //   python3: base64.b64encode(hashlib.sha256((app_id+str(ts)+path+secret).encode()).digest())
    //   shell:   printf '%s' "..." | shasum -a 256 | xxd -r -p | base64
    // 两者一致后写死于此。

    #[test]
    fn signature_known_vector_match_path() {
        assert_eq!(
            compute_signature("nipaAppId", 1_727_000_000, "/api/v2/match", "nipaAppSecret"),
            "Vp/bqjvMHAd4FVI7bUHaLIZkOX7rW0Nwy2mj1buiDBA="
        );
    }

    #[test]
    fn signature_known_vector_comment_path_with_id() {
        // Path 含具体数字 ID、不含 query（调研文档 §3 注意事项）。
        assert_eq!(
            compute_signature(
                "testId",
                1_700_000_000,
                "/api/v2/comment/123450001",
                "testSecret"
            ),
            "nImiZ5BC+SZhQ8+GHKOipD8RaXx417quuS3F7IyDKUs="
        );
    }

    #[test]
    fn signature_known_vector_minimal() {
        assert_eq!(
            compute_signature("a", 1, "/p", "s"),
            "Vo5gWecCZr0jhh9wAegq8g4eGTQQY41T3Sgt3ac35l8="
        );
    }

    #[test]
    fn signature_is_case_sensitive_and_order_sensitive() {
        let base = compute_signature("a", 1, "/p", "s");
        assert_ne!(base, compute_signature("A", 1, "/p", "s"));
        assert_ne!(base, compute_signature("s", 1, "/p", "a"));
    }

    #[test]
    fn credentials_headers() {
        let auth = DandanAuth::Credentials {
            app_id: "id".into(),
            app_secret: "sec".into(),
        };
        assert_eq!(
            auth.headers("/api/v2/match"),
            vec![
                ("X-AppId", "id".to_string()),
                ("X-AppSecret", "sec".to_string()),
            ]
        );
    }

    #[test]
    fn signature_headers_consistent_with_algorithm() {
        let auth = DandanAuth::Signature {
            app_id: "id".into(),
            app_secret: "sec".into(),
        };
        let headers = auth.headers("/api/v2/match");
        assert_eq!(headers.len(), 3);
        assert_eq!(headers[0], ("X-AppId", "id".to_string()));
        let ts: i64 = headers[1].1.parse().expect("X-Timestamp 应为整数秒");
        assert_eq!(
            headers[2].1,
            compute_signature("id", ts, "/api/v2/match", "sec")
        );
    }

    #[test]
    fn none_yields_no_headers_and_unavailable() {
        assert!(DandanAuth::None.headers("/api/v2/match").is_empty());
        assert!(!DandanAuth::None.is_available());
        assert!(
            DandanAuth::Credentials {
                app_id: "i".into(),
                app_secret: "s".into()
            }
            .is_available()
        );
    }
}
