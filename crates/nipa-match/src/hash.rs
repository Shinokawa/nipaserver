//! 弹弹play 文件 hash：前 16MB（16×1024×1024 字节）的 MD5，hex 小写。
//!
//! 文件不足 16MB 时对全文件计算（调研文档 §1）。官方测试样本
//! （https://kaedei.lanzouo.com/itjCq30geele）前 16MB MD5 =
//! `658d05841b9476ccc7420b3f0bb21c3b`，可作实现校验基准。

use md5::{Digest, Md5};
use std::io::Read;

/// 参与 hash 的前缀长度：16MB。
pub const DANDAN_HASH_SIZE: usize = 16 * 1024 * 1024;

/// 对内存中的数据计算弹弹play hash（只取前 16MB）。
pub fn dandan_hash_bytes(data: &[u8]) -> String {
    let prefix = &data[..data.len().min(DANDAN_HASH_SIZE)];
    hex::encode(Md5::digest(prefix))
}

/// 从 `Read` 流式计算弹弹play hash（最多读取前 16MB，不整块载入内存）。
///
/// 同步阻塞 IO；异步上下文中请包在 `spawn_blocking` 里（IO 节流由上层负责，§4.1）。
pub fn dandan_hash_reader<R: Read>(mut reader: R) -> std::io::Result<String> {
    let mut hasher = Md5::new();
    let mut remaining = DANDAN_HASH_SIZE;
    let mut buf = vec![0u8; 64 * 1024];
    while remaining > 0 {
        let want = buf.len().min(remaining);
        let n = reader.read(&mut buf[..want])?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        remaining -= n;
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 期望值由 python3 hashlib 独立生成。

    #[test]
    fn empty_input() {
        assert_eq!(dandan_hash_bytes(b""), "d41d8cd98f00b204e9800998ecf8427e");
    }

    #[test]
    fn small_input_hashes_whole_file() {
        assert_eq!(
            dandan_hash_bytes(b"hello"),
            "5d41402abc4b2a76b9719d911017c592"
        );
    }

    #[test]
    fn exactly_16mb_zeros() {
        let data = vec![0u8; DANDAN_HASH_SIZE];
        assert_eq!(dandan_hash_bytes(&data), "2c7ab85a893283e98c931e9511add182");
    }

    #[test]
    fn larger_than_16mb_only_prefix_counts() {
        // 16MB+4096 的模式化数据：期望值 = 前 16MB 的 MD5（python 生成）。
        let data: Vec<u8> = (0..DANDAN_HASH_SIZE + 4096)
            .map(|i| ((i * 31 + 7) % 256) as u8)
            .collect();
        assert_eq!(dandan_hash_bytes(&data), "86cf57f926ca2598ddf451bb3375ac32");
        // 尾部再多 1 字节不影响结果。
        let mut longer = data.clone();
        longer.push(0xFF);
        assert_eq!(dandan_hash_bytes(&longer), dandan_hash_bytes(&data));
    }

    #[test]
    fn reader_matches_bytes() {
        let data: Vec<u8> = (0..DANDAN_HASH_SIZE + 999)
            .map(|i| ((i * 31 + 7) % 256) as u8)
            .collect();
        let via_reader = dandan_hash_reader(&data[..]).unwrap();
        assert_eq!(via_reader, dandan_hash_bytes(&data));
        assert_eq!(
            dandan_hash_reader(&b"hello"[..]).unwrap(),
            "5d41402abc4b2a76b9719d911017c592"
        );
    }
}
