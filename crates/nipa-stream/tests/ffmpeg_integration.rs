//! ffmpeg 集成测试：现场生成 fixture（testsrc2 视频 2 秒 + 中文 srt 字幕 mux 进
//! mkv），覆盖 probe 全字段、字幕抽取往返、路径越界拒绝、图形字幕报错、
//! FfmpegLocator 真机探测。
//!
//! 依赖本机 ffmpeg/ffprobe（PATH 可见）。没有 ffmpeg 的环境会以
//! "skipped (no ffmpeg)" 打印后直接通过——CI 缺 ffmpeg 时等效 ignore，
//! 本机（/opt/homebrew/bin/ffmpeg 7.x）全跑。

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use nipa_stream::{FfmpegLocator, FfmpegPaths, StreamError, build_stream_tools};
use serde_json::{Value, json};

/// 探测本机 ffmpeg；没有则 None（测试打印跳过说明后通过）。
fn ffmpeg() -> Option<&'static FfmpegPaths> {
    static PATHS: OnceLock<Option<FfmpegPaths>> = OnceLock::new();
    PATHS.get_or_init(FfmpegLocator::detect).as_ref()
}

macro_rules! require_ffmpeg {
    () => {
        match ffmpeg() {
            Some(p) => p,
            None => {
                eprintln!("skipped (no ffmpeg on this machine)");
                return;
            }
        }
    };
}

/// fixture 目录（进程内共享，进程结束由 OS 临时目录策略清理）。
fn fixture_dir() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nipa-stream-fixture-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 生成测试 mkv：testsrc2 视频 2 秒 + aac 音轨 + 一条中文 srt 字幕 mux 进 mkv，
/// 附 format tag title。全进程只生成一次。
fn fixture_mkv(paths: &FfmpegPaths) -> &'static PathBuf {
    static FILE: OnceLock<PathBuf> = OnceLock::new();
    FILE.get_or_init(|| {
        let dir = fixture_dir();
        let srt = dir.join("sub.srt");
        std::fs::write(
            &srt,
            "1\n00:00:00,100 --> 00:00:00,900\n第一句：欢迎来到测试世界\n\n\
             2\n00:00:01,000 --> 00:00:01,500\n第二句：字幕抽取往返验证\n\n\
             3\n00:00:01,600 --> 00:00:01,990\nThird line in English\n",
        )
        .unwrap();
        let mkv = dir.join("fixture.mkv");
        let status = Command::new(&paths.ffmpeg)
            .args(["-v", "error", "-y"])
            .args(["-f", "lavfi", "-i", "testsrc2=size=320x240:rate=10:duration=2"])
            .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
            .arg("-i")
            .arg(&srt)
            .args(["-map", "0:v", "-map", "1:a", "-map", "2:s"])
            .args(["-c:v", "libx264", "-pix_fmt", "yuv420p"])
            .args(["-c:a", "aac", "-c:s", "srt"])
            .args(["-metadata", "title=NipaStream 测试影片"])
            .args(["-metadata:s:a:0", "language=jpn"])
            .args(["-metadata:s:s:0", "language=chi"])
            .arg(&mkv)
            .status()
            .expect("生成 fixture mkv 失败（spawn）");
        assert!(status.success(), "生成 fixture mkv 失败：{status}");
        mkv
    })
}

// ===== FfmpegLocator =====

#[test]
fn locator_detects_local_ffmpeg() {
    let paths = require_ffmpeg!();
    assert!(paths.ffmpeg.is_file());
    assert!(paths.ffprobe.is_file());
    assert!(
        paths.ffmpeg_version.starts_with("ffmpeg version"),
        "版本行意外：{}",
        paths.ffmpeg_version
    );
    assert!(paths.ffprobe_version.starts_with("ffprobe version"));
}

// ===== probe 全字段 =====

#[tokio::test]
async fn probe_reports_full_summary() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    let result = nipa_stream::probe(&paths.ffprobe, mkv).await.unwrap();

    // 原始 JSON：evidence 期望的 -show_format -show_streams 形态。
    assert!(result.raw.get("format").is_some());
    assert!(result.raw.get("streams").is_some());
    assert!(result.raw.get("chapters").is_some());

    let s = &result.summary;
    assert!(s.container.as_deref().unwrap().contains("matroska"));
    let dur = s.duration_secs.unwrap();
    assert!((1.5..=3.5).contains(&dur), "时长意外：{dur}");

    let v = s.video.as_ref().expect("应有视频流");
    assert_eq!(v.codec, "h264");
    assert_eq!((v.width, v.height), (Some(320), Some(240)));
    assert_eq!(v.bit_depth, Some(8));
    assert!(!v.hdr);

    assert_eq!(s.audio_tracks.len(), 1);
    assert_eq!(s.audio_tracks[0].codec, "aac");
    assert_eq!(s.audio_tracks[0].channels, Some(1));
    assert_eq!(s.audio_tracks[0].language.as_deref(), Some("jpn"));

    assert_eq!(s.subtitle_tracks.len(), 1);
    let t = &s.subtitle_tracks[0];
    assert_eq!(t.index, 2);
    assert_eq!(t.codec, "subrip");
    assert_eq!(t.language.as_deref(), Some("chi"));
    assert!(t.is_text);

    assert_eq!(
        s.format_tags.get("title").map(String::as_str),
        Some("NipaStream 测试影片")
    );
}

#[tokio::test]
async fn probe_missing_file_errors() {
    let paths = require_ffmpeg!();
    let err = nipa_stream::probe(&paths.ffprobe, &fixture_dir().join("no-such.mkv"))
        .await
        .unwrap_err();
    assert!(matches!(err, StreamError::MediaNotFound(_)));
}

// ===== 字幕抽取往返 =====

#[tokio::test]
async fn subtitle_roundtrip_preserves_chinese_dialogue() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    // 全片 2 秒：单段覆盖。
    let text = nipa_stream::extract_subtitle_text(&paths.ffmpeg, mkv, 2, &[(0.0, 2.0)])
        .await
        .unwrap();
    assert!(text.contains("第一句：欢迎来到测试世界"), "抽取结果：{text}");
    assert!(text.contains("第二句：字幕抽取往返验证"));
    assert!(text.contains("Third line in English"));
    // SRT 结构应已剥净。
    assert!(!text.contains("-->"), "残留时间戳：{text}");
    assert!(
        !text.lines().any(|l| l.chars().all(|c| c.is_ascii_digit())),
        "残留序号行：{text}"
    );
}

#[tokio::test]
async fn subtitle_extract_from_video_stream_fails_cleanly() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    // 流 0 是视频轨：ffmpeg 无法转 srt，会以 CommandFailed 或 GraphicSubtitle
    // 报错（ffmpeg 报 "Subtitle encoding currently only possible from text to
    // text or bitmap to bitmap"），不 panic 不产出乱码。
    let err = nipa_stream::extract_subtitle_text(&paths.ffmpeg, mkv, 0, &[(0.0, 1.0)])
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        StreamError::GraphicSubtitle { .. } | StreamError::CommandFailed { .. }
    ));
}

// ===== agent 工具 =====

async fn call_tool(
    tools: &[std::sync::Arc<dyn nipa_agent::Tool>],
    name: &str,
    args: Value,
) -> Result<nipa_agent::ToolOutput, nipa_agent::ToolError> {
    tools
        .iter()
        .find(|t| t.name() == name)
        .unwrap_or_else(|| panic!("工具 {name} 未注册"))
        .call(args)
        .await
}

#[tokio::test]
async fn probe_media_tool_returns_slim_json() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    let tools = build_stream_tools(paths, vec![fixture_dir()]);
    assert_eq!(tools.len(), 2);

    let out = call_tool(&tools, "probe_media", json!({ "path": mkv }))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&out.content).unwrap();
    assert!(v["container"].as_str().unwrap().contains("matroska"));
    assert_eq!(v["duration_secs"].as_u64(), Some(2));
    assert_eq!(v["video"]["codec"], "h264");
    assert_eq!(v["video"]["width"], 320);
    assert_eq!(v["audio_tracks"][0]["language"], "jpn");
    let sub = &v["subtitle_tracks"][0];
    assert_eq!(sub["index"], 2);
    assert_eq!(sub["is_text"], true);
    assert_eq!(v["title"], "NipaStream 测试影片");
    // 精简校验：不应把 ffprobe 原始大 JSON 泄进 agent 上下文。
    assert!(v.get("streams").is_none());
    assert!(v.get("format").is_none());
}

#[tokio::test]
async fn extract_subtitle_tool_returns_dialogue() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    let tools = build_stream_tools(paths, vec![fixture_dir()]);

    // 默认 auto：2 秒短片收缩为单段全片。
    let out = call_tool(
        &tools,
        "extract_subtitle",
        json!({ "path": mkv, "stream_index": 2 }),
    )
    .await
    .unwrap();
    assert!(out.content.contains("欢迎来到测试世界"), "输出：{}", out.content);

    // 显式 position 也要工作。
    let out = call_tool(
        &tools,
        "extract_subtitle",
        json!({ "path": mkv, "stream_index": 2, "position": "start" }),
    )
    .await
    .unwrap();
    assert!(out.content.contains("欢迎来到测试世界"));
}

#[tokio::test]
async fn extract_subtitle_tool_rejects_non_subtitle_stream() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    let tools = build_stream_tools(paths, vec![fixture_dir()]);
    let err = call_tool(
        &tools,
        "extract_subtitle",
        json!({ "path": mkv, "stream_index": 0 }),
    )
    .await
    .unwrap_err();
    match err {
        nipa_agent::ToolError::RespondToModel(msg) => {
            assert!(msg.contains("不是字幕轨"), "错误信息：{msg}");
            assert!(msg.contains("[2]"), "应列出可用字幕轨：{msg}");
        }
        other => panic!("期望 RespondToModel，得到 {other:?}"),
    }
}

// ===== 路径越界拒绝（§8.4） =====

#[tokio::test]
async fn tools_reject_paths_outside_allowed_roots() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths).clone();
    // allowed root 指向一个与 fixture 无关的目录。
    let other_root = fixture_dir().join("allowed-empty");
    std::fs::create_dir_all(&other_root).unwrap();
    let tools = build_stream_tools(paths, vec![other_root]);

    for tool_name in ["probe_media", "extract_subtitle"] {
        let err = call_tool(
            &tools,
            tool_name,
            json!({ "path": mkv, "stream_index": 2 }),
        )
        .await
        .unwrap_err();
        match err {
            nipa_agent::ToolError::RespondToModel(msg) => {
                assert!(msg.contains("不在允许的媒体库目录"), "{tool_name}: {msg}")
            }
            other => panic!("{tool_name}: 期望 RespondToModel，得到 {other:?}"),
        }
    }
}

#[tokio::test]
async fn tools_reject_dotdot_traversal() {
    let paths = require_ffmpeg!();
    let mkv = fixture_mkv(paths);
    let root = fixture_dir().join("allowed-empty2");
    std::fs::create_dir_all(&root).unwrap();
    let tools = build_stream_tools(paths, vec![root.clone()]);
    // root/../fixture.mkv canonicalize 后逃出 root。
    let sneaky = format!("{}/../{}", root.display(), mkv.file_name().unwrap().to_str().unwrap());
    let err = call_tool(&tools, "probe_media", json!({ "path": sneaky }))
        .await
        .unwrap_err();
    assert!(matches!(err, nipa_agent::ToolError::RespondToModel(_)));
}

// ===== 图形字幕报错 =====

/// 尝试构造带 dvd_subtitle 图形字幕的 mkv。ffmpeg 无法从 srt 直接编码 dvdsub，
/// 构造失败（编译无 dvdsub encoder 等）则跳过——任务允许跳过难构造项；
/// 非字幕轨路径的错误分支已在上面覆盖，probe 侧 is_text=false 判定在单元测试覆盖。
#[tokio::test]
async fn graphic_subtitle_rejected_by_tool() {
    let paths = require_ffmpeg!();
    let dir = fixture_dir();
    let src = fixture_mkv(paths);
    let gfx = dir.join("graphic-sub.mkv");
    // srt → dvdsub 转码 mux（部分 ffmpeg 构建不带 dvdsub encoder）。
    let status = Command::new(&paths.ffmpeg)
        .args(["-v", "error", "-y"])
        .arg("-i")
        .arg(src)
        .args(["-map", "0:v", "-map", "0:s"])
        .args(["-c:v", "copy", "-c:s", "dvdsub"])
        .arg(&gfx)
        .status()
        .unwrap();
    if !status.success() {
        eprintln!("skipped (this ffmpeg build cannot encode dvdsub fixtures)");
        return;
    }

    // probe：判为图形字幕。
    let result = nipa_stream::probe(&paths.ffprobe, &gfx).await.unwrap();
    let track = result
        .summary
        .subtitle_tracks
        .first()
        .expect("应有字幕轨");
    assert!(!track.is_text, "dvd_subtitle 应判为图形字幕");
    let index = track.index;

    // 直接抽取：明确的 GraphicSubtitle 错误。
    let err = nipa_stream::extract_subtitle_text(&paths.ffmpeg, &gfx, index, &[(0.0, 1.0)])
        .await
        .unwrap_err();
    assert!(matches!(err, StreamError::GraphicSubtitle { .. }), "得到 {err}");

    // 工具层：RespondToModel 且信息可读。
    let tools = build_stream_tools(paths, vec![dir]);
    let err = call_tool(
        &tools,
        "extract_subtitle",
        json!({ "path": gfx, "stream_index": index }),
    )
    .await
    .unwrap_err();
    match err {
        nipa_agent::ToolError::RespondToModel(msg) => {
            assert!(msg.contains("图形字幕"), "错误信息：{msg}")
        }
        other => panic!("期望 RespondToModel，得到 {other:?}"),
    }
}
