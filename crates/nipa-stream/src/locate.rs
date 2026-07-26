//! ffmpeg/ffprobe 启动探测（§6.3）：不静态链接、不 FFI，sidecar 子进程模式。
//!
//! 探测顺序：环境变量（`NIPA_FFMPEG`/`NIPA_FFPROBE`）→ PATH → 应用目录（可执行
//! 文件所在目录）。每个候选都要跑 `-version` 验证可执行并取版本首行——路径存在
//! 但跑不起来（权限/架构不符）视为该候选失败，继续降级。
//!
//! 探测失败（返回 `None`）触发 §6.3 降级矩阵：仅 Direct Play，L2 evidence 退化为
//! 文件名 + 目录 + 兄弟文件（见 nipa-scanner evidence 模块的降级说明行）。

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// 探测成功后的 ffmpeg/ffprobe 路径与版本信息。
#[derive(Debug, Clone)]
pub struct FfmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    /// `ffmpeg -version` 输出首行（如 `ffmpeg version 7.1.1 Copyright ...`）。
    pub ffmpeg_version: String,
    /// `ffprobe -version` 输出首行。
    pub ffprobe_version: String,
}

/// 探测器：字段即三级探测源（§6.3）。[`FfmpegLocator::detect`] 从真实环境组装；
/// 测试可手工构造以覆盖各级降级路径（edition 2024 下 `set_var` 是 unsafe，
/// 测试不动真实环境变量）。
#[derive(Debug, Clone, Default)]
pub struct FfmpegLocator {
    /// `NIPA_FFMPEG`：显式指定的 ffmpeg 路径。
    pub env_ffmpeg: Option<OsString>,
    /// `NIPA_FFPROBE`：显式指定的 ffprobe 路径。
    pub env_ffprobe: Option<OsString>,
    /// `PATH`（which 行为手写：按分隔符拆目录逐个找可执行文件）。
    pub path_var: Option<OsString>,
    /// 应用目录（当前可执行文件所在目录，release 附带 LGPL 构建时的落点）。
    pub app_dir: Option<PathBuf>,
}

impl FfmpegLocator {
    /// 从真实进程环境探测（生产入口）。
    pub fn detect() -> Option<FfmpegPaths> {
        Self::from_env().locate()
    }

    /// 读取真实环境变量与可执行文件目录组装探测器。
    pub fn from_env() -> Self {
        Self {
            env_ffmpeg: env::var_os("NIPA_FFMPEG"),
            env_ffprobe: env::var_os("NIPA_FFPROBE"),
            path_var: env::var_os("PATH"),
            app_dir: env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(Path::to_path_buf)),
        }
    }

    /// 按序探测。ffmpeg 与 ffprobe 各自独立走三级降级；两者都找到才算成功
    /// （字幕抽取要 ffmpeg、媒体探测要 ffprobe，缺一即进降级矩阵）。
    pub fn locate(&self) -> Option<FfmpegPaths> {
        let (ffmpeg, ffmpeg_version) = self.find_one("ffmpeg", self.env_ffmpeg.as_deref())?;
        let (ffprobe, ffprobe_version) = self.find_one("ffprobe", self.env_ffprobe.as_deref())?;
        Some(FfmpegPaths {
            ffmpeg,
            ffprobe,
            ffmpeg_version,
            ffprobe_version,
        })
    }

    /// 单个二进制的三级探测：env 显式路径 → PATH 搜索 → 应用目录。
    /// 每个候选跑 `-version` 验证，失败继续下一级。
    fn find_one(
        &self,
        name: &str,
        env_override: Option<&std::ffi::OsStr>,
    ) -> Option<(PathBuf, String)> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = env_override {
            candidates.push(PathBuf::from(p));
        }
        if let Some(p) = self.search_path(name) {
            candidates.push(p);
        }
        if let Some(dir) = &self.app_dir {
            candidates.push(dir.join(exe_name(name)));
        }
        candidates.into_iter().find_map(|c| {
            let version = version_of(&c)?;
            Some((c, version))
        })
    }

    /// 手写 which：按平台分隔符拆 PATH，返回第一个存在的可执行候选。
    fn search_path(&self, name: &str) -> Option<PathBuf> {
        let path_var = self.path_var.as_ref()?;
        let file = exe_name(name);
        env::split_paths(path_var)
            .filter(|dir| !dir.as_os_str().is_empty())
            .map(|dir| dir.join(&file))
            .find(|p| is_executable(p))
    }
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// 跑 `<path> -version` 取输出首行。spawn 失败/非零退出/空输出都返回 `None`
/// （候选无效，继续降级）。
fn version_of(path: &Path) -> Option<String> {
    let output = Command::new(path).arg("-version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let first = stdout.lines().next()?.trim();
    if first.is_empty() {
        None
    } else {
        Some(first.to_string())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// 造一个假 ffmpeg/ffprobe：shell 脚本打印一行版本信息。
    fn fake_binary(dir: &Path, name: &str, version_line: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\necho \"{version_line}\"\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn test_dir(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("nipa-stream-locate-{}-{tag}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn env_override_wins() {
        let dir = test_dir("env");
        let ffmpeg = fake_binary(&dir, "my-ffmpeg", "ffmpeg version env-9.9");
        let ffprobe = fake_binary(&dir, "my-ffprobe", "ffprobe version env-9.9");
        let locator = FfmpegLocator {
            env_ffmpeg: Some(ffmpeg.clone().into_os_string()),
            env_ffprobe: Some(ffprobe.clone().into_os_string()),
            path_var: None,
            app_dir: None,
        };
        let paths = locator.locate().expect("env 级探测应命中");
        assert_eq!(paths.ffmpeg, ffmpeg);
        assert_eq!(paths.ffprobe, ffprobe);
        assert_eq!(paths.ffmpeg_version, "ffmpeg version env-9.9");
        assert_eq!(paths.ffprobe_version, "ffprobe version env-9.9");
    }

    #[test]
    fn falls_back_to_path_search() {
        let dir = test_dir("path");
        fake_binary(&dir, "ffmpeg", "ffmpeg version path-1.0");
        fake_binary(&dir, "ffprobe", "ffprobe version path-1.0");
        // env 指向不存在的路径：应跳过该级、降级到 PATH。
        let locator = FfmpegLocator {
            env_ffmpeg: Some(dir.join("no-such-ffmpeg").into_os_string()),
            env_ffprobe: None,
            path_var: Some(
                env::join_paths([dir.join("empty-subdir-not-created"), dir.clone()]).unwrap(),
            ),
            app_dir: None,
        };
        let paths = locator.locate().expect("PATH 级探测应命中");
        assert_eq!(paths.ffmpeg, dir.join("ffmpeg"));
        assert_eq!(paths.ffmpeg_version, "ffmpeg version path-1.0");
    }

    #[test]
    fn falls_back_to_app_dir() {
        let dir = test_dir("appdir");
        fake_binary(&dir, "ffmpeg", "ffmpeg version app-2.0");
        fake_binary(&dir, "ffprobe", "ffprobe version app-2.0");
        let locator = FfmpegLocator {
            env_ffmpeg: None,
            env_ffprobe: None,
            path_var: Some(OsString::from("/nonexistent-dir-for-nipa-test")),
            app_dir: Some(dir.clone()),
        };
        let paths = locator.locate().expect("应用目录级探测应命中");
        assert_eq!(paths.ffprobe, dir.join("ffprobe"));
        assert_eq!(paths.ffprobe_version, "ffprobe version app-2.0");
    }

    #[test]
    fn missing_everywhere_returns_none() {
        let locator = FfmpegLocator {
            env_ffmpeg: None,
            env_ffprobe: None,
            path_var: Some(OsString::from("/nonexistent-dir-for-nipa-test")),
            app_dir: Some(PathBuf::from("/nonexistent-dir-for-nipa-test")),
        };
        assert!(locator.locate().is_none());
    }

    #[test]
    fn non_executable_file_rejected_by_path_search() {
        let dir = test_dir("noexec");
        let path = dir.join("ffmpeg");
        fs::write(&path, "not a binary").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let locator = FfmpegLocator {
            path_var: Some(dir.into_os_string()),
            ..Default::default()
        };
        assert!(locator.search_path("ffmpeg").is_none());
    }
}
