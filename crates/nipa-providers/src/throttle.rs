//! 每 provider 一个的简单令牌桶节流（开发文档 §5 egress 层）。
//!
//! 手写 tokio `Mutex` + `Instant`，不引第三方 crate。桶容量 = 每秒速率
//! （即最多允许 1 秒的突发），按耗时线性补充。

use std::time::{Duration, Instant};

use tokio::sync::Mutex;

/// 令牌桶。`acquire` 在无可用令牌时 `sleep` 等待，不丢弃请求。
pub struct Throttle {
    capacity: f64,
    refill_per_sec: f64,
    state: Mutex<State>,
}

struct State {
    tokens: f64,
    last_refill: Instant,
}

impl Throttle {
    /// `rate_per_sec`：稳态速率，同时也是桶容量（至少 1）。
    pub fn new(rate_per_sec: f64) -> Self {
        let capacity = rate_per_sec.max(1.0);
        Self {
            capacity,
            refill_per_sec: rate_per_sec.max(f64::MIN_POSITIVE),
            state: Mutex::new(State {
                tokens: capacity,
                last_refill: Instant::now(),
            }),
        }
    }

    /// 取走一个令牌；不足时等待补充。
    pub async fn acquire(&self) {
        loop {
            let wait = {
                let mut s = self.state.lock().await;
                let now = Instant::now();
                let elapsed = now.duration_since(s.last_refill).as_secs_f64();
                s.tokens = (s.tokens + elapsed * self.refill_per_sec).min(self.capacity);
                s.last_refill = now;
                if s.tokens >= 1.0 {
                    s.tokens -= 1.0;
                    None
                } else {
                    Some(Duration::from_secs_f64(
                        (1.0 - s.tokens) / self.refill_per_sec,
                    ))
                }
            };
            match wait {
                None => return,
                Some(d) => tokio::time::sleep(d).await,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn burst_within_capacity_does_not_block() {
        let t = Throttle::new(10.0);
        let start = Instant::now();
        for _ in 0..10 {
            t.acquire().await;
        }
        assert!(start.elapsed() < Duration::from_millis(100));
    }

    #[tokio::test]
    async fn exceeding_capacity_waits() {
        let t = Throttle::new(10.0);
        let start = Instant::now();
        for _ in 0..12 {
            t.acquire().await;
        }
        // 第 11、12 个令牌需等待补充（各 ~100ms）。
        assert!(start.elapsed() >= Duration::from_millis(150));
    }
}
