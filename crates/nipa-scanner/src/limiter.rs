//! 磁盘 IO 并发节流（§4.1）。
//!
//! hash 计算与字幕抽取共用同一把 [`IoLimiter`]，并发上限可配（默认 2）。
//! **与 agent 并发无关**——agent 全局并发由所选模型档位 RPM 联动（§2.2），
//! 这里只管磁盘/NAS 带宽，防止扫描打满网络挂载。

use std::sync::Arc;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// 默认 IO 并发上限。
pub const DEFAULT_IO_CONCURRENCY: usize = 2;

/// 一次受节流保护的 IO 许可。Drop 即释放。
pub type IoPermit = OwnedSemaphorePermit;

/// 磁盘 IO 并发节流器（tokio [`Semaphore`] 封装）。
///
/// `Clone` 共享同一底层信号量——scanner 与字幕抽取各持一份克隆即可。
///
/// ```no_run
/// # async fn demo(limiter: nipa_scanner::IoLimiter) -> std::io::Result<()> {
/// let _permit = limiter.acquire().await;
/// let hash = tokio::task::spawn_blocking(|| nipa_scanner::dandan_hash("/lib/ep01.mkv"))
///     .await
///     .expect("spawn_blocking panicked")?;
/// // _permit drop 时自动释放。
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone)]
pub struct IoLimiter {
    semaphore: Arc<Semaphore>,
}

impl IoLimiter {
    /// 指定并发上限创建。`max_concurrency` 为 0 时按 1 处理（0 会永久卡死所有 IO）。
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency.max(1))),
        }
    }

    /// 取得一个 IO 许可；满载时挂起等待。许可 Drop 即释放。
    pub async fn acquire(&self) -> IoPermit {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .expect("IoLimiter 的信号量永不关闭")
    }

    /// 立即尝试取得许可；满载时返回 `None`（不等待）。
    pub fn try_acquire(&self) -> Option<IoPermit> {
        self.semaphore.clone().try_acquire_owned().ok()
    }

    /// 当前空闲许可数（观测/调试用）。
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }
}

impl Default for IoLimiter {
    fn default() -> Self {
        Self::new(DEFAULT_IO_CONCURRENCY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn default_allows_two_concurrent_permits() {
        let limiter = IoLimiter::default();
        assert_eq!(limiter.available(), DEFAULT_IO_CONCURRENCY);

        let p1 = limiter.acquire().await;
        let p2 = limiter.acquire().await;
        assert_eq!(limiter.available(), 0);
        assert!(limiter.try_acquire().is_none(), "满载时 try_acquire 应失败");

        drop(p1);
        assert_eq!(limiter.available(), 1);
        drop(p2);
        assert_eq!(limiter.available(), 2);
    }

    #[tokio::test]
    async fn clones_share_the_same_budget() {
        let a = IoLimiter::new(1);
        let b = a.clone();
        let _p = a.acquire().await;
        assert!(b.try_acquire().is_none(), "克隆必须共享同一信号量");
    }

    #[tokio::test]
    async fn acquire_waits_until_permit_released() {
        let limiter = IoLimiter::new(1);
        let permit = limiter.acquire().await;

        let waiter = {
            let limiter = limiter.clone();
            tokio::spawn(async move {
                let _p = limiter.acquire().await; // 在 permit 释放前挂起
            })
        };

        // 未释放前 waiter 不应完成。
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());

        drop(permit);
        waiter.await.expect("waiter 应在许可释放后完成");
    }

    #[tokio::test]
    async fn zero_concurrency_is_clamped_to_one() {
        let limiter = IoLimiter::new(0);
        let _p = limiter.acquire().await; // 不会永久挂起
        assert_eq!(limiter.available(), 0);
    }
}
