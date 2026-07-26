//! DeviceProfile × MediaSource 的最小播放判定器（开发文档 §6.1）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    /// NipaPlay 的 mpv/mdk 内核优先直放。
    Nipa,
    /// 浏览器使用保守白名单。
    #[default]
    Web,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceProfile {
    #[serde(alias = "Client")]
    pub client: ClientKind,
    #[serde(alias = "Containers")]
    pub containers: Vec<String>,
    #[serde(alias = "VideoCodecs")]
    pub video_codecs: Vec<String>,
    #[serde(alias = "AudioCodecs")]
    pub audio_codecs: Vec<String>,
    #[serde(alias = "MaxBitrate", alias = "MaxStreamingBitrate")]
    pub max_bitrate: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct MediaSource {
    pub container: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub bitrate: Option<u64>,
    pub hdr: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayMethod {
    DirectPlay,
    Remux,
    Transcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscodeReason {
    ContainerNotSupported,
    VideoCodecNotSupported,
    AudioCodecNotSupported,
    ContainerBitrateExceedsLimit,
    HdrNotSupported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayDecision {
    pub method: PlayMethod,
    pub reasons: Vec<TranscodeReason>,
    /// HLS 输出视频是 copy 还是编码。
    pub copy_video: bool,
}

/// 判定视频播放方式。未上报 profile 的 Web 端使用 mp4/webm +
/// h264/av1 + aac/opus 的保守白名单；NipaPlay 始终直放。
pub fn decide_video(profile: &DeviceProfile, source: &MediaSource) -> PlayDecision {
    if profile.client == ClientKind::Nipa {
        return PlayDecision {
            method: PlayMethod::DirectPlay,
            reasons: vec![],
            copy_video: true,
        };
    }

    let containers = defaulted(&profile.containers, &["mp4", "webm"]);
    let video_codecs = defaulted(&profile.video_codecs, &["h264", "av1"]);
    let audio_codecs = defaulted(&profile.audio_codecs, &["aac", "opus"]);
    let container = normalize_container(&source.container);
    let mut reasons = Vec::new();
    if !matches_any(&containers, &container) {
        reasons.push(TranscodeReason::ContainerNotSupported);
    }
    if let Some(codec) = &source.video_codec
        && !matches_any(&video_codecs, codec)
    {
        reasons.push(TranscodeReason::VideoCodecNotSupported);
    }
    if let Some(codec) = &source.audio_codec
        && !matches_any(&audio_codecs, codec)
    {
        reasons.push(TranscodeReason::AudioCodecNotSupported);
    }
    if source.bitrate.unwrap_or(40_000_000) > profile.max_bitrate.unwrap_or(u64::MAX) {
        reasons.push(TranscodeReason::ContainerBitrateExceedsLimit);
    }
    if source.hdr {
        reasons.push(TranscodeReason::HdrNotSupported);
    }
    if reasons.is_empty() {
        return PlayDecision {
            method: PlayMethod::DirectPlay,
            reasons,
            copy_video: true,
        };
    }

    // v1 最常见的 remux 路径：mkv/hevc/dts -> fMP4, video copy + AAC。
    // HDR 与码率超限必须转视频，不能用 remux 规避。
    let video_copyable = source
        .video_codec
        .as_deref()
        .is_some_and(|c| matches!(c.to_ascii_lowercase().as_str(), "h264" | "hevc"));
    let must_encode = reasons.iter().any(|r| {
        matches!(
            r,
            TranscodeReason::VideoCodecNotSupported
                | TranscodeReason::ContainerBitrateExceedsLimit
                | TranscodeReason::HdrNotSupported
        )
    });
    let method = if video_copyable && !must_encode {
        PlayMethod::Remux
    } else {
        PlayMethod::Transcode
    };
    PlayDecision {
        method,
        reasons,
        copy_video: method == PlayMethod::Remux,
    }
}

fn defaulted(values: &[String], defaults: &[&str]) -> Vec<String> {
    if values.is_empty() {
        defaults.iter().map(|s| (*s).to_string()).collect()
    } else {
        values.iter().map(|s| s.to_ascii_lowercase()).collect()
    }
}

/// 把 ffprobe 的逗号容器列表收敛成 API 使用的单一常见扩展名。
pub fn normalize_container(input: &str) -> String {
    input
        .split(',')
        .map(str::trim)
        .find_map(|c| match c {
            "matroska" => Some("mkv"),
            "mov" | "m4v" | "3gp" | "3g2" | "mj2" => Some("mp4"),
            _ if !c.is_empty() => Some(c),
            _ => None,
        })
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn matches_any(allowed: &[String], value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    allowed.iter().any(|list| {
        list.split(',')
            .map(str::trim)
            .any(|candidate| candidate == value)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(container: &str, video: &str, audio: &str) -> MediaSource {
        MediaSource {
            container: container.into(),
            video_codec: Some(video.into()),
            audio_codec: Some(audio.into()),
            bitrate: Some(8_000_000),
            hdr: false,
        }
    }

    #[test]
    fn nipa_is_always_direct() {
        let profile = DeviceProfile {
            client: ClientKind::Nipa,
            ..Default::default()
        };
        assert_eq!(
            decide_video(&profile, &source("mkv", "hevc", "dts")).method,
            PlayMethod::DirectPlay
        );
    }

    #[test]
    fn web_default_direct_whitelist() {
        assert_eq!(
            decide_video(&DeviceProfile::default(), &source("mp4", "h264", "aac")).method,
            PlayMethod::DirectPlay
        );
    }

    #[test]
    fn ffprobe_container_list_is_normalized_to_one_name() {
        assert_eq!(normalize_container("mov,mp4,m4a,3gp,3g2,mj2"), "mp4");
        assert_eq!(normalize_container("matroska,webm"), "mkv");
    }

    #[test]
    fn container_or_audio_only_can_remux() {
        let d = decide_video(
            &DeviceProfile::default(),
            &source("matroska,webm", "h264", "dts"),
        );
        assert_eq!(d.method, PlayMethod::Remux);
        assert!(d.reasons.contains(&TranscodeReason::ContainerNotSupported));
        assert!(d.reasons.contains(&TranscodeReason::AudioCodecNotSupported));
    }

    #[test]
    fn unsupported_video_transcodes() {
        let d = decide_video(&DeviceProfile::default(), &source("mkv", "vp9", "opus"));
        assert_eq!(d.method, PlayMethod::Transcode);
        let hevc = decide_video(&DeviceProfile::default(), &source("mkv", "hevc", "dts"));
        assert_eq!(hevc.method, PlayMethod::Transcode);
    }

    #[test]
    fn profile_declared_hevc_can_remux() {
        let profile = DeviceProfile {
            video_codecs: vec!["h264,hevc".into()],
            ..Default::default()
        };
        let d = decide_video(&profile, &source("mkv", "hevc", "dts"));
        assert_eq!(d.method, PlayMethod::Remux);
    }

    #[test]
    fn bitrate_and_hdr_force_transcode() {
        let profile = DeviceProfile {
            max_bitrate: Some(5_000_000),
            ..Default::default()
        };
        let mut s = source("mp4", "h264", "aac");
        s.bitrate = Some(8_000_000);
        s.hdr = true;
        assert_eq!(decide_video(&profile, &s).method, PlayMethod::Transcode);
    }
}
