//! nipa-stream 的两个只读 agent 工具（开发文档 §4.2 工具集）：
//! `probe_media`（evidence 不足时 agent 主动加查）与 `extract_subtitle`
//! （agent 自选采样段再读一段字幕）。
//!
//! 安全论证（§4.2 prompt 安全 + §8.4 路径安全）：
//! - 两个工具全为只读（子进程只读媒体文件，无任何写路径）；
//! - **路径校验钩子**：构造时注入 `allowed_roots`（library roots），每次调用先
//!   canonicalize 再校验落在某个 root 内——字幕/文件名是不可信输入，即使注入
//!   指令诱导 agent 传入 `/etc/passwd` 或 `../../` 逃逸路径，也会被
//!   `RespondToModel` 拒绝，工具不会变成任意文件读取器；canonicalize 同时
//!   消解 symlink 逃逸（§8.4）。
//!
//! 返回 JSON 刻意精简（相对 ffprobe 原始输出丢弃码率/编码参数等无关字段）
//! 以省 token；完整原始 JSON 只进 evidence bundle，不经 agent 上下文。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use nipa_agent::{BoxFuture, Tool, ToolError, ToolOutput};
use serde_json::{Value, json};

use crate::error::StreamError;
use crate::extract::{default_sample_ranges, extract_subtitle_text};
use crate::locate::FfmpegPaths;
use crate::probe::probe;

/// 组装 nipa-stream 工具集（ffmpeg 探测成功时挂载；探测失败走 §6.3 降级矩阵，
/// 不注册这两个工具）。
pub fn build_stream_tools(
    paths: &FfmpegPaths,
    allowed_roots: Vec<PathBuf>,
) -> Vec<Arc<dyn Tool>> {
    let guard = Arc::new(PathGuard::new(allowed_roots));
    vec![
        Arc::new(ProbeMedia {
            ffprobe: paths.ffprobe.clone(),
            guard: guard.clone(),
        }),
        Arc::new(ExtractSubtitle {
            ffmpeg: paths.ffmpeg.clone(),
            ffprobe: paths.ffprobe.clone(),
            guard,
        }),
    ]
}

// ===== 路径校验钩子（§8.4） =====

/// 路径白名单校验：canonicalize 后必须落在某个 allowed root 内。
struct PathGuard {
    /// 构造时即 canonicalize 的 roots（root 本身含 symlink 也先归一）。
    roots: Vec<PathBuf>,
}

impl PathGuard {
    fn new(allowed_roots: Vec<PathBuf>) -> Self {
        Self {
            // root canonicalize 失败（不存在）就原样保留——此时任何 canonicalize
            // 成功的候选路径都不会 starts_with 它，等效于该 root 不生效。
            roots: allowed_roots
                .into_iter()
                .map(|r| r.canonicalize().unwrap_or(r))
                .collect(),
        }
    }

    /// 校验并返回 canonicalize 后的路径。失败一律 `RespondToModel`
    /// （模型可自纠：换成 evidence 里给出的真实路径）。
    fn check(&self, raw: &str) -> Result<PathBuf, ToolError> {
        let canonical = Path::new(raw).canonicalize().map_err(|e| {
            ToolError::RespondToModel(format!("路径 `{raw}` 无法访问（{e}）"))
        })?;
        if self.roots.iter().any(|root| canonical.starts_with(root)) {
            Ok(canonical)
        } else {
            Err(ToolError::RespondToModel(format!(
                "路径 `{raw}` 不在允许的媒体库目录内，拒绝访问"
            )))
        }
    }
}

fn stream_err(e: StreamError) -> ToolError {
    // 全部回喂模型：媒体损坏/图形字幕/轨不存在都是模型可理解并绕开的失败。
    ToolError::RespondToModel(e.to_string())
}

fn arg_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    match args.get(key).and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(ToolError::RespondToModel(format!(
            "参数 `{key}` 缺失或为空，需为非空字符串"
        ))),
    }
}

// ===== probe_media =====

struct ProbeMedia {
    ffprobe: PathBuf,
    guard: Arc<PathGuard>,
}

impl Tool for ProbeMedia {
    fn name(&self) -> &str {
        "probe_media"
    }

    fn description(&self) -> &str {
        "ffprobe 探测媒体文件：容器、时长、分辨率、视频编码、音轨语言、字幕轨清单（含是否文本字幕）与容器标签。evidence 中的 ffprobe 摘要不足时使用。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "媒体文件绝对路径（必须在媒体库目录内）" }
            },
            "required": ["path"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = self.guard.check(arg_str(&arguments, "path")?)?;
            let result = probe(&self.ffprobe, &path).await.map_err(stream_err)?;
            let s = &result.summary;
            // 精简版（省 token）：不回原始 JSON，只回识别用得上的字段。
            let out = json!({
                "container": s.container,
                "duration_secs": s.duration_secs.map(|d| d.round() as u64),
                "video": s.video.as_ref().map(|v| json!({
                    "codec": v.codec,
                    "width": v.width,
                    "height": v.height,
                    "bit_depth": v.bit_depth,
                    "hdr": v.hdr,
                })),
                "audio_tracks": s.audio_tracks.iter().map(|a| json!({
                    "codec": a.codec,
                    "channels": a.channels,
                    "language": a.language,
                })).collect::<Vec<_>>(),
                "subtitle_tracks": s.subtitle_tracks.iter().map(|t| json!({
                    "index": t.index,
                    "codec": t.codec,
                    "language": t.language,
                    "is_text": t.is_text,
                })).collect::<Vec<_>>(),
                "title": s.format_tags.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("title"))
                    .map(|(_, v)| v.as_str()),
            });
            Ok(ToolOutput::json(&out))
        })
    }
}

// ===== extract_subtitle =====

struct ExtractSubtitle {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    guard: Arc<PathGuard>,
}

impl Tool for ExtractSubtitle {
    fn name(&self) -> &str {
        "extract_subtitle"
    }

    fn description(&self) -> &str {
        "抽取内挂文本字幕轨的对白文本（自动转 UTF-8，剥掉时间戳）。position 可选 start/middle/end/auto（默认 auto=前中后三段采样）。图形字幕（PGS/VobSub）无文本可抽。"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "媒体文件绝对路径（必须在媒体库目录内）" },
                "stream_index": { "type": "integer", "description": "字幕轨全局流索引（probe_media 返回的 subtitle_tracks[].index）" },
                "position": {
                    "type": "string",
                    "enum": ["start", "middle", "end", "auto"],
                    "description": "采样位置，默认 auto（前中后各一段）"
                }
            },
            "required": ["path", "stream_index"]
        })
    }

    fn call(&self, arguments: Value) -> BoxFuture<'_, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let path = self.guard.check(arg_str(&arguments, "path")?)?;
            let stream_index = arguments
                .get("stream_index")
                .and_then(Value::as_u64)
                .ok_or_else(|| {
                    ToolError::RespondToModel("参数 `stream_index` 缺失或不是非负整数".into())
                })? as u32;
            let position = match arguments.get("position").and_then(Value::as_str) {
                None | Some("auto") => Position::Auto,
                Some("start") => Position::Start,
                Some("middle") => Position::Middle,
                Some("end") => Position::End,
                Some(other) => {
                    return Err(ToolError::RespondToModel(format!(
                        "position `{other}` 无效，需为 start|middle|end|auto"
                    )));
                }
            };

            // 先 probe 拿时长与字幕轨类型：图形字幕在起 ffmpeg 前就明确拒绝，
            // 顺带把"轨不存在/不是字幕轨"变成可自纠的错误信息。
            let probed = probe(&self.ffprobe, &path).await.map_err(stream_err)?;
            let track = probed.summary.subtitle_by_index(stream_index).ok_or_else(|| {
                let available: Vec<u32> = probed
                    .summary
                    .subtitle_tracks
                    .iter()
                    .map(|t| t.index)
                    .collect();
                ToolError::RespondToModel(format!(
                    "流 0:{stream_index} 不是字幕轨；可用字幕轨 index：{available:?}"
                ))
            })?;
            if !track.is_text {
                return Err(stream_err(StreamError::GraphicSubtitle { stream_index }));
            }

            let duration = probed.summary.duration_secs.unwrap_or(0.0);
            let ranges = position.ranges(duration);
            let text = extract_subtitle_text(&self.ffmpeg, &path, stream_index, &ranges)
                .await
                .map_err(stream_err)?;
            if text.is_empty() {
                return Ok(ToolOutput::text("（该采样时段内没有字幕对白）"));
            }
            Ok(ToolOutput::text(text))
        })
    }
}

enum Position {
    Start,
    Middle,
    End,
    Auto,
}

impl Position {
    /// 单段位置取 default 三段中的对应段；auto 用全部三段。
    fn ranges(&self, duration_secs: f64) -> Vec<(f64, f64)> {
        let all = default_sample_ranges(duration_secs);
        match self {
            Position::Auto => all,
            Position::Start => all.first().copied().into_iter().collect(),
            Position::Middle => {
                let mid = all.len() / 2;
                all.get(mid).copied().into_iter().collect()
            }
            Position::End => all.last().copied().into_iter().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_guard_rejects_outside_root() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let guard = PathGuard::new(vec![root]);
        // /etc/hosts 真实存在但不在 temp root 内。
        let err = guard.check("/etc/hosts").unwrap_err();
        match err {
            ToolError::RespondToModel(msg) => assert!(msg.contains("不在允许的媒体库目录")),
            other => panic!("期望 RespondToModel，得到 {other:?}"),
        }
    }

    #[test]
    fn path_guard_rejects_traversal() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        let inside = root.join("nipa-guard-test-file");
        std::fs::write(&inside, "x").unwrap();
        let guard = PathGuard::new(vec![root.clone()]);
        // ../ 逃逸到 root 外的真实文件。
        let sneaky = format!("{}/../../etc/hosts", root.display());
        assert!(guard.check(&sneaky).is_err());
        // 正常路径通过。
        assert_eq!(guard.check(inside.to_str().unwrap()).unwrap(), inside);
    }

    #[test]
    fn path_guard_rejects_nonexistent() {
        let guard = PathGuard::new(vec![std::env::temp_dir()]);
        assert!(guard.check("/no/such/nipa/file.mkv").is_err());
    }

    #[test]
    fn position_ranges_pick_segments() {
        let dur = 1440.0;
        assert_eq!(Position::Auto.ranges(dur).len(), 3);
        assert_eq!(Position::Start.ranges(dur), vec![(60.0, 90.0)]);
        assert_eq!(Position::Middle.ranges(dur), vec![(720.0, 90.0)]);
        assert_eq!(Position::End.ranges(dur), vec![(1260.0, 90.0)]);
        // 短片收缩为单段后各位置都退化到同一段。
        assert_eq!(Position::Middle.ranges(100.0), vec![(0.0, 100.0)]);
    }
}
