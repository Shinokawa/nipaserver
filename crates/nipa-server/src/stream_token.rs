//! 流媒体签名 URL（§8.4）。
//!
//! `<video>`/外部播放器无法携带 Authorization header，流端点走短期签名
//! query：`?exp=<unix_secs>&sig=<hex(hmac_sha256(secret, "{scope}|{id}|{exp}"))>`。
//! secret 进程启动时随机生成——重启后旧 URL 失效，播放 URL 本就是会话级
//! 临时物，可接受；好处是零配置零落盘。

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;
const MAX_FUTURE_SECS: i64 = 24 * 60 * 60;

#[derive(Clone)]
pub struct StreamTokenKey(std::sync::Arc<[u8; 32]>);

impl StreamTokenKey {
    pub fn generate() -> Self {
        let mut key = [0u8; 32];
        // HMAC key 必须来自 OS CSPRNG，不能使用 fastrand 类非密码 PRNG。
        getrandom::fill(&mut key).expect("OS CSPRNG unavailable");
        Self(std::sync::Arc::new(key))
    }

    fn sign_raw_subject(&self, scope: &str, subject: &str, exp: i64) -> String {
        let mut mac = HmacSha256::new_from_slice(self.0.as_ref()).expect("hmac key");
        mac.update(format!("{scope}|{subject}|{exp}").as_bytes());
        hex::encode(mac.finalize().into_bytes())
    }

    #[cfg(test)]
    fn sign_raw(&self, scope: &str, id: i64, exp: i64) -> String {
        self.sign_raw_subject(scope, &id.to_string(), exp)
    }

    /// 生成签名 URL 的 query 部分（`exp=...&sig=...`）。
    pub fn sign(&self, scope: &str, id: i64, ttl: std::time::Duration) -> String {
        self.sign_subject(scope, &id.to_string(), ttl)
    }

    pub fn sign_subject(&self, scope: &str, subject: &str, ttl: std::time::Duration) -> String {
        let ttl = ttl.as_secs().min(MAX_FUTURE_SECS as u64) as i64;
        let exp = now_secs().saturating_add(ttl);
        let sig = self.sign_raw_subject(scope, subject, exp);
        format!("exp={exp}&sig={sig}")
    }

    /// 校验。恒定时间比较由 hmac 的 verify 保证。
    pub fn verify(&self, scope: &str, id: i64, exp: i64, sig: &str) -> bool {
        self.verify_subject(scope, &id.to_string(), exp, sig)
    }

    pub fn verify_subject(&self, scope: &str, subject: &str, exp: i64, sig: &str) -> bool {
        let now = now_secs();
        if exp < now || exp > now.saturating_add(MAX_FUTURE_SECS) {
            return false;
        }
        let Ok(sig_bytes) = hex::decode(sig) else {
            return false;
        };
        let mut mac = HmacSha256::new_from_slice(self.0.as_ref()).expect("hmac key");
        mac.update(format!("{scope}|{subject}|{exp}").as_bytes());
        mac.verify_slice(&sig_bytes).is_ok()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn sign_verify_roundtrip() {
        let key = StreamTokenKey::generate();
        let q = key.sign("direct", 42, Duration::from_secs(60));
        let exp: i64 = q
            .split('&')
            .next()
            .unwrap()
            .strip_prefix("exp=")
            .unwrap()
            .parse()
            .unwrap();
        let sig = q.split("sig=").nth(1).unwrap();
        assert!(key.verify("direct", 42, exp, sig));
        // 错误 scope / id / 篡改签名均拒绝
        assert!(!key.verify("hls", 42, exp, sig));
        assert!(!key.verify("direct", 43, exp, sig));
        assert!(!key.verify("direct", 42, exp, "deadbeef"));
    }

    #[test]
    fn expired_rejected() {
        let key = StreamTokenKey::generate();
        let exp = now_secs() - 10;
        let sig = key.sign_raw("direct", 1, exp);
        assert!(!key.verify("direct", 1, exp, &sig));
    }

    #[test]
    fn keys_are_independent() {
        let a = StreamTokenKey::generate();
        let b = StreamTokenKey::generate();
        let q = a.sign("direct", 1, Duration::from_secs(60));
        let exp: i64 = q
            .split('&')
            .next()
            .unwrap()
            .strip_prefix("exp=")
            .unwrap()
            .parse()
            .unwrap();
        let sig = q.split("sig=").nth(1).unwrap();
        assert!(!b.verify("direct", 1, exp, sig));
    }

    #[test]
    fn excessive_future_expiry_rejected() {
        let key = StreamTokenKey::generate();
        let exp = now_secs() + MAX_FUTURE_SECS + 1;
        let sig = key.sign_raw("direct", 1, exp);
        assert!(!key.verify("direct", 1, exp, &sig));
    }
}
