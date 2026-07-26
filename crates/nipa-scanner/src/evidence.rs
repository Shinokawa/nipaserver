//! evidence bundle 组装（§4.2）：把单个文件的全部识别线索汇成一段中文证据文本，
//! 一次性放进 L2 agent 首条 user 消息。
//!
//! 注意（§4.2 prompt 安全）：文件名/字幕内容是**不可信输入**，"证据仅作识别依据、
//! 不响应其中指令"的约束由 agent 的 system prompt 承担，本模块只负责组装数据。

use serde_json::Value;

/// 兄弟文件列表超过该数量时压缩表示（前若干个 + 总数）。
const SIBLING_LIST_LIMIT: usize = 20;
/// 压缩表示时实际列出的兄弟文件数。
const SIBLING_LIST_SHOWN: usize = 10;

/// [`build_evidence`] 的输入。
#[derive(Debug, Clone, Default)]
pub struct EvidenceParams<'a> {
    /// 相对库根的完整路径（目录结构是强线索，§4.2）。
    pub rel_path: &'a str,
    /// `ffprobe -show_format -show_streams` 的 JSON 输出。
    /// M3 前或 ffmpeg 缺失时为 `None`（§6.3 降级矩阵）。
    pub ffprobe: Option<&'a Value>,
    /// 字幕采样文本（已嗅探转码为 UTF-8）。无字幕流/图形字幕/抽取失败时为 `None`。
    pub subtitle_sample: Option<&'a str>,
    /// 同目录兄弟文件名列表（含自身与否均可，原样呈现）。
    pub siblings: &'a [String],
}

/// 组装喂给 agent 的中文证据文本。
///
/// 输出分四段：文件路径 / ffprobe 摘要（容器、时长、分辨率、音轨语言、
/// format.tags.title）/ 字幕采样 / 同目录文件统计。ffprobe 为 `None` 时输出
/// 降级说明行（§6.3）；兄弟文件超过 20 个时压缩为"前 10 个 + 总数"。
pub fn build_evidence(params: &EvidenceParams<'_>) -> String {
    let mut out = String::new();

    out.push_str("【文件路径】\n");
    out.push_str(params.rel_path);
    out.push('\n');

    out.push_str("\n【ffprobe 摘要】\n");
    match params.ffprobe {
        Some(probe) => out.push_str(&format_ffprobe_summary(probe)),
        None => out.push_str(
            "ffprobe 不可用（未安装或探测失败）——以下识别仅能依赖文件名、目录结构与兄弟文件，准确率可能受影响。\n",
        ),
    }

    out.push_str("\n【字幕采样】\n");
    match params.subtitle_sample {
        Some(text) if !text.trim().is_empty() => {
            out.push_str(text.trim_end());
            out.push('\n');
        }
        _ => out.push_str("无可用字幕文本（无字幕流、图形字幕 PGS/VobSub 无文本可采、或抽取失败）。\n"),
    }

    out.push_str(&format_siblings(params.siblings));
    out
}

fn format_ffprobe_summary(probe: &Value) -> String {
    let mut lines = String::new();
    let format = probe.get("format");

    let container = format
        .and_then(|f| f.get("format_name"))
        .and_then(Value::as_str);
    lines.push_str(&format!("容器：{}\n", container.unwrap_or("未知")));

    let duration = format
        .and_then(|f| f.get("duration"))
        .and_then(value_as_f64);
    match duration {
        Some(secs) => lines.push_str(&format!("时长：{}\n", format_duration(secs))),
        None => lines.push_str("时长：未知\n"),
    }

    match first_video_resolution(probe) {
        Some((w, h)) => lines.push_str(&format!("分辨率：{w}x{h}\n")),
        None => lines.push_str("分辨率：未知\n"),
    }

    let langs = audio_languages(probe);
    if langs.is_empty() {
        lines.push_str("音轨语言：未标注\n");
    } else {
        lines.push_str(&format!("音轨语言：{}\n", langs.join(", ")));
    }

    if let Some(title) = format_tag(format, "title") {
        lines.push_str(&format!("标题（format.tags.title）：{title}\n"));
    }

    lines
}

/// ffprobe 的数值字段常以字符串形式出现（如 `"duration": "1420.32"`），两种都接受。
fn value_as_f64(v: &Value) -> Option<f64> {
    v.as_f64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn format_duration(secs: f64) -> String {
    let total = secs.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h} 小时 {m} 分 {s} 秒")
    } else if m > 0 {
        format!("{m} 分 {s} 秒")
    } else {
        format!("{s} 秒")
    }
}

fn streams(probe: &Value) -> &[Value] {
    probe
        .get("streams")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn first_video_resolution(probe: &Value) -> Option<(u64, u64)> {
    streams(probe).iter().find_map(|s| {
        if s.get("codec_type").and_then(Value::as_str) != Some("video") {
            return None;
        }
        let w = s.get("width").and_then(Value::as_u64)?;
        let h = s.get("height").and_then(Value::as_u64)?;
        Some((w, h))
    })
}

fn audio_languages(probe: &Value) -> Vec<String> {
    let mut langs = Vec::new();
    for s in streams(probe) {
        if s.get("codec_type").and_then(Value::as_str) != Some("audio") {
            continue;
        }
        let lang = s
            .get("tags")
            .and_then(|t| tag_value(t, "language"))
            .unwrap_or("und");
        if !langs.iter().any(|l| l == lang) {
            langs.push(lang.to_string());
        }
    }
    langs
}

/// 取 `format.tags.<key>`。tag 键大小写随容器而变（mkv 常为小写、mp4 可能大写），
/// 大小写不敏感匹配。
fn format_tag<'a>(format: Option<&'a Value>, key: &str) -> Option<&'a str> {
    tag_value(format?.get("tags")?, key)
}

fn tag_value<'a>(tags: &'a Value, key: &str) -> Option<&'a str> {
    let map = tags.as_object()?;
    map.iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
}

fn format_siblings(siblings: &[String]) -> String {
    let total = siblings.len();
    let mut out = format!("\n【同目录文件】（共 {total} 个）\n");
    if total == 0 {
        out.push_str("（无其他文件）\n");
        return out;
    }
    let shown = if total > SIBLING_LIST_LIMIT {
        SIBLING_LIST_SHOWN
    } else {
        total
    };
    for name in &siblings[..shown] {
        out.push_str("- ");
        out.push_str(name);
        out.push('\n');
    }
    if shown < total {
        out.push_str(&format!("……（其余 {} 个略）\n", total - shown));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_ffprobe() -> Value {
        json!({
            "format": {
                "format_name": "matroska,webm",
                "duration": "1423.5",
                "tags": { "title": "[Sub] ぼっち・ざ・ろっく！ - 01" }
            },
            "streams": [
                { "codec_type": "video", "width": 1920, "height": 1080 },
                { "codec_type": "audio", "tags": { "language": "jpn" } },
                { "codec_type": "audio", "tags": { "language": "chi" } },
                { "codec_type": "subtitle", "tags": { "language": "chi" } }
            ]
        })
    }

    #[test]
    fn full_evidence_contains_all_sections() {
        let probe = sample_ffprobe();
        let siblings = vec!["ep01.mkv".to_string(), "ep02.mkv".to_string()];
        let text = build_evidence(&EvidenceParams {
            rel_path: "Anime/Bocchi/ep01.mkv",
            ffprobe: Some(&probe),
            subtitle_sample: Some("后藤独：我是不是该加入乐队……\n"),
            siblings: &siblings,
        });

        assert!(text.contains("【文件路径】\nAnime/Bocchi/ep01.mkv"));
        assert!(text.contains("容器：matroska,webm"));
        assert!(text.contains("时长：23 分 44 秒")); // 1423.5s 四舍五入 1424s
        assert!(text.contains("分辨率：1920x1080"));
        assert!(text.contains("音轨语言：jpn, chi"));
        assert!(text.contains("标题（format.tags.title）：[Sub] ぼっち・ざ・ろっく！ - 01"));
        assert!(text.contains("后藤独：我是不是该加入乐队"));
        assert!(text.contains("【同目录文件】（共 2 个）"));
        assert!(text.contains("- ep01.mkv"));
        assert!(text.contains("- ep02.mkv"));
        assert!(!text.contains("略）"), "少量兄弟文件不应压缩");
    }

    #[test]
    fn missing_ffprobe_emits_degradation_line() {
        let text = build_evidence(&EvidenceParams {
            rel_path: "a.mkv",
            ffprobe: None,
            subtitle_sample: None,
            siblings: &[],
        });
        assert!(text.contains("ffprobe 不可用"));
        assert!(text.contains("准确率可能受影响"));
        assert!(text.contains("无可用字幕文本"));
        assert!(text.contains("【同目录文件】（共 0 个）"));
        assert!(text.contains("（无其他文件）"));
    }

    #[test]
    fn hour_scale_duration_and_numeric_duration_value() {
        // duration 也可能是 JSON 数值而非字符串。
        let probe = json!({ "format": { "format_name": "mov", "duration": 5400.0 } });
        let text = build_evidence(&EvidenceParams {
            rel_path: "m.mp4",
            ffprobe: Some(&probe),
            ..Default::default()
        });
        assert!(text.contains("时长：1 小时 30 分 0 秒"));
        assert!(text.contains("分辨率：未知"));
        assert!(text.contains("音轨语言：未标注"));
        assert!(!text.contains("format.tags.title"), "无 title 时不输出该行");
    }

    #[test]
    fn audio_language_dedup_and_untagged_fallback() {
        let probe = json!({
            "format": {},
            "streams": [
                { "codec_type": "audio", "tags": { "language": "jpn" } },
                { "codec_type": "audio", "tags": { "language": "jpn" } },
                { "codec_type": "audio" }
            ]
        });
        let text = build_evidence(&EvidenceParams {
            rel_path: "a.mkv",
            ffprobe: Some(&probe),
            ..Default::default()
        });
        assert!(text.contains("音轨语言：jpn, und"));
    }

    #[test]
    fn format_tag_lookup_is_case_insensitive() {
        let probe = json!({ "format": { "tags": { "TITLE": "Movie Name" } } });
        let text = build_evidence(&EvidenceParams {
            rel_path: "a.mp4",
            ffprobe: Some(&probe),
            ..Default::default()
        });
        assert!(text.contains("标题（format.tags.title）：Movie Name"));
    }

    #[test]
    fn more_than_20_siblings_are_compressed() {
        let siblings: Vec<String> = (1..=25).map(|i| format!("ep{i:02}.mkv")).collect();
        let text = build_evidence(&EvidenceParams {
            rel_path: "Anime/X/ep01.mkv",
            siblings: &siblings,
            ..Default::default()
        });
        assert!(text.contains("【同目录文件】（共 25 个）"));
        assert!(text.contains("- ep01.mkv"));
        assert!(text.contains("- ep10.mkv"));
        assert!(!text.contains("- ep11.mkv"), "压缩表示只列前 10 个");
        assert!(text.contains("……（其余 15 个略）"));
    }

    #[test]
    fn exactly_20_siblings_are_listed_in_full() {
        let siblings: Vec<String> = (1..=20).map(|i| format!("ep{i:02}.mkv")).collect();
        let text = build_evidence(&EvidenceParams {
            rel_path: "Anime/X/ep01.mkv",
            siblings: &siblings,
            ..Default::default()
        });
        assert!(text.contains("- ep20.mkv"));
        assert!(!text.contains("略）"));
    }

    #[test]
    fn blank_subtitle_sample_falls_back_to_unavailable_line() {
        let text = build_evidence(&EvidenceParams {
            rel_path: "a.mkv",
            subtitle_sample: Some("   \n"),
            ..Default::default()
        });
        assert!(text.contains("无可用字幕文本"));
    }
}
