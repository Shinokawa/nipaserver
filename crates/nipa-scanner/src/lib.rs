//! nipa-scanner：目录扫描、文件指纹、IO 节流、evidence bundle、diff 扫描。
//!
//! 对应开发文档 §4.1（三级管线 / L0 缓存 / IO 节流）与 §4.2（evidence bundle）。
//!
//! 回流约束（§3.1）：本 crate 经 flutter_rust_bridge 回流 NipaPlay 客户端，
//! **不得依赖 axum/tower/sqlx**——DB 操作在 nipa-server 侧完成，这里只产出纯数据。
//!
//! 模块一览：
//! - [`walk`]：递归目录扫描（同步实现，调用方经 `spawn_blocking` 使用）；
//! - [`hash`]：变更指纹 `sha256(size|mtime)[:16]` 与弹弹play hash（前 16MB MD5）;
//! - [`limiter`]：磁盘 IO 并发节流（hash 计算与字幕抽取共用，与 agent 并发无关）；
//! - [`evidence`]：L2 agent 的 evidence bundle 组装（中文证据文本）；
//! - [`plan`]：diff 扫描纯函数（新增/变更/移动候选/缺失）。
//!
//! TODO（后续里程碑）：
//! - 文件系统 watcher（notify crate）；
//! - 字幕抽取与编码嗅探（encoding_rs + chardetng，喂 agent 前转 UTF-8）。

pub mod evidence;
pub mod hash;
pub mod limiter;
pub mod plan;
pub mod walk;

pub use evidence::{EvidenceParams, build_evidence};
pub use hash::{DANDAN_HASH_READ_LIMIT, dandan_hash, fingerprint};
pub use limiter::{IoLimiter, IoPermit};
pub use plan::{KnownFile, MovedCandidate, ScanPlan, plan_scan};
pub use walk::{DiscoveredFile, VIDEO_EXTENSIONS, walk_library, walk_library_with_extensions};

#[cfg(test)]
pub(crate) mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// 极简临时目录（std-only，避免引入 tempfile 依赖）。Drop 时递归删除。
    pub struct TempDir(PathBuf);

    impl TempDir {
        pub fn new(tag: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "nipa-scanner-test-{}-{tag}-{n}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("创建临时目录失败");
            Self(dir)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }

        /// 在临时目录下写入相对路径 `rel`（自动创建父目录），返回绝对路径。
        pub fn write(&self, rel: &str, contents: &[u8]) -> PathBuf {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).expect("创建父目录失败");
            }
            std::fs::write(&p, contents).expect("写入测试文件失败");
            p
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
