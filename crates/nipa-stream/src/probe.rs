//! ffprobe 媒体探测（§4.2 evidence / §6.1 播放判定共用）。
//!
//! `ffprobe -v quiet -print_format json -show_format -show_streams -show_chapters`
//! 一次拿全：原始 JSON 原样保留喂 evidence bundle（nipa-scanner `EvidenceParams.ffprobe`
//! 期望的正是这份原始输出），另解析出结构化摘要供播放判定与 agent 工具精简输出。

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;
use tokio::process::Command;

use crate::error::StreamError;

/// 一次 ffprobe 探测的完整结果。
#[derive(Debug, Clone)]
pub struct ProbeResult {
    /// ffprobe 原始 JSON（format + streams + chapters），喂 evidence 用。
    pub raw: Value,
    /// 解析出的结构化摘要。
    pub summary: MediaSummary,
}

/// 结构化媒体摘要。
#[derive(Debug, Clone, Default)]
pub struct MediaSummary {
    /// `format.format_name`（如 `matroska,webm`）。
    pub container: Option<String>,
    /// `format.duration`（秒）。
    pub duration_secs: Option<f64>,
    /// 第一条视频流。
    pub video: Option<VideoInfo>,
    pub audio_tracks: Vec<AudioInfo>,
    pub subtitle_tracks: Vec<SubtitleInfo>,
    /// `format.tags`（title/comment——字幕组常写明作品名，§4.2）。
    pub format_tags: BTreeMap<String, String>,
}

/// 视频流摘要。
#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub codec: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 从 pix_fmt 推断的位深（如 yuv420p10le → 10）。
    pub bit_depth: Option<u8>,
    /// HDR 粗判：color_transfer 为 smpte2084（PQ/HDR10）或 arib-std-b67（HLG）。
    pub hdr: bool,
}

/// 音频流摘要。
#[derive(Debug, Clone)]
pub struct AudioInfo {
    pub codec: String,
    pub channels: Option<u32>,
    /// `tags.language`（未标注为 `None`）。
    pub language: Option<String>,
}

/// 字幕流摘要。
#[derive(Debug, Clone)]
pub struct SubtitleInfo {
    /// 全局流索引（`streams[].index`，即 `-map 0:{index}` 用的下标）。
    pub index: u32,
    pub codec: String,
    pub language: Option<String>,
    /// 文本字幕（subrip/ass/ssa/mov_text/webvtt…）可抽取；
    /// 图形字幕（hdmv_pgs_subtitle/dvd_subtitle…）无文本可采（§4.2）。
    pub is_text: bool,
}

/// 文本字幕编码集合（可经 `-f srt` 抽取为文本）。
const TEXT_SUBTITLE_CODECS: &[&str] = &[
    "subrip", "srt", "ass", "ssa", "mov_text", "webvtt", "text", "microdvd",
];

/// 判断字幕 codec_name 是否文本字幕。未知 codec 保守按图形处理
/// （抽取路径会给出明确错误，而不是喂 agent 一段二进制乱码）。
pub fn is_text_subtitle_codec(codec: &str) -> bool {
    TEXT_SUBTITLE_CODECS.contains(&codec)
}

/// 跑 ffprobe 探测媒体文件。
///
/// `-v quiet` 抑制横幅；stdout 是纯 JSON。非零退出（文件损坏/不是媒体文件）
/// 返回 [`StreamError::CommandFailed`]，stderr 此时因 `-v quiet` 常为空——错误信息
/// 以退出码为准。
pub async fn probe(ffprobe_path: &Path, media_path: &Path) -> Result<ProbeResult, StreamError> {
    if !media_path.is_file() {
        return Err(StreamError::MediaNotFound(media_path.to_path_buf()));
    }
    let output = Command::new(ffprobe_path)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            "-show_chapters",
        ])
        .arg(media_path)
        .output()
        .await
        .map_err(|source| StreamError::Spawn {
            program: ffprobe_path.display().to_string(),
            source,
        })?;
    if !output.status.success() {
        return Err(StreamError::CommandFailed {
            program: "ffprobe".into(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let raw: Value = serde_json::from_slice(&output.stdout)?;
    let summary = summarize(&raw);
    Ok(ProbeResult { raw, summary })
}

/// 从 ffprobe 原始 JSON 解析结构化摘要。ffprobe 的数值字段常以字符串出现
/// （如 `"duration": "1420.32"`），数字/字符串两种形态都接受。
pub fn summarize(raw: &Value) -> MediaSummary {
    let format = raw.get("format");
    let mut summary = MediaSummary {
        container: format
            .and_then(|f| f.get("format_name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        duration_secs: format.and_then(|f| f.get("duration")).and_then(value_as_f64),
        format_tags: format
            .and_then(|f| f.get("tags"))
            .and_then(Value::as_object)
            .map(|tags| {
                tags.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
        ..Default::default()
    };

    for stream in raw
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let codec = stream
            .get("codec_name")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string();
        let language = stream
            .get("tags")
            .and_then(|t| t.as_object())
            .and_then(|t| {
                t.iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("language"))
                    .and_then(|(_, v)| v.as_str())
            })
            .map(str::to_string);
        match stream.get("codec_type").and_then(Value::as_str) {
            Some("video") if summary.video.is_none() => {
                // 跳过封面图伪视频流（attached_pic）。
                let attached_pic = stream
                    .get("disposition")
                    .and_then(|d| d.get("attached_pic"))
                    .and_then(Value::as_i64)
                    == Some(1);
                if attached_pic {
                    continue;
                }
                let color_transfer = stream
                    .get("color_transfer")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                summary.video = Some(VideoInfo {
                    codec,
                    width: stream.get("width").and_then(Value::as_u64).map(|w| w as u32),
                    height: stream
                        .get("height")
                        .and_then(Value::as_u64)
                        .map(|h| h as u32),
                    bit_depth: stream
                        .get("pix_fmt")
                        .and_then(Value::as_str)
                        .and_then(bit_depth_of_pix_fmt),
                    hdr: matches!(color_transfer, "smpte2084" | "arib-std-b67"),
                });
            }
            Some("audio") => summary.audio_tracks.push(AudioInfo {
                codec,
                channels: stream
                    .get("channels")
                    .and_then(Value::as_u64)
                    .map(|c| c as u32),
                language,
            }),
            Some("subtitle") => {
                let Some(index) = stream.get("index").and_then(Value::as_u64) else {
                    continue;
                };
                summary.subtitle_tracks.push(SubtitleInfo {
                    index: index as u32,
                    is_text: is_text_subtitle_codec(&codec),
                    codec,
                    language,
                });
            }
            _ => {}
        }
    }
    summary
}

impl MediaSummary {
    /// 按全局流索引查字幕轨。
    pub fn subtitle_by_index(&self, index: u32) -> Option<&SubtitleInfo> {
        self.subtitle_tracks.iter().find(|s| s.index == index)
    }
}

fn value_as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}

/// 从 pix_fmt 名推断位深：`yuv420p10le` → 10、`yuv420p` → 8。
/// 规则：取名字中最后一段数字且合理（8..=16）者；无数字段（如 `yuv420p`）按 8。
fn bit_depth_of_pix_fmt(pix_fmt: &str) -> Option<u8> {
    // p10le/p12be/p16 形态：找 'p' 后紧跟的数字。
    let bytes = pix_fmt.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'p' {
            let digits: String = pix_fmt[i + 1..]
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            if let Ok(n) = digits.parse::<u8>()
                && (8..=16).contains(&n)
            {
                return Some(n);
            }
        }
    }
    // 常见 8bit 格式无显式位深后缀。
    if pix_fmt.starts_with("yuv") || pix_fmt.starts_with("nv") || pix_fmt.starts_with("rgb") {
        Some(8)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bit_depth_parsing() {
        assert_eq!(bit_depth_of_pix_fmt("yuv420p"), Some(8));
        assert_eq!(bit_depth_of_pix_fmt("yuv420p10le"), Some(10));
        assert_eq!(bit_depth_of_pix_fmt("yuv422p12be"), Some(12));
        assert_eq!(bit_depth_of_pix_fmt("nv12"), Some(8));
        assert_eq!(bit_depth_of_pix_fmt("weird"), None);
    }

    #[test]
    fn summarize_full_shape() {
        let raw = json!({
            "format": {
                "format_name": "matroska,webm",
                "duration": "1420.32",
                "tags": { "title": "某作品 第03话", "COMMENT": "字幕组" }
            },
            "streams": [
                {
                    "index": 0, "codec_type": "video", "codec_name": "hevc",
                    "width": 1920, "height": 1080,
                    "pix_fmt": "yuv420p10le", "color_transfer": "smpte2084"
                },
                {
                    "index": 1, "codec_type": "audio", "codec_name": "aac",
                    "channels": 2, "tags": { "language": "jpn" }
                },
                {
                    "index": 2, "codec_type": "subtitle", "codec_name": "ass",
                    "tags": { "language": "chi" }
                },
                {
                    "index": 3, "codec_type": "subtitle", "codec_name": "hdmv_pgs_subtitle"
                }
            ]
        });
        let s = summarize(&raw);
        assert_eq!(s.container.as_deref(), Some("matroska,webm"));
        assert!((s.duration_secs.unwrap() - 1420.32).abs() < 1e-9);
        let v = s.video.as_ref().unwrap();
        assert_eq!(v.codec, "hevc");
        assert_eq!((v.width, v.height), (Some(1920), Some(1080)));
        assert_eq!(v.bit_depth, Some(10));
        assert!(v.hdr);
        assert_eq!(s.audio_tracks.len(), 1);
        assert_eq!(s.audio_tracks[0].language.as_deref(), Some("jpn"));
        assert_eq!(s.subtitle_tracks.len(), 2);
        assert!(s.subtitle_tracks[0].is_text);
        assert!(!s.subtitle_tracks[1].is_text);
        assert_eq!(s.subtitle_tracks[1].index, 3);
        assert_eq!(s.format_tags.get("title").unwrap(), "某作品 第03话");
        assert_eq!(s.format_tags.get("COMMENT").unwrap(), "字幕组");
    }

    #[test]
    fn summarize_tolerates_missing_fields() {
        let s = summarize(&json!({}));
        assert!(s.container.is_none());
        assert!(s.video.is_none());
        assert!(s.audio_tracks.is_empty());
        assert!(s.subtitle_tracks.is_empty());
    }
}
