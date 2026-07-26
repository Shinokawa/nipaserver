//! nipa-stream：ffprobe/ffmpeg sidecar、播放判定、HLS session（开发文档 §6）。
//!
//! 当前为 M0 stub，不引入 ffmpeg 相关依赖（sidecar 子进程模式，§2.3/§6.3）。
//!
//! TODO(M3):
//! - ffmpeg/ffprobe 启动探测：环境变量 → PATH → 应用目录附带；缺失降级矩阵（§6.3）；
//! - 播放判定（借鉴 Jellyfin StreamBuilder 逻辑，不抄代码）：
//!   DeviceProfile × MediaSourceInfo → DirectPlay / Remux / Transcode；
//!   拒绝 Direct Play 时累积 TranscodeReason 标志记日志；
//! - HLS：完整 VOD m3u8 预生成、force_key_frames 4s 对齐、seek 检测 kill+重启、
//!   session 60s 无请求清理、fMP4 输出；
//! - 硬件加速 v1 仅 macOS VideoToolbox + 软编回落（§6.2）。

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

/// ffprobe 摘要占位（§4.2 evidence 与 §6.1 判定共用）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaProbe {
    pub container: Option<String>,
    pub duration_ms: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}
