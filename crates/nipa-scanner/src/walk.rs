//! 目录扫描：递归遍历媒体库根目录，发现视频文件（§4.1 文件发现）。
//!
//! 同步实现——调用方（nipa-server / 回流客户端）在异步上下文中经
//! `tokio::task::spawn_blocking` 使用。

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use walkdir::WalkDir;

/// 识别为视频的扩展名集合（不区分大小写）。
///
/// 需要自定义集合时用 [`walk_library_with_extensions`]。
pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "mkv", "avi", "mov", "wmv", "flv", "ts", "m2ts", "webm",
];

/// 一次扫描中发现的物理文件（入库前的中间表示，对应 §9 media_files 的入口数据）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveredFile {
    /// 相对库根的规范化路径：一律 '/' 分隔；非 UTF-8 路径 lossy 存储（§9 rel_path）。
    pub rel_path: String,
    /// 非 UTF-8 路径的原始字节（对应 §9 raw_path BLOB）；路径为合法 UTF-8 时为 `None`。
    ///
    /// Unix 上为相对路径的原始 `OsStr` 字节（平台分隔符原样）；非 Unix 平台
    /// 无字节级路径表示，存 lossy UTF-8（与 `rel_path` 等价，仅作占位）。
    pub raw_path: Option<Vec<u8>>,
    /// 文件大小（字节）。
    pub size: u64,
    /// Unix mtime，毫秒。与 [`crate::fingerprint`] 的 `modified_millis` 输入对齐；
    /// SMB/FAT/Docker 挂载的精度与漂移问题见 §4.1（因此 fingerprint 不作识别缓存 key）。
    pub mtime: i64,
}

/// 递归遍历 `root`，返回发现的视频文件（按 `rel_path` 排序，结果确定性）。
///
/// 规则（§4.1 / §8.4 路径安全）：
/// - 不跟随 symlink（目录与文件均跳过，防逃逸与环）；
/// - 跳过隐藏条目（名字以 `.` 开头的目录整棵剪枝、文件跳过——
///   同时挡掉 macOS `.DS_Store`/`._*` AppleDouble 噪声）；
/// - 仅收集扩展名命中 [`VIDEO_EXTENSIONS`]（不区分大小写）的普通文件；
/// - 不可读的条目（权限等 IO 错误）静默跳过，不中断整次扫描。
pub fn walk_library(root: impl AsRef<Path>) -> Vec<DiscoveredFile> {
    walk_library_with_extensions(root, VIDEO_EXTENSIONS)
}

/// 同 [`walk_library`]，但使用自定义扩展名集合（不含点号，不区分大小写）。
pub fn walk_library_with_extensions(
    root: impl AsRef<Path>,
    extensions: &[&str],
) -> Vec<DiscoveredFile> {
    let root = root.as_ref();
    let mut out = Vec::new();

    let walker = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        // depth 0 是 root 本身，永不过滤（root 目录名以 '.' 开头是合法的）。
        .filter_entry(|e| e.depth() == 0 || !is_hidden(e.file_name()));

    for entry in walker {
        let Ok(entry) = entry else {
            continue; // 不可读条目：跳过，不中断扫描。
        };
        if !entry.file_type().is_file() || entry.path_is_symlink() {
            continue;
        }
        if !has_video_extension(entry.path(), extensions) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };

        let (rel_path, raw_path) = normalize_rel_path(rel);

        let mtime = meta
            .modified()
            .map(system_time_to_millis)
            .unwrap_or_default();

        out.push(DiscoveredFile {
            rel_path,
            raw_path,
            size: meta.len(),
            mtime,
        });
    }

    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    out
}

/// 相对路径规范化：'/' 分隔 lossy 字符串 + 非 UTF-8 时的原始字节。
fn normalize_rel_path(rel: &Path) -> (String, Option<Vec<u8>>) {
    // Windows 反斜杠在 components 层被抹平，各段以 '/' 重接。
    let rel_path = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    // 非 UTF-8 路径：lossy 之外保留原始字节（§9 raw_path）。
    let raw_path = if rel.as_os_str().to_str().is_some() {
        None
    } else {
        Some(os_str_bytes(rel.as_os_str()))
    };
    (rel_path, raw_path)
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

fn has_video_extension(path: &Path, extensions: &[&str]) -> bool {
    let Some(ext) = path.extension() else {
        return false;
    };
    let ext = ext.to_string_lossy().to_ascii_lowercase();
    extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))
}

#[cfg(unix)]
fn os_str_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    s.as_bytes().to_vec()
}

#[cfg(not(unix))]
fn os_str_bytes(s: &std::ffi::OsStr) -> Vec<u8> {
    // Windows 路径是 WTF-16，无原始字节表示；存 lossy UTF-8 占位。
    s.to_string_lossy().into_owned().into_bytes()
}

fn system_time_to_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(e) => -(e.duration().as_millis() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TempDir;

    fn rel_paths(files: &[DiscoveredFile]) -> Vec<&str> {
        files.iter().map(|f| f.rel_path.as_str()).collect()
    }

    #[test]
    fn discovers_video_files_recursively_with_normalized_rel_path() {
        let dir = TempDir::new("walk-basic");
        dir.write("Anime/Bocchi/ep01.mkv", b"AAAA");
        dir.write("Anime/Bocchi/ep01.ass", b"subtitle"); // 非视频扩展名
        dir.write("Movies/Deep/Nest/film.m2ts", b"BBBBBB");
        dir.write("readme.txt", b"hi");
        dir.write("root.mp4", b"CC");

        let files = walk_library(dir.path());
        assert_eq!(
            rel_paths(&files),
            vec!["Anime/Bocchi/ep01.mkv", "Movies/Deep/Nest/film.m2ts", "root.mp4"]
        );

        let ep01 = &files[0];
        assert_eq!(ep01.size, 4);
        assert!(ep01.mtime > 0, "mtime 应为正的 Unix 毫秒");
        assert!(ep01.raw_path.is_none(), "UTF-8 路径不应携带 raw_path");
    }

    #[test]
    fn extension_matching_is_case_insensitive() {
        let dir = TempDir::new("walk-case");
        dir.write("a.MP4", b"x");
        dir.write("b.MkV", b"x");
        dir.write("c.WEBM", b"x");

        let files = walk_library(dir.path());
        assert_eq!(rel_paths(&files), vec!["a.MP4", "b.MkV", "c.WEBM"]);
    }

    #[test]
    fn skips_hidden_directories_and_hidden_files() {
        let dir = TempDir::new("walk-hidden");
        dir.write(".hidden/secret.mkv", b"x"); // 隐藏目录整棵剪枝
        dir.write("Anime/.cache/tmp.mp4", b"x"); // 深层隐藏目录同样剪枝
        dir.write("Anime/._junk.mkv", b"x"); // AppleDouble 隐藏文件
        dir.write("Anime/ok.mkv", b"x");

        let files = walk_library(dir.path());
        assert_eq!(rel_paths(&files), vec!["Anime/ok.mkv"]);
    }

    #[cfg(unix)]
    #[test]
    fn skips_symlinks_to_files_and_directories() {
        let dir = TempDir::new("walk-symlink");
        let real = dir.write("real/video.mkv", b"data");
        std::os::unix::fs::symlink(&real, dir.path().join("link.mkv")).unwrap();
        std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("linkdir")).unwrap();

        let files = walk_library(dir.path());
        assert_eq!(rel_paths(&files), vec!["real/video.mkv"]);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_rel_path_is_lossy_with_raw_bytes_preserved() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        use std::path::Path;

        // 路径层逻辑不依赖文件系统，直接验证（0xFF 不是合法 UTF-8）。
        let rel = Path::new(OsStr::from_bytes(b"dir\xFF/bad\xFFname.mkv"));
        let (rel_path, raw_path) = normalize_rel_path(rel);
        assert_eq!(rel_path, "dir\u{FFFD}/bad\u{FFFD}name.mkv");
        assert_eq!(raw_path.as_deref(), Some(b"dir\xFF/bad\xFFname.mkv".as_slice()));

        // 端到端（仅在文件系统允许非 UTF-8 文件名时执行；
        // macOS APFS 会以 EILSEQ 拒绝，Linux/ext4 上完整覆盖）。
        let dir = TempDir::new("walk-nonutf8");
        let name = OsStr::from_bytes(b"bad\xFFname.mkv");
        if std::fs::write(dir.path().join(name), b"x").is_ok() {
            let files = walk_library(dir.path());
            assert_eq!(files.len(), 1);
            assert_eq!(files[0].rel_path, "bad\u{FFFD}name.mkv");
            assert_eq!(files[0].raw_path.as_deref(), Some(b"bad\xFFname.mkv".as_slice()));
        }
    }

    #[test]
    fn utf8_rel_path_has_no_raw_bytes() {
        let (rel_path, raw_path) =
            normalize_rel_path(std::path::Path::new("Anime/ぼっち/第01話.mkv"));
        assert_eq!(rel_path, "Anime/ぼっち/第01話.mkv");
        assert!(raw_path.is_none());
    }

    #[test]
    fn custom_extension_set_overrides_default() {
        let dir = TempDir::new("walk-custom");
        dir.write("a.mkv", b"x");
        dir.write("b.iso", b"x");

        let files = walk_library_with_extensions(dir.path(), &["iso"]);
        assert_eq!(rel_paths(&files), vec!["b.iso"]);
    }

    #[test]
    fn nonexistent_root_yields_empty() {
        let dir = TempDir::new("walk-missing");
        let missing = dir.path().join("no-such-dir");
        assert!(walk_library(&missing).is_empty());
    }
}
