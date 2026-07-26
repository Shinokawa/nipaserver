//! 按需 fMP4 HLS session：服务端预生成完整 VOD playlist，请求 segment 时才启动
//! ffmpeg，跨越 8 段或向后 seek 时 kill + `-ss` 重启。

use std::collections::HashMap;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::{FfmpegPaths, PlayMethod, StreamError};

#[derive(Debug, Clone)]
pub struct HlsConfig {
    pub segment_duration: f64,
    pub seek_gap_segments: u32,
    pub idle_timeout: Duration,
    pub segment_wait_timeout: Duration,
}

impl Default for HlsConfig {
    fn default() -> Self {
        Self {
            segment_duration: 4.0,
            seek_gap_segments: 8,
            idle_timeout: Duration::from_secs(120),
            segment_wait_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HlsSessionSpec {
    pub source: PathBuf,
    pub duration_secs: f64,
    pub method: PlayMethod,
}

#[derive(Debug)]
pub struct SegmentData {
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

#[derive(Clone)]
pub struct HlsManager {
    inner: Arc<Inner>,
}

struct Inner {
    ffmpeg: PathBuf,
    video_toolbox: bool,
    root: PathBuf,
    config: HlsConfig,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

struct Session {
    spec: HlsSessionSpec,
    dir: PathBuf,
    last_access: Mutex<Instant>,
    process: Mutex<ProcessState>,
    ffmpeg_log: Arc<Mutex<VecDeque<String>>>,
}

struct ProcessState {
    child: Option<Child>,
    start_index: u32,
}

impl HlsManager {
    pub fn new(paths: &FfmpegPaths, config: HlsConfig) -> Result<Self, StreamError> {
        let mut random = [0u8; 12];
        getrandom::fill(&mut random).map_err(|e| StreamError::Other(e.to_string()))?;
        let root = std::env::temp_dir().join(format!(
            "nipa-hls-{}-{}",
            std::process::id(),
            hex::encode(random)
        ));
        std::fs::create_dir_all(&root)?;
        let video_toolbox =
            cfg!(target_os = "macos") && supports_encoder(&paths.ffmpeg, "h264_videotoolbox");
        let inner = Arc::new(Inner {
            ffmpeg: paths.ffmpeg.clone(),
            video_toolbox,
            root,
            config,
            sessions: Mutex::new(HashMap::new()),
        });
        spawn_reaper(Arc::downgrade(&inner));
        Ok(Self { inner })
    }

    /// 建立一个不可猜的 session id。只创建目录，此时不起 ffmpeg。
    pub async fn create_session(&self, spec: HlsSessionSpec) -> Result<String, StreamError> {
        if !spec.source.is_file() || !spec.duration_secs.is_finite() || spec.duration_secs <= 0.0 {
            return Err(StreamError::Other("无效的 HLS 媒体源或时长".into()));
        }
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|e| StreamError::Other(e.to_string()))?;
        let id = hex::encode(random);
        let dir = self.inner.root.join(&id);
        std::fs::create_dir(&dir)?;
        let session = Arc::new(Session {
            spec,
            dir,
            last_access: Mutex::new(Instant::now()),
            process: Mutex::new(ProcessState {
                child: None,
                start_index: 0,
            }),
            ffmpeg_log: Arc::new(Mutex::new(VecDeque::with_capacity(20))),
        });
        self.inner.sessions.lock().await.insert(id.clone(), session);
        Ok(id)
    }

    pub async fn has_session(&self, id: &str) -> bool {
        self.inner.sessions.lock().await.contains_key(id)
    }

    /// `query` 必须是已签名的 `exp=...&sig=...`，同样传给每个 segment URL。
    pub async fn playlist(&self, id: &str, query: &str) -> Result<String, StreamError> {
        let session = self.session(id).await?;
        touch(&session).await;
        Ok(vod_playlist(
            session.spec.duration_secs,
            self.inner.config.segment_duration,
            query,
        ))
    }

    pub async fn segment(&self, id: &str, index: i32) -> Result<SegmentData, StreamError> {
        let session = self.session(id).await?;
        touch(&session).await;
        let count = segment_count(
            session.spec.duration_secs,
            self.inner.config.segment_duration,
        );
        if index < -1 || index >= count as i32 {
            return Err(StreamError::Other(format!("segment {index} 越界")));
        }
        let target = if index == -1 {
            session.dir.join("-1.m4s")
        } else {
            session.dir.join(format!("{index}.m4s"))
        };
        if !is_ready_file(&target) {
            self.ensure_transcoding(&session, index).await?;
            wait_until_ready(&session, index, self.inner.config.segment_wait_timeout).await?;
        }
        let bytes = std::fs::read(&target)?;
        Ok(SegmentData {
            bytes,
            content_type: "video/mp4",
        })
    }

    pub async fn shutdown(&self) {
        let sessions = {
            let mut map = self.inner.sessions.lock().await;
            map.drain().map(|(_, s)| s).collect::<Vec<_>>()
        };
        for session in sessions {
            stop(&session).await;
            let _ = std::fs::remove_dir_all(&session.dir);
        }
        let _ = std::fs::remove_dir_all(&self.inner.root);
    }

    async fn session(&self, id: &str) -> Result<Arc<Session>, StreamError> {
        self.inner
            .sessions
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| StreamError::Other("未知或已过期的 HLS session".into()))
    }

    async fn ensure_transcoding(
        &self,
        session: &Arc<Session>,
        requested: i32,
    ) -> Result<(), StreamError> {
        let requested = requested.max(0) as u32;
        let mut process = session.process.lock().await;
        let running = match process.child.as_mut() {
            Some(child) => child.try_wait()?.is_none(),
            None => false,
        };
        let current = latest_segment(&session.dir);
        let restart = !running
            || requested == 0 && !session.dir.join("-1.m4s").exists()
            || current.is_some_and(|i| requested < i)
            || current
                .is_some_and(|i| requested.saturating_sub(i) > self.inner.config.seek_gap_segments);
        if !restart {
            return Ok(());
        }
        if let Some(child) = process.child.as_mut() {
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
        // ffmpeg 异常中止时最后一段可能是半写，只删它，保留其他缓存段。
        if let Some(last) = latest_segment(&session.dir) {
            let _ = std::fs::remove_file(session.dir.join(format!("{last}.m4s")));
        }
        let args = ffmpeg_args(
            &session.spec,
            &session.dir,
            requested,
            self.inner.config.segment_duration,
            self.inner.video_toolbox,
        );
        let mut child = Command::new(&self.inner.ffmpeg)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| StreamError::Spawn {
                program: self.inner.ffmpeg.display().to_string(),
                source,
            })?;
        if let Some(stderr) = child.stderr.take() {
            let log = session.ffmpeg_log.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "nipa_stream::ffmpeg", %line, "ffmpeg");
                    let mut log = log.lock().await;
                    if log.len() == 20 {
                        log.pop_front();
                    }
                    log.push_back(line);
                }
            });
        }
        process.child = Some(child);
        process.start_index = requested;
        Ok(())
    }
}

fn ffmpeg_args(
    spec: &HlsSessionSpec,
    dir: &Path,
    start: u32,
    segment_duration: f64,
    video_toolbox: bool,
) -> Vec<String> {
    let mut args = vec!["-hide_banner".into(), "-loglevel".into(), "warning".into()];
    if start > 0 {
        let mut seek = start as f64 * segment_duration;
        if spec.method == PlayMethod::Remux {
            seek += 0.5;
        }
        seek = seek.min((spec.duration_secs - 5.0).max(0.0));
        args.extend(["-ss".into(), format!("{seek:.3}")]);
    }
    args.extend(["-i".into(), spec.source.display().to_string()]);
    args.extend([
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
    ]);
    args.extend([
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
    ]);
    match spec.method {
        PlayMethod::Remux => args.extend([
            "-fflags".into(),
            "+genpts".into(),
            "-c:v".into(),
            "copy".into(),
        ]),
        PlayMethod::Transcode | PlayMethod::DirectPlay => {
            let encoder = if video_toolbox {
                "h264_videotoolbox"
            } else {
                "libx264"
            };
            args.extend([
                "-c:v".into(),
                encoder.into(),
                "-pix_fmt".into(),
                "yuv420p".into(),
            ]);
            args.extend([
                "-force_key_frames".into(),
                format!("expr:gte(t,n_forced*{segment_duration})"),
            ]);
            if encoder == "libx264" {
                args.extend([
                    "-preset".into(),
                    "veryfast".into(),
                    "-sc_threshold".into(),
                    "0".into(),
                ]);
            }
        }
    }
    // 视频转码时强制音频转码，避免 seek 后 copy audio 超前。
    args.extend([
        "-c:a".into(),
        "aac".into(),
        "-ac".into(),
        "2".into(),
        "-b:a".into(),
        "192k".into(),
    ]);
    args.extend([
        "-copyts".into(),
        "-avoid_negative_ts".into(),
        "disabled".into(),
        "-max_muxing_queue_size".into(),
        "128".into(),
        "-f".into(),
        "hls".into(),
        "-max_delay".into(),
        "5000000".into(),
        "-hls_time".into(),
        segment_duration.to_string(),
        "-hls_segment_type".into(),
        "fmp4".into(),
        "-hls_fmp4_init_filename".into(),
        "-1.m4s".into(),
        "-start_number".into(),
        start.to_string(),
        "-hls_segment_filename".into(),
        dir.join("%d.m4s").display().to_string(),
        "-hls_playlist_type".into(),
        "event".into(),
        "-hls_list_size".into(),
        "0".into(),
        "-hls_segment_options".into(),
        "movflags=+frag_discont".into(),
        "-y".into(),
        dir.join("progress.m3u8").display().to_string(),
    ]);
    args
}

fn vod_playlist(duration: f64, segment_duration: f64, query: &str) -> String {
    let count = segment_count(duration, segment_duration);
    let mut out = format!(
        "#EXTM3U\n#EXT-X-VERSION:7\n#EXT-X-PLAYLIST-TYPE:VOD\n#EXT-X-TARGETDURATION:{}\n#EXT-X-MEDIA-SEQUENCE:0\n#EXT-X-MAP:URI=\"-1.m4s?{}\"\n",
        segment_duration.ceil() as u64,
        query
    );
    for i in 0..count {
        let begin = i as f64 * segment_duration;
        let len = (duration - begin).min(segment_duration).max(0.0);
        out.push_str(&format!("#EXTINF:{len:.6},\n{i}.m4s?{query}\n"));
    }
    out.push_str("#EXT-X-ENDLIST\n");
    out
}

fn segment_count(duration: f64, segment_duration: f64) -> u32 {
    let full = (duration / segment_duration).floor();
    let remainder = duration - full * segment_duration;
    // ffprobe 常把 AAC encoder padding 算进 format.duration（几十毫秒）。若为这点
    // 尾巴单列一个 segment，fMP4 里最后一帧的默认时长会让 MSE 把总时长再撑大
    // 一个完整切片。小于 250ms 的尾差并入上一段，避免 12.02s 显示成 16s。
    let count = if full >= 1.0 && remainder <= 0.25 {
        full
    } else {
        full + 1.0
    };
    count.max(1.0) as u32
}

fn latest_segment(dir: &Path) -> Option<u32> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.strip_suffix(".m4s")?.parse::<u32>().ok()
        })
        .max()
}

fn is_ready_file(path: &Path) -> bool {
    path.metadata().is_ok_and(|m| m.is_file() && m.len() > 0)
}

async fn wait_until_ready(
    session: &Session,
    index: i32,
    timeout: Duration,
) -> Result<(), StreamError> {
    let target = if index == -1 {
        session.dir.join("-1.m4s")
    } else {
        session.dir.join(format!("{index}.m4s"))
    };
    let next = (index >= 0).then(|| session.dir.join(format!("{}.m4s", index + 1)));
    let deadline = Instant::now() + timeout;
    loop {
        let exited = {
            let mut state = session.process.lock().await;
            match state.child.as_mut() {
                Some(child) => child.try_wait()?.is_some(),
                None => true,
            }
        };
        let complete = is_ready_file(&target)
            && (index == -1 || exited || next.as_ref().is_some_and(|p| is_ready_file(p)));
        if complete {
            return Ok(());
        }
        if exited {
            let tail = session
                .ffmpeg_log
                .lock()
                .await
                .iter()
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ");
            return Err(StreamError::Other(format!(
                "ffmpeg 未生成 segment {index}: {tail}"
            )));
        }
        if Instant::now() >= deadline {
            return Err(StreamError::Other(format!("等待 segment {index} 超时")));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn touch(session: &Session) {
    *session.last_access.lock().await = Instant::now();
}

async fn stop(session: &Session) {
    let mut state = session.process.lock().await;
    if let Some(child) = state.child.as_mut() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    state.child = None;
}

fn spawn_reaper(inner: Weak<Inner>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let Some(inner) = inner.upgrade() else { break };
            let snapshot = inner
                .sessions
                .lock()
                .await
                .iter()
                .map(|(id, s)| (id.clone(), s.clone()))
                .collect::<Vec<_>>();
            for (id, session) in snapshot {
                if session.last_access.lock().await.elapsed() > inner.config.idle_timeout {
                    let removed = inner.sessions.lock().await.remove(&id);
                    if let Some(session) = removed {
                        stop(&session).await;
                        let _ = std::fs::remove_dir_all(&session.dir);
                    }
                }
            }
        }
    });
}

fn supports_encoder(ffmpeg: &Path, encoder: &str) -> bool {
    std::process::Command::new(ffmpeg)
        .arg("-encoders")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains(encoder))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playlist_covers_full_duration_and_propagates_signature() {
        let p = vod_playlist(9.25, 4.0, "exp=1&sig=abc");
        assert_eq!(p.matches("#EXTINF:").count(), 3);
        assert!(p.contains("#EXTINF:1.250000,"));
        assert_eq!(p.matches("sig=abc").count(), 4); // init + 3 segments
        assert!(p.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn encoder_padding_does_not_create_a_dust_segment() {
        let p = vod_playlist(12.023, 4.0, "exp=1&sig=abc");
        assert_eq!(p.matches("#EXTINF:").count(), 3);
        assert!(!p.contains("3.m4s"));
    }

    #[test]
    fn remux_args_seek_before_input_and_copy_video() {
        let spec = HlsSessionSpec {
            source: "/tmp/in.mkv".into(),
            duration_secs: 100.0,
            method: PlayMethod::Remux,
        };
        let args = ffmpeg_args(&spec, Path::new("/tmp/out"), 3, 4.0, false);
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < input);
        assert!(args.windows(2).any(|w| w == ["-c:v", "copy"]));
        assert!(args.windows(2).any(|w| w == ["-start_number", "3"]));
    }

    #[test]
    fn transcode_args_force_aligned_keyframes() {
        let spec = HlsSessionSpec {
            source: "/tmp/in.mkv".into(),
            duration_secs: 100.0,
            method: PlayMethod::Transcode,
        };
        let args = ffmpeg_args(&spec, Path::new("/tmp/out"), 0, 4.0, false);
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert!(args.iter().any(|a| a == "expr:gte(t,n_forced*4)"));
        assert!(
            args.windows(2)
                .any(|w| w == ["-copyts", "-avoid_negative_ts"])
        );
    }
}
