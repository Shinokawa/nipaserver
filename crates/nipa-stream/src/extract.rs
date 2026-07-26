//! 内挂文本字幕抽取（§4.2 字幕采样：ffprobe 不能抽字幕内容，必须走 ffmpeg）。
//!
//! `ffmpeg -ss {start} -t {dur} -i file -map 0:{index} -f srt -` 把指定时段的
//! 字幕轨转成 SRT 打到 stdout；输出经 chardetng 嗅探 + encoding_rs 转 UTF-8
//! （防 GBK/BIG5/Shift-JIS 乱码喂模型，§4.2），再剥掉 SRT 序号与时间戳行、
//! 每段限 30 行对白。
//!
//! PGS/VobSub 图形字幕无文本可采（v1 不做 OCR）：ffmpeg 会报
//! "only possible from text to text or bitmap to bitmap"，映射为明确的
//! [`StreamError::GraphicSubtitle`]，evidence 侧据此标注 unavailable。

use std::path::Path;

use tokio::process::Command;

use crate::error::StreamError;

/// 每个采样段保留的最大对白行数（§4.2：前中后各 ~30 行，总量限 ~2K tokens）。
pub const MAX_LINES_PER_SEGMENT: usize = 30;

/// 抽取指定字幕轨在若干时段内的对白文本。
///
/// - `stream_index`：**全局**流索引（`-map 0:{index}`，即 ffprobe `streams[].index`）；
/// - `ranges`：`(start_secs, dur_secs)` 采样段列表（如前/中/后三段）；
/// - 返回：各段对白拼接（段间空行分隔），已转 UTF-8、剥掉序号/时间戳、限行。
///
/// 所有段都抽不出文本时返回空字符串（无对白 ≠ 错误：时段内可能恰好没有字幕）。
/// 图形字幕返回 [`StreamError::GraphicSubtitle`]。
pub async fn extract_subtitle_text(
    ffmpeg_path: &Path,
    media_path: &Path,
    stream_index: u32,
    ranges: &[(f64, f64)],
) -> Result<String, StreamError> {
    if !media_path.is_file() {
        return Err(StreamError::MediaNotFound(media_path.to_path_buf()));
    }
    let mut segments = Vec::new();
    for &(start, dur) in ranges {
        let srt = extract_one_range(ffmpeg_path, media_path, stream_index, start, dur).await?;
        let dialogue = strip_srt_to_dialogue(&srt, MAX_LINES_PER_SEGMENT);
        if !dialogue.is_empty() {
            segments.push(dialogue);
        }
    }
    Ok(segments.join("\n\n"))
}

/// 按媒体总时长生成"前/中/后各 90 秒"的默认三段采样窗口（§4.2）。
/// 短片自动收缩：窗口互相重叠或越界时去重/夹取，最短退化为单段全片。
pub fn default_sample_ranges(duration_secs: f64) -> Vec<(f64, f64)> {
    const WINDOW: f64 = 90.0;
    if duration_secs <= 0.0 {
        return vec![(0.0, WINDOW)];
    }
    if duration_secs <= WINDOW * 3.0 {
        // 短片：单段覆盖全片。
        return vec![(0.0, duration_secs)];
    }
    let starts = [
        60.0_f64.min(duration_secs / 10.0),
        duration_secs / 2.0,
        (duration_secs - 180.0).max(0.0),
    ];
    starts.iter().map(|&s| (s, WINDOW)).collect()
}

/// 抽取单个时段：`-ss` 放 `-i` 前做输入端快速 seek，`-t` 限时长。
async fn extract_one_range(
    ffmpeg_path: &Path,
    media_path: &Path,
    stream_index: u32,
    start_secs: f64,
    dur_secs: f64,
) -> Result<Vec<u8>, StreamError> {
    let output = Command::new(ffmpeg_path)
        .args(["-v", "error", "-nostdin"])
        .args(["-ss", &format_secs(start_secs)])
        .args(["-t", &format_secs(dur_secs)])
        .arg("-i")
        .arg(media_path)
        .args(["-map", &format!("0:{stream_index}"), "-f", "srt", "-"])
        .output()
        .await
        .map_err(|source| StreamError::Spawn {
            program: ffmpeg_path.display().to_string(),
            source,
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // ffmpeg 对图形字幕→srt 的报错固定形态：
        // "Subtitle encoding currently only possible from text to text or bitmap to bitmap"
        if stderr.contains("text to text") || stderr.contains("bitmap") {
            return Err(StreamError::GraphicSubtitle { stream_index });
        }
        return Err(StreamError::CommandFailed {
            program: "ffmpeg".into(),
            status: output.status.to_string(),
            stderr: stderr.trim().to_string(),
        });
    }
    Ok(output.stdout)
}

fn format_secs(secs: f64) -> String {
    // 保留毫秒精度即可；ffmpeg 接受小数秒。
    format!("{secs:.3}")
}

/// chardetng 嗅探 + encoding_rs 解码为 UTF-8（§4.2 编码嗅探）。
///
/// 内挂 mkv 字幕按规范是 UTF-8，但外挂 srt mux 进来的 GBK/BIG5/Shift-JIS
/// 内容并不少见；嗅探失败时按 UTF-8 lossy 兜底。
pub fn decode_to_utf8(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }
    // UTF-8 有效就直接用（chardetng 对短文本可能误报 legacy 编码）。
    if let Ok(s) = std::str::from_utf8(bytes) {
        return s.to_string();
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (decoded, _, _) = encoding.decode(bytes);
    decoded.into_owned()
}

/// 剥掉 SRT 结构只留对白：丢弃序号行（纯数字）、时间戳行（含 `-->`）、空行；
/// 连续重复行去重（滚动字幕/卡拉OK 常见）；限 `max_lines` 行。
fn strip_srt_to_dialogue(raw: &[u8], max_lines: usize) -> String {
    let text = decode_to_utf8(raw);
    let mut lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.contains("-->") || line.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if lines.last() == Some(&line) {
            continue;
        }
        lines.push(line);
        if lines.len() >= max_lines {
            break;
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_srt_keeps_only_dialogue() {
        let srt = "1\n00:00:01,000 --> 00:00:03,000\n第一句对白\n\n2\n00:00:04,000 --> 00:00:06,000\n第二句对白\n第二句对白\nSecond line\n";
        let out = strip_srt_to_dialogue(srt.as_bytes(), 30);
        assert_eq!(out, "第一句对白\n第二句对白\nSecond line");
    }

    #[test]
    fn strip_srt_respects_line_limit() {
        let mut srt = String::new();
        for i in 1..=50 {
            srt.push_str(&format!(
                "{i}\n00:00:0{},000 --> 00:00:09,000\n行{i}\n\n",
                i % 10
            ));
        }
        let out = strip_srt_to_dialogue(srt.as_bytes(), 30);
        assert_eq!(out.lines().count(), 30);
    }

    #[test]
    fn decode_sniffs_gbk() {
        // "你好，世界" 的 GBK 编码。
        let gbk: &[u8] = &[0xc4, 0xe3, 0xba, 0xc3, 0xa3, 0xac, 0xca, 0xc0, 0xbd, 0xe7];
        assert_eq!(decode_to_utf8(gbk), "你好，世界");
    }

    #[test]
    fn decode_passes_utf8_through() {
        assert_eq!(decode_to_utf8("こんにちは".as_bytes()), "こんにちは");
    }

    #[test]
    fn default_ranges_three_segments_for_long_media() {
        let ranges = default_sample_ranges(1440.0);
        assert_eq!(ranges.len(), 3);
        assert_eq!(ranges[0], (60.0, 90.0));
        assert_eq!(ranges[1], (720.0, 90.0));
        assert_eq!(ranges[2], (1260.0, 90.0));
    }

    #[test]
    fn default_ranges_collapse_for_short_media() {
        let ranges = default_sample_ranges(120.0);
        assert_eq!(ranges, vec![(0.0, 120.0)]);
    }
}
