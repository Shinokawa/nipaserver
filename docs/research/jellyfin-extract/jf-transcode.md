# Jellyfin 转码执行层精读报告（供 nipa-stream Rust 实现参考）

来源（GPL，仅提炼逻辑/参数，勿翻译代码）：
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/Jellyfin.Api/Controllers/DynamicHlsController.cs`（命令行组装 + seek 状态机）
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Controller/MediaEncoding/EncodingHelper.cs`（编码参数/硬件加速/滤镜链）
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.MediaEncoding/Transcoding/TranscodeManager.cs`（job 生命周期）
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.Controller/MediaEncoding/TranscodingThrottler.cs`、`TranscodingSegmentCleaner.cs`、`TranscodingJob.cs`
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/src/Jellyfin.MediaEncoding.Hls/Playlist/DynamicHlsPlaylistGenerator.cs`（m3u8 生成）
- `/Users/sakiko/Desktop/nipaserver/reference/jellyfin/MediaBrowser.MediaEncoding/Encoder/EncoderValidator.cs`（能力探测）

---

## 1. HLS 动态转码的 ffmpeg 命令行组装

### 1.1 总体结构（GetCommandLineArguments 的骨架）

命令行按固定顺序拼接，模板等价于：

```
ffmpeg {input_modifier} {input_args} \
  -map_metadata -1 -map_chapters -1 \
  -threads {N} {map_args} \
  {video_args} {audio_args} \
  -copyts -avoid_negative_ts disabled \
  -max_muxing_queue_size 128 \
  -f hls -max_delay 5000000 \
  -hls_time {seg_len} -hls_segment_type {mpegts|fmp4 ...} \
  -start_number {segId} \
  -hls_segment_filename "{prefix}%d{.ts|.mp4}" \
  -hls_playlist_type {event|vod} -hls_list_size 0 \
  [-hls_segment_options movflags=+frag_discont] \
  -y "{playlist.m3u8}"
```

关键点：
- **`-copyts -avoid_negative_ts disabled` 是 HLS seek 后时间戳正确的核心**：ffmpeg 输出的 segment 时间戳保持源时间轴，seek 重启后新 segment 的 PTS 与播放器时间轴对齐。
- `-map_metadata -1 -map_chapters -1` 丢弃元数据/章节，防止污染输出。
- `-threads N`：`min(cpu_core_limit, 逻辑核数)`，0 = 自动。
- `-max_muxing_queue_size 128`（可配置，最小 128）。
- `-max_delay 5000000`（5s，微秒）。

### 1.2 输入 seek（-ss，放在 -i 之前 = fast seek）

`GetFastSeekCommandLineParameter` 逻辑：
- `startTimeTicks > 0` 时才输出 `-ss {time}`，**位置在 `-i` 之前**（input modifier 尾部）。
- **remux（`-c:v copy`）时给 seek 加 0.5s 偏移**：`seekTick = time + 5_000_000 ticks`。原因：ffmpeg `-ss` 会 seek 到前一个关键帧，直连模式 segment 本来就以关键帧切分，加 0.5s 使其命中"恰好这个关键帧"，字幕同步更准。转码模式不加偏移（decoder 精确丢帧）。
- **Clamp 到 `[0, duration - 5s]`**：防止 seek 超出 EOF 导致 muxer 收不到包报错。
- tick 单位 = 100ns（1s = 10^7 ticks），输出格式为秒（小数）。
- 有外挂字幕/外挂音频输入时，`-ss` 需要对每个 `-i` 各重复一次（每路输入独立 seek）。

input modifier 其他要素（按需）：
- `-analyzeduration X -probesize Y`（问题文件）。
- `-fflags +genpts`：**只要视频是 copy codec 就加**（remux 必备）；`+igndts/+ignidx/+discardcorrupt` 按源标志。
- 有硬解时输入前显式 `-f {容器格式}`。
- 硬解时输入尾部加 `-noautoscale`（禁止自动插入 sw scaler）。

### 1.3 切片参数

- `hls_time` = SegmentLength：**转码 = 3s；remux（copy）= 6s**（Apple 设备 UA 也是 6）；client 可覆盖。
- `-start_number {segId}`：seek 重启时从请求的 segment 序号开始编号，playlist 客户端侧是预先算好的（见 1.6），文件名连续性由此保证。
- **force_key_frames 表达式**（`GetHlsVideoKeyFrameArguments`，仅转码时）：
  - 基础：`-force_key_frames:0 "expr:gte(t,n_forced*{seg_len})"`
  - GOP 限制：`-g:v:0 {ceil(seg_len*fps)} -keyint_min:v:0 {同值}` —— 防止编码器因场景切换插入关键帧后，下一个强制关键帧落到 segment 边界之外导致 seek 破碎。
  - 按编码器分三类：
    * **只用 GOP**（不支持 force_key_frames）：`h264/hevc/av1_nvenc`、`h264/hevc/av1_qsv`、`h264/av1_amf`、`h264/hevc_rkmpp`、`libsvtav1`；
    * **只用 keyFrameArg**：`libx264`（另加 `-sc_threshold:v:0 0` 禁场景切换插关键帧）、`libx265`、`h264/hevc/av1_vaapi`；
    * **其余（含 videotoolbox）**：keyFrameArg + gopArg 都加。

### 1.4 fMP4 init segment 处理

- segmentContainer == "mp4" 时：`-hls_segment_type fmp4 -hls_fmp4_init_filename "{basename}-1.mp4"`（Windows 用全路径，Unix 只用文件名，ffmpeg 会写到 m3u8 同目录）。
- **init 文件命名成 "{basename}-1.mp4"，即 index=-1**：segment 请求 URL 中 `segmentId == -1` 表示请求 init segment，controller 视作"必须从 0 开始转码"。
- 有视频输出时追加 `-hls_segment_options movflags=+frag_discont`（ffmpeg>=5.0；旧版用 `-hls_ts_options`）——让 fMP4 把音频初始 delay 写进 TFDT，否则 seek 后音画不同步。
- m3u8（客户端下发的）：fMP4 时 `#EXT-X-VERSION:7` + `#EXT-X-MAP:URI="hls1/main/-1.mp4?...&runtimeTicks=0&actualSegmentLengthTicks=0"`；ts 时 VERSION:3。
- HEVC 输出打 tag：`-tag:v:0 hvc1`（Safari 需要 hvc1 而非 hev1）。

### 1.5 音频参数（GetAudioArguments）

带视频的 HLS：
- copy：`-codec:a:0 copy` (+ 可能的 bsf)。
- 转码：`-codec:a:0 {aac|libfdk_aac|aac_at...}`，编码器选择：aac 优先 `aac_at`（macOS）> `libfdk_aac` > `aac`；mp3→`libmp3lame`，opus→`libopus`。
- `-ac {channels}`：HLS 只允许 1/2/6/8 声道（Apple HLS 规范），5→6、7→8、其他怪布局→2。
- 码率：VBR 开启时用 `GetAudioVbrModeParam`（aac_at: `-aac_at_mode:a 2 -b:a N`；libfdk: `-vbr:a 1..5` 按每声道码率分档），否则 `-ab {bitrate}`；无损（flac/alac）不给码率。
- `-ar {rate}`；**ac-4 源强制 `-ar 48000`**（怪采样率会炸大多数编码器）。
- opus/dts/truehd 进 mp4 muxer 需 `-strict -2`。
- bsf：源容器是 ts/aac/hls 且 AAC 且输出 fmp4 → `-bsf:a aac_adtstoasc`。
- **音频 copy + 视频转码 + seek 的音画对齐**（HlsAudioSeekStrategy）：
  - 策略 A（默认 TrimCopiedAudio）：`-bsf:a "noise=drop='lt(pts*tb\,{seek_seconds:F3})'"` —— 用 noise bsf 丢掉 seek 点之前的 copy 音频包（ffmpeg>=5.0），因为 copy 音频不走 decoder，会从前一个关键帧开始多出几秒。
  - 策略 B（TranscodeAudio）：视频转码时禁止音频 copy，直接重编码。
  - **这是 seek 后音频超前/滞后 bug 的根源，Rust 实现必须二选一**。
- 纯音频 HLS：`-vn -acodec ...` 结构类似。
- 2 声道下混：可选 `-af "volume=2"` 类 boost 与专用下混滤镜。

### 1.6 播放列表策略（重要架构决策）

Jellyfin **不让 ffmpeg 的 m3u8 直接给客户端**——`main.m3u8` 由服务端根据总时长“纸上计算”生成：
- 等长切分：`floor(duration/seg_len)` 个整段 + 余数尾段；每行 `#EXTINF:{len:0.000000}, nodesc` + URL 携带 `runtimeTicks`（该段起点）与 `actualSegmentLengthTicks`（该段时长）。
- remux 时可选用**关键帧提取**（mkv/mp4 元数据读关键帧时间）按关键帧切分，避免 copy 模式切不准。
- 帧率为分数（23.976）且转码时，segment 长度微调：`seg_len * ceil(fps)/fps`。
- ffmpeg 侧的 m3u8（playlistPath）只作为"转码进度探针"（数 `#EXTINF` 行数）。
- **好处**：客户端拿到完整 VOD playlist 可任意 seek，seek 变成"请求任意 segment"，服务端负责按需启动转码。nipa-stream 应采用同样设计。

---

## 2. seek 时 kill/重启转码 job 的状态机（GetDynamicSegment）

请求 `GET /hls1/{playlistId}/{segmentId}.{ext}?runtimeTicks=...` 时：

```
1. segment 文件已存在？ → 直接返回（job.ActiveRequestCount++）
2. 取 per-playlist 异步锁（keyed lock on playlistPath）
3. 锁内再查一次文件存在（double-check）
4. 计算 currentTranscodingIndex：
   - 活跃 job 不存在或已退出 → null
   - 否则扫目录中前缀匹配的 segment 文件，取 mtime 最新者的序号
5. 判定 startTranscoding：
   a. segmentId == -1（init 请求）→ 重启，segId 置 0
   b. currentIndex == null → 启动
   c. segmentId < currentIndex → 重启（ffmpeg 不能倒着转）
   d. segmentId - currentIndex > gap → 重启（向前 seek 太远，不如跳过去）
      gap = 24 / segmentLength   （即 24 秒内容量：3s 段=8 个，6s 段=4 个）
6. 若重启：
   - KillTranscodingJobs(deviceId/playSessionId, deleteFiles=false)  ← 不删已有文件！
   - 删除"最后一个（可能残缺的）segment 文件"
   - streamingRequest.StartTimeTicks = runtimeTicks（用请求段的起点作为 -ss）
   - state.WaitForPath = segmentPath（StartFfMpeg 等待此文件出现而非 m3u8）
   - StartFfMpeg(..., GetCommandLineArguments(..., startNumber=segmentId))
7. 若不重启（顺序播放命中正在转码的区间）：
   - job.ActiveRequestCount++；若被 throttle 暂停则 UnpauseTranscoding()
8. 返回 segment（见下）
```

**segment 就绪判定（GetSegmentResult）**——避免读到半写文件：
- job 已退出 → 文件存在即视为完整；
- `segmentIndex < currentTranscodingIndex` → 转码点已越过，视为完整；
- 否则轮询（100ms）：`segment 存在 && (job 已退出 || 下一个 segment 文件已出现)` 才返回 —— **"下一段已出现"是"这段写完了"的代理信号**，比 watch 文件大小可靠。
- 响应完成回调里：`job.DownloadPositionTicks = max(旧值, runtimeTicks + actualSegmentLengthTicks)`（喂给 throttler）+ `OnTranscodeEndRequest`（ActiveRequestCount--，归零则启动 kill 计时器）。

**Kill 流程（TranscodingJob.Stop）**：先向 ffmpeg stdin 写 `q\n`（优雅退出，flush 文件），`WaitForExit(5000)` 超时才硬 kill。

**幽灵 job 兜底计时器**：HLS job 的 ping timeout = 60s（Progressive 10s）。每次 segment 请求/客户端 progress 上报都会重置。超时无 ping → kill job + 删临时文件。客户端切走/关闭不告而别时靠这个回收。

---

## 3. 硬件加速参数矩阵

### 3.1 探测方式（EncoderValidator，全部用子进程输出解析，无试转）

| 探测项 | 命令 | 解析 |
|---|---|---|
| 版本 | `ffmpeg -version` | 正则取 x.y.z，门控各特性 |
| 编码器 | `ffmpeg -encoders` | 正则 `^ V..... (\S+)` 匹配所需清单 |
| 解码器 | `ffmpeg -decoders` | 同上 |
| hwaccel | `ffmpeg -hwaccels` | 按行拆，跳过首行 |
| 滤镜 | `ffmpeg -filters` | 匹配所需清单（tonemap_videotoolbox、scale_vt、alphasrc 等） |
| 滤镜选项 | `ffmpeg -h filter=X` | 输出含选项名字符串 |
| VAAPI 设备 | `ffmpeg -v verbose -init_hw_device vaapi=va:/dev/dri/renderD128` | stderr 含驱动名（iHD/i965/radeonsi） |
| hwaccel flag | `-loglevel quiet -hwaccel_flags +X -f lavfi -i nullsrc=s=1x1:d=100 -f null -` | 退出码 |
| 暂停键支持 | `-f lavfi -i nullsrc=s=1x1:d=N -f null -` 输出是否含 p 键提示 | 字符串匹配 |

结果缓存为能力集：`SupportsEncoder(name)` / `SupportsHwaccel(name)` / `SupportsFilter(name)` / `EncoderVersion`。

**编码器选择**：hw 类型→`{codec}_{suffix}` 映射（`h264_videotoolbox`、`hevc_nvenc`…），`SupportsEncoder` 通过才用，否则回落 `libx264`/`libx265`/`libsvtav1`。

### 3.2 解码端参数（输入侧，GetHwaccelType 提炼）

| 平台 | 解码参数（hw surface 全程管线时附加 `-hwaccel_output_format X -noautorotate`） |
|---|---|
| VideoToolbox | `-hwaccel videotoolbox [-hwaccel_output_format videotoolbox_vld] -noautorotate` |
| NVDEC | `-hwaccel cuda [-hwaccel_output_format cuda] [-hwaccel_flags +unsafe_output] -threads 1`（nvdec 无多线程） |
| VAAPI | `-hwaccel vaapi [-hwaccel_output_format vaapi]`（h264 baseline 加 `-hwaccel_flags +allow_profile_mismatch`） |
| QSV | `-hwaccel qsv [-hwaccel_output_format qsv]`；Linux 上可 prefer vaapi 子设备 |
| D3D11VA(AMF) | `-hwaccel d3d11va -hwaccel_output_format d3d11 -threads 2` |
| 设备初始化 | 之前还要 `-init_hw_device` 声明：如 `-init_hw_device videotoolbox=vt`、`-init_hw_device cuda=cu:0`、`-init_hw_device vaapi=va:/dev/dri/renderD128` + `-filter_hw_device {alias}` |

10bit HEVC/VP9 硬解需按配置开关（`EnableDecodingColorDepth10Hevc` 等）决定是否走硬解，否则返回空串回落软解。

### 3.3 编码端质量参数（GetVideoQualityParam + GetVideoBitrateParam 矩阵）

| 编码器 | preset | 码率控制 |
|---|---|---|
| libx264 | `-preset veryfast`(默认) `-crf 23` + `-x264opts:0 subme=0:me_range=16:rc_lookahead=10:me=hex:open_gop=0` | `-maxrate {b} -bufsize {2b}` |
| libx265 | `-preset .. -crf 28` + `-x265-params:0 no-scenecut=1:no-open-gop=1:no-info=1[:提速项]` | `-maxrate {b} -bufsize {2b}` |
| h264/hevc_videotoolbox | `-prio_speed 1`（快档）/ `0`（慢档） | `-b:v {b} -qmin -1 -qmax -1`（**不要给 maxrate/bufsize，会导致 VT 编码器 hang**） |
| h264/hevc/av1_nvenc | `-preset p1..p7`（p1=最快） | `-b:v {b} -maxrate {b} -bufsize {2b}`；**不传 -level**（NVENC 不能自适应会直接报错） |
| h264/hevc/av1_vaapi | Intel iHD: `-compression_level 1..7` | `-rc_mode VBR -b:v {b} -maxrate {b} -bufsize {2b}`（i965 用 CBR） |
| h264/hevc/av1_qsv | `-preset veryfast` 等合法档 | `-mbbrc 1 -b:v {b} -maxrate {b+1} -rc_init_occupancy {2b} -bufsize {4b}`（maxrate=b+1 触发 VBR）；h264_qsv 码率下限 1000k |
| h264/hevc_amf | `-quality speed/balanced/quality` (+hevc: `-header_insertion_mode gop -gops_per_idr 1`) | `-rc cbr -qmin 0 -qmax 32 -b:v -maxrate -bufsize` |
| libsvtav1 | `-preset 5..13` | `-b:v {b} -bufsize {2b}` + `-svtav1-params:0 rc=1:tune=0:...` |

Profile/level 归一化要点：HEVC 目标只转 8bit → main10 请求强制 `main`；h264 level clamp 到 ≤5.1（"51"），hevc ≤150（hevc_qsv 除 3）；nvenc 跳过 level；`-profile:v:0 {p}`。

### 3.4 Tone-mapping（HDR→SDR）滤镜链

判定：`VideoRange == HDR && bitDepth >= 10 && 开关开启`。滤镜链首部统一插 `setparams=color_primaries=bt2020:color_trc=smpte2084:colorspace=bt2020nc`（HLG 用 arib-std-b67）标注输入色彩。

| 路径 | 滤镜 |
|---|---|
| 软件 | `tonemapx=tonemap={bt2390|hable|...}:desat=0:peak=100:t=bt709:m=bt709:p=bt709:format=yuv420p[:param=..][:range=tv|pc]`（需 jellyfin-ffmpeg 的 tonemapx；上游 ffmpeg 可用 `zscale+tonemap` 替代） |
| **VideoToolbox 首选** | `scale_vt=w=..:h=..:color_matrix=bt709:color_primaries=bt709:color_transfer=bt709` —— scale_vt 一发完成缩放+tonemap |
| VideoToolbox(Metal) | `tonemap_videotoolbox=format=nv12:p=bt709:t=bt709:m=bt709:tonemap={alg}:peak={p}:desat={d}` |
| CUDA | `tonemap_cuda=format=yuv420p:p=bt709:t=bt709:m=bt709:tonemap={alg}:peak=100:desat=0[:tonemap_mode=..][:range=..]` |
| VAAPI (Intel) | `tonemap_vaapi=format=nv12:p=bt709:t=bt709:m=bt709:extra_hw_frames=32`（可前置 `procamp_vaapi=b=..:c=..`） |
| OpenCL | `tonemap_opencl=...`（同 cuda 模板，suffix 换 opencl；AMD/QSV 无原生时的后备，需 `-init_hw_device opencl@va`） |
| Vulkan | `libplacebo=upscaler=none:downscaler=none:w=..:h=..:format=..:tonemapping=bt.2390:peak_detect=0:color_primaries=bt709:color_trc=bt709:colorspace=bt709`（支持部分 DoVi） |

VideoToolbox 完整管线示例（硬解→硬滤→硬编，全程 vram）：
`-hwaccel videotoolbox -hwaccel_output_format videotoolbox_vld ... -vf "scale_vt=w=1920:h=1080:color_matrix=bt709:color_primaries=bt709:color_transfer=bt709"` + `-codec:v:0 hevc_videotoolbox`。软编时结尾补 `hwdownload,format=nv12`。

滤镜组装：main/sub/overlay 三链；无字幕烧录时 `-vf "f1,f2,..."`，烧录时 `-filter_complex "[0:s]{sub}[sub];[0:v]{main}[main];[main][sub]overlay_xx=eof_action=pass:repeatlast=0"`。非 SDR 直通时链首 `setparams=...bt709`（覆盖输出色彩标注）。

---

## 4. 临时目录 / 清理 / throttle

### 4.1 临时目录
- 路径：`{cache}/transcodes/{md5(mediaPath + userAgent + deviceId + playSessionId)}.m3u8` + 同名前缀 segment 文件。同一媒体+设备+会话 → 同一 basename，天然幂等。
- **启动时清空整个 transcodes 目录**（上次进程遗留）。
- kill 时按情况删除：`{basename}*` glob 全删，IO 忙时延迟 1500ms 重试最多 10 次（Windows 文件锁）。
- 另有每日定时任务删除 24h 前的残留。

### 4.2 Throttle（转码限速，防止一口气转完占满 CPU/盘）
- 条件：本地文件 + 时长 ≥5min。
- 每 5s 检查：`gap = TranscodingPositionTicks - DownloadPositionTicks`（转码进度来自 stderr `time=` 解析；下载进度来自 segment 响应完成回调）。
- `gap > max(ThrottleDelaySeconds, 60)s` → **暂停**：向 ffmpeg stdin 写 `p`（jellyfin-ffmpeg 的 pkey；上游 ffmpeg 6.1- 可写 `c`，7+ 不可靠）→ 也可用 SIGSTOP/SIGCONT（Rust 侧对 sidecar 进程更干净，跨平台注意 Windows 无信号）。
- gap 回落 → 写 `u`（或回车）恢复。新 segment 请求命中时也强制 unpause。
- stderr 进度解析：按空格拆词找 `time=HH:MM:SS.ss`、`fps=`、`size=Nkb`、`bitrate=Nkbits/s`；`当前位置 = startMs + time`。

### 4.3 滚动删除已看过的 segment（可选，EnableSegmentDeletion）
- 条件：HLS + 本地/HTTP + ≥5min。每 20s：`idxMax = (downloadPos_s - max(SegmentKeepSeconds,20)) / segLen`，删 0..idxMax 号文件。
- **陷阱**：copy-codec + 快盘时 ffmpeg 秒转完退出，滚动删除失去意义且文件全量堆积 → Jellyfin 用 `-readrate 10 -readrate_catchup 1000` 限制输入读取速度（仅 remux + 开启段删除时）。

---

## 5. nipa-stream Rust 实现建议

### 5.1 Session 结构

```rust
struct HlsSession {
    id: String,                     // md5(path + device_id + play_session_id)
    dir: PathBuf,                   // {cache}/transcodes/{id}/
    media_path: PathBuf,
    profile: TranscodeProfile,      // 判定结果: remux | transcode{vcodec, acodec, ...}
    seg_len: u32,                   // transcode=3, remux=6
    child: Option<FfmpegJob>,
    last_access: Instant,           // 每次 segment 请求刷新
    lock: tokio::sync::Mutex<()>,   // per-session 串行化启停判定
}

struct FfmpegJob {
    child: tokio::process::Child,   // stdin=piped(写 q 退出), stderr=piped(进度)
    start_seg: u32,                 // -start_number
    transcode_pos: AtomicI64,       // ticks, 来自 stderr time= 解析
    download_pos: AtomicI64,        // ticks, segment 响应完成时更新
    paused: AtomicBool,             // throttle 状态
    exited: Arc<Notify/AtomicBool>,
}

// 全局: DashMap<String, Arc<HlsSession>> + 每 10s tick 的清理任务
```

### 5.2 关键控制流（照抄 Jellyfin 状态机，参数取硬编码值）
1. `GET /hls/{sid}/main.m3u8`：不起 ffmpeg，按 duration 等长切分直接生成 VOD playlist（fMP4: VERSION 7 + EXT-X-MAP 指向 init.mp4，即 seg=-1），URL 带 `runtime_ticks`+`seg_len_ticks`。
2. `GET /hls/{sid}/{n}.mp4`：existing→serve；否则锁内判定 `n==-1(init) || 无 job || n < cur || n - cur > 24/seg_len` → kill（stdin 写 `q`，5s 超时 SIGKILL）→ 以 `-ss {n*seg_len}`（钳制到 duration-5s；remux +0.5s）+ `-start_number {n}` 重启 → 轮询 100ms 等 `n.mp4 存在 && (n+1).mp4 存在 or 进程退出`。
3. 清理定时器：`last_access > 60s` → kill + 删目录（开发文档 §6.2 的 60s 与 Jellyfin HLS ping timeout 60s 一致）。

### 5.3 参数模板（v1：VideoToolbox + 软编回落，fMP4）

Remux（视频兼容，仅换容器/音频）：
```
-analyzeduration 200M -probesize 1G
-ss {sec}.5 -noautorotate -i "{input}" -map_metadata -1 -map_chapters -1
-map 0:v:0 -map 0:a:{idx} -codec:v:0 copy -tag:v:0 hvc1(若hevc) -start_at_zero
-codec:a:0 aac -ac 2 -ab 256000
-copyts -avoid_negative_ts disabled -max_muxing_queue_size 128
-f hls -max_delay 5000000 -hls_time 6 -hls_segment_type fmp4
-hls_fmp4_init_filename "init.mp4" -start_number {n}
-hls_segment_filename "{dir}/%d.mp4"
-hls_playlist_type vod -hls_list_size 0 -hls_segment_options movflags=+frag_discont
-y "{dir}/index.m3u8"
```

Transcode（VideoToolbox h264，SDR）：
```
-hwaccel videotoolbox -hwaccel_output_format videotoolbox_vld -noautorotate
-ss {sec} -i "{input}" -map_metadata -1 -map_chapters -1 -threads 0
-map 0:v:0 -map 0:a:{idx}
-codec:v:0 h264_videotoolbox -prio_speed 1
-b:v {vb} -qmin -1 -qmax -1
-force_key_frames:0 "expr:gte(t,n_forced*3)" -g:v:0 {ceil(3*fps)} -keyint_min:v:0 {同}
-vf "scale_vt=w={w}:h={h}" (需要缩放时；HDR 源改用 scale_vt=...:color_matrix=bt709:color_primaries=bt709:color_transfer=bt709)
-codec:a:0 aac -ac 2 -ab {ab} -ar 48000(仅 ac-4)
[-bsf:a "noise=drop='lt(pts*tb\,{sec}.000)'"  ← 若音频 copy]
-copyts -avoid_negative_ts disabled -max_muxing_queue_size 128
-f hls -max_delay 5000000 -hls_time 3 -hls_segment_type fmp4
-hls_fmp4_init_filename "init.mp4" -start_number {n}
-hls_segment_filename "{dir}/%d.mp4" -hls_playlist_type vod -hls_list_size 0
-hls_segment_options movflags=+frag_discont -y "{dir}/index.m3u8"
```
软编回落：把 hwaccel 三项去掉、编码器换 `libx264 -preset veryfast -crf 23 -maxrate {vb} -bufsize {2vb} -sc_threshold:v:0 0 -x264opts:0 subme=0:me_range=16:rc_lookahead=10:me=hex:open_gop=0`，`-vf` 换 `scale=trunc(min(...)/2)*2:...`（宽高必须偶数）。

### 5.4 清理定时器与 throttle 设计
- 单个 tokio interval(10s) 扫全表：(a) last_access 超时 kill+删目录；(b) throttle 判定（gap>60s 写 `p`/SIGSTOP，回落写 `u`/SIGCONT——**LGPL 上游 ffmpeg 无 p/u 键，v1 建议直接用 SIGSTOP/SIGCONT（Unix）**，或干脆 v1 不做 throttle 只做 60s idle kill）。
- 进程退出统一走 `q` → wait 5s → kill 的两段式；启动进程后必须持续消费 stderr（否则管道满死锁），逐行解析 `time=` 更新进度。
- 启动即清空 transcodes 根目录；每次 kill 删 session 目录（重试応对 Windows 文件锁）。

### 5.5 坑与教训（Jellyfin 用血泪换的）
1. `-copyts -avoid_negative_ts disabled` 缺一不可，否则 seek 后段内时间戳从 0 开始，播放器时间轴跳变。
2. fMP4 必须 `movflags=+frag_discont`，否则 seek 重启后音频 delay 丢失。
3. **"下一段存在"才算本段写完**——不要文件一出现就 serve。
4. 音频 copy + 视频转码 + seek 必须用 noise bsf 丢包或强制转音频，否则音超前数秒。
5. VideoToolbox 不要给 maxrate/bufsize（会 hang）；NVENC 不要给 -level（会报错）。
6. 向前 seek 距离 ≤24s 时不重启（让现有进程追），否则重启；向后 seek 一律重启。
7. kill 前先写 `q` 优雅退出，避免残缺 segment；重启前删掉 mtime 最新的（残缺）segment。
8. 宽高经 scale 后必须 `trunc(../2)*2` 取偶。
9. remux 的 `-ss` 加 0.5s 偏移命中关键帧；一律 clamp 到 duration-5s。
10. 帧率 23.976 等分数帧率时 playlist 段长按 `ceil(fps)/fps` 微调，否则 EXTINF 与实际段长漂移累积。