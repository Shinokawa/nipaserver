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
//! TODO(M3):
//! - 播放判定（借鉴 Jellyfin StreamBuilder 逻辑，不抄代码）：
//!   DeviceProfile × MediaSourceInfo → DirectPlay / Remux / Transcode；
//!   拒绝 Direct Play 时累积 TranscodeReason 标志记日志；
//! - HLS：完整 VOD m3u8 预生成、force_key_frames 4s 对齐、seek 检测 kill+重启、
//!   session 60s 无请求清理、fMP4 输出；
//! - 硬件加速 v1 仅 macOS VideoToolbox + 软编回落（§6.2）。

pub mod error;
pub mod extract;
pub mod locate;
pub mod probe;
pub mod tools;

pub use error::StreamError;
pub use extract::{default_sample_ranges, extract_subtitle_text};
pub use locate::{FfmpegLocator, FfmpegPaths};
pub use probe::{AudioInfo, MediaSummary, ProbeResult, SubtitleInfo, VideoInfo, probe};
pub use tools::build_stream_tools;

use serde::{Deserialize, Serialize};

/// 播放模式判定结果（§6.1）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMethod {
    /// 全兼容 → tower-http ServeFile 直伺服（95% 场景）。
    DirectPlay,
    /// 容器/音频不兼容 → `-c:v copy` 换封装 HLS fMP4。
    Remux,
    /// 视频编码/码率不兼容、图形字幕烧录、HDR→SDR → 完整转码。
    Transcode,
}

/// 客户端上报的设备能力占位（§6.1 DeviceProfile）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub containers: Vec<String>,
    pub video_codecs: Vec<String>,
    pub audio_codecs: Vec<String>,
    pub max_bitrate: Option<u64>,
}
