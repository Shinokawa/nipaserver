//! 文件指纹与弹弹play hash（§4.1 L0/L1）。
//!
//! - [`fingerprint`]：`sha256("size|modified_millis")` 前 16 hex——仅用于
//!   "是否需要重算 hash"的变更检测（对齐 NipaPlay 现有做法），**不作为识别
//!   结果的缓存 key**（size+mtime 相同的不同文件会串档，§4.1）；
//! - [`dandan_hash`]：文件前 16MB 的 MD5 hex（不足 16MB 取全文件），
//!   即弹弹play `/api/v2/match` 所用 hash；L0 缓存主键 = `(size, dandan_hash)`。

use md5::{Digest as _, Md5};
use sha2::Sha256;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

/// 弹弹play hash 的读取上限：前 16MB。
pub const DANDAN_HASH_READ_LIMIT: u64 = 16 * 1024 * 1024;

/// 变更指纹：`sha256("{size}|{modified_millis}")` 的前 16 位 hex。
///
/// 仅用于 diff 扫描判断"文件是否变过、需不需要重算 dandan_hash"，
/// 语义对齐 NipaPlay 客户端现有实现。
pub fn fingerprint(size: u64, modified_millis: i64) -> String {
    let digest = Sha256::digest(format!("{size}|{modified_millis}").as_bytes());
    hex::encode(&digest[..8]) // 8 字节 = 16 个 hex 字符
}

/// 弹弹play hash：文件前 16MB（不足取全文件）的 MD5，小写 hex。
///
/// 同步阻塞 IO——异步上下文中经 `spawn_blocking` 调用，并先取
/// [`crate::IoLimiter`] 许可（§4.1 IO 节流）。
pub fn dandan_hash(path: impl AsRef<Path>) -> io::Result<String> {
    let file = File::open(path)?;
    let mut reader = file.take(DANDAN_HASH_READ_LIMIT);
    let mut hasher = Md5::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    #[test]
    fn fingerprint_is_first_16_hex_of_sha256() {
        // sha256("1024|1700000000000") = ab09b2dec59065d3a160d62b94f829537a24a981fd34012972cdf4efdd4ca0cc
        assert_eq!(fingerprint(1024, 1_700_000_000_000), "ab09b2dec59065d3");
        assert_eq!(fingerprint(1024, 1_700_000_000_000).len(), 16);
    }

    #[test]
    fn fingerprint_changes_with_size_or_mtime() {
        let base = fingerprint(100, 1000);
        assert_ne!(base, fingerprint(101, 1000));
        assert_ne!(base, fingerprint(100, 1001));
        assert_eq!(base, fingerprint(100, 1000)); // 确定性
    }

    #[test]
    fn dandan_hash_small_file_is_whole_file_md5() {
        let dir = TempDir::new("hash-small");
        let path = dir.write("small.mkv", b"hello world");
        // md5("hello world")
        assert_eq!(dandan_hash(&path).unwrap(), "5eb63bbbe01eeed093cb22bb8f5acdc3");
    }

    #[test]
    fn dandan_hash_empty_file() {
        let dir = TempDir::new("hash-empty");
        let path = dir.write("empty.mkv", b"");
        // md5("")
        assert_eq!(dandan_hash(&path).unwrap(), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn dandan_hash_only_reads_first_16mb() {
        let dir = TempDir::new("hash-16mb");
        let limit = DANDAN_HASH_READ_LIMIT as usize;

        // 恰好 16MB 与 16MB+尾巴：前 16MB 相同 → hash 必须相同。
        let head = vec![0xABu8; limit];
        let exact = dir.write("exact.mkv", &head);

        let mut longer = head.clone();
        longer.extend_from_slice(b"trailing bytes that must be ignored");
        let padded = dir.write("padded.mkv", &longer);

        let h_exact = dandan_hash(&exact).unwrap();
        let h_padded = dandan_hash(&padded).unwrap();
        assert_eq!(h_exact, h_padded, "超出 16MB 的内容不得影响 hash");

        // 而前 16MB 内容不同则 hash 不同。
        let mut different = head;
        different[0] = 0xCD;
        let diff = dir.write("diff.mkv", &different);
        assert_ne!(dandan_hash(&diff).unwrap(), h_exact);
    }

    #[test]
    fn dandan_hash_missing_file_is_io_error() {
        let dir = TempDir::new("hash-missing");
        assert!(dandan_hash(dir.path().join("nope.mkv")).is_err());
    }

    /// 官方基准（§4.1）：弹弹play 官方测试视频前 16MB MD5 =
    /// `658d05841b9476ccc7420b3f0bb21c3b`。
    ///
    /// 需本地存在该测试视频，路径经环境变量 `NIPA_DANDAN_SAMPLE` 指定：
    /// `NIPA_DANDAN_SAMPLE=/path/to/sample.mp4 cargo test -p nipa-scanner -- --ignored`
    #[test]
    #[ignore = "需要弹弹play官方测试视频（NIPA_DANDAN_SAMPLE 环境变量指定路径）"]
    fn dandan_hash_official_sample() {
        let path = std::env::var("NIPA_DANDAN_SAMPLE")
            .expect("设置 NIPA_DANDAN_SAMPLE 指向官方测试视频");
        assert_eq!(dandan_hash(path).unwrap(), "658d05841b9476ccc7420b3f0bb21c3b");
    }
}
