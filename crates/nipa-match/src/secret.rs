//! AppSecret 分发（开发文档 §4.1：复用 NipaPlay 既有分发基础设施）。
//!
//! secret 不在二进制里：启动时从分发服务器拉取混淆的 `encryptedAppSecret`，
//! 本地做字符变换还原。还原算法与客户端 `dandanplay_service_io.dart` 的
//! `_b()` 逐步对齐（四步：大小写镜像 → 首字符移位 → 数字取补 → 大小写互换）。

use serde::Deserialize;

/// 分发服务器（与 NipaPlay 客户端一致）。
pub const SECRET_SERVERS: &[&str] = &[
    "https://nipaplay.aimes-soft.com",
    "https://kurisu.aimes-soft.com",
];

/// 与客户端一致的 AppId。
pub const NIPA_APP_ID: &str = "nipaplayv1";

#[derive(Debug, Deserialize)]
struct SecretResponse {
    #[serde(rename = "encryptedAppSecret")]
    encrypted_app_secret: Option<String>,
}

/// 还原混淆的 secret（对齐客户端 `_b()`）。
///
/// 1. 字母表镜像（保持大小写：A↔Z, b↔y…）；
/// 2. 长度 ≥5 时首字符移动到倒数第 4 位之前；
/// 3. 数字 d → (10 - d) 的字符（d=1..9 映射 9..1；实际 secret 不含 0，
///    与客户端行为逐字节一致即可）；
/// 4. 大小写互换。
pub fn decode_secret(encrypted: &str) -> String {
    // 1) 字母表镜像
    let mirrored: String = encrypted
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                (b'A' + 25 - (c as u8 - b'A')) as char
            } else if c.is_ascii_lowercase() {
                (b'a' + 25 - (c as u8 - b'a')) as char
            } else {
                c
            }
        })
        .collect();
    // 2) 首字符移位（与 Dart substring 逻辑一致）
    let shifted: String = {
        let chars: Vec<char> = mirrored.chars().collect();
        if chars.len() >= 5 {
            let first = chars[0];
            let mid: String = chars[1..chars.len() - 4].iter().collect();
            let tail: String = chars[chars.len() - 4..].iter().collect();
            format!("{mid}{first}{tail}")
        } else {
            mirrored
        }
    };
    // 3) 数字取补：d → char('0' + 10 - d)
    let digits: String = shifted
        .chars()
        .map(|c| {
            if c.is_ascii_digit() {
                let d = c as u8 - b'0';
                (b'0' + (10 - d)) as char
            } else {
                c
            }
        })
        .collect();
    // 4) 大小写互换
    digits
        .chars()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                c
            }
        })
        .collect()
}

/// 从分发服务器拉取并还原 appSecret。逐服务器降级尝试；
/// 全部失败返回 None（调用方按 L1 降级路径处理——跳过 hash 匹配）。
pub async fn fetch_app_secret(user_agent: &str) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(8))
        .build()
        .ok()?;
    for server in SECRET_SERVERS {
        let url = format!("{server}/nipaplay.php");
        match client
            .get(&url)
            .header(reqwest::header::USER_AGENT, user_agent)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<SecretResponse>().await
                    && let Some(enc) = body.encrypted_app_secret
                {
                    let secret = decode_secret(&enc);
                    if !secret.is_empty() {
                        tracing::info!(server, "appSecret obtained from distribution server");
                        return Some(secret);
                    }
                }
                tracing::warn!(server, "secret response missing encryptedAppSecret");
            }
            Ok(resp) => tracing::warn!(server, status = %resp.status(), "secret server error"),
            Err(e) => tracing::warn!(server, error = %e, "secret server unreachable"),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用还原算法的逆运算构造测试向量（编码 = 还原的逆），
    /// 保证 decode(encode(x)) == x 的往返性质。
    fn encode_secret(plain: &str) -> String {
        // 逆 4) 大小写互换（自逆）
        let s: String = plain
            .chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    c.to_ascii_lowercase()
                } else if c.is_ascii_lowercase() {
                    c.to_ascii_uppercase()
                } else {
                    c
                }
            })
            .collect();
        // 逆 3) 数字取补（自逆，d=10-d' 双向）
        let s: String = s
            .chars()
            .map(|c| {
                if c.is_ascii_digit() {
                    let d = c as u8 - b'0';
                    (b'0' + (10 - d)) as char
                } else {
                    c
                }
            })
            .collect();
        // 逆 2) 把倒数第 5 位移回开头
        let chars: Vec<char> = s.chars().collect();
        let s: String = if chars.len() >= 5 {
            let idx = chars.len() - 5;
            let first = chars[idx];
            let head: String = chars[..idx].iter().collect();
            let tail: String = chars[idx + 1..].iter().collect();
            format!("{first}{head}{tail}")
        } else {
            s
        };
        // 逆 1) 字母表镜像（自逆）
        s.chars()
            .map(|c| {
                if c.is_ascii_uppercase() {
                    (b'A' + 25 - (c as u8 - b'A')) as char
                } else if c.is_ascii_lowercase() {
                    (b'a' + 25 - (c as u8 - b'a')) as char
                } else {
                    c
                }
            })
            .collect()
    }

    #[test]
    fn decode_roundtrip() {
        for plain in ["abcDEF123xy", "s3cr3tK3yV4lu3", "ab", "aBcDe12345XyZ"] {
            let enc = encode_secret(plain);
            assert_eq!(decode_secret(&enc), plain, "roundtrip failed for {plain}");
        }
    }

    #[test]
    fn mirror_is_case_preserving_in_step1() {
        // 'a' 镜像 'z'，再步骤 4 变 'Z'（短串 <5 不移位；无数字）
        assert_eq!(decode_secret("a"), "Z");
        assert_eq!(decode_secret("A"), "z");
    }
}
