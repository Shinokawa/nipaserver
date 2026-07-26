//! nipa-stream：ffprobe/ffmpeg sidecar、播放判定、HLS session（开发文档 §6）。
//!
//! M3 前奏（探测层，不做转码）已落地：
//! - [`locate`]：ffmpeg/ffprobe 启动探测（环境变量 → PATH → 应用目录，§6.3；
//!   缺失触发降级矩阵——仅 Direct Play、evidence 退化）；
//! - [`probe`]：ffprobe 媒体探测（原始 JSON 喂 evidence + 结构化摘要，§4.2/§6.1）；
//! - [`extract`]：内挂文本字幕抽取（编码嗅探转 UTF-8、前中后采样，§4.2）；
//! - [`tools`]：`probe_media` / `extract_subtitle` 两个只读 agent 工具
//!   （带 §8.4 路径校验钩子）。
//!
//! M3 提供纯函数播放判定与按需 HLS session；HTTP 签名和路径边界留在
//! `nipa-server`，避免媒体 crate 依赖 axum/sqlx。

pub mod error;
pub mod extract;
pub mod hls;
pub mod locate;
pub mod playback;
pub mod probe;
pub mod tools;

pub use error::StreamError;
pub use extract::{default_sample_ranges, extract_subtitle_text};
pub use hls::{HlsConfig, HlsManager, HlsSessionSpec, SegmentData};
pub use locate::{FfmpegLocator, FfmpegPaths};
pub use playback::{
    ClientKind, DeviceProfile, MediaSource, PlayDecision, PlayMethod, TranscodeReason,
    decide_video, normalize_container,
};
pub use probe::{AudioInfo, MediaSummary, ProbeResult, SubtitleInfo, VideoInfo, probe};
pub use tools::build_stream_tools;
