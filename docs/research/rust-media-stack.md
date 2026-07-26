# Rust 自建类 Jellyfin 媒体服务器技术调研

## 1) Web 框架：axum

- **结论：选 axum，无悬念。** axum 0.8.x 是当前稳定版（0.8.0 发布于 2025-01，0.9 在 main 分支开发中），由 tokio 团队维护，构建于 tokio + hyper + tower 之上，是 2026 年新 Rust 项目的事实默认 Web 框架。
- 关键点：
  - 0.8 路由语法为 `/{id}` 与 `/{*rest}`（不再是 `/:id`）；原生 async trait，无需 `#[async_trait]`。
  - 无自有中间件系统，直接复用 `tower::Service`，因此超时、tracing、压缩、CORS、鉴权等由 **tower-http** 免费获得。
  - **tower-http 的 `ServeFile`/`ServeDir` 原生支持 HTTP Range 请求**（206 Partial Content / Content-Range），这是 Direct Play 场景的核心：直接把媒体文件挂成 route service 即可支持 `<video>` 拖动。注意 Safari 会先发 2 字节 range 探测，不支持 range 则完全不播；自己手写 range 时 Content-Range 末尾偏移是**闭区间**（`bytes 0-9999/10000`），写错会导致 VLC 等播放器异常。
  - Actix Web 吞吐略高（约 10–15%），但 axum 的生态组合性（含 gRPC/tonic 同栈）更适合本项目。

## 2) 视频播放路径

### 判定逻辑（参考 Jellyfin）
Jellyfin 的决策在 `MediaBrowser.Model.Dlna` 的 **StreamBuilder** 中：将客户端上报的 **DeviceProfile**（DirectPlayProfiles / CodecProfiles / ContainerProfiles / SubtitleProfiles / TranscodingProfiles / MaxStreamingBitrate）与 ffprobe 得到的 **MediaSourceInfo** 匹配，产出 StreamInfo（PlayMethod + 播放 URL 参数）。优先级严格分层：

1. **Direct Play**：容器 + 视频编码 + 音频编码 + 字幕 + 码率全部兼容 → 原样发文件（Range 请求即可，服务器几乎零负载）。
2. **Remux / Direct Stream**：视频流兼容但容器或音频不兼容 → `-c:v copy` 只换容器/转音频（如 mkv→HLS fMP4、DTS→AAC）。
3. **Transcode**：视频编码/码率/分辨率不兼容，或图形字幕（PGS/VobSub）需烧录，或 HDR 需 tone-mapping 到 SDR → 完整视频转码。

拒绝 direct play 时累积 **TranscodeReason** 标志（ContainerNotSupported / VideoCodecNotSupported / AudioChannelsNotSupported / VideoBitrateNotSupported 等），便于日志与调试。Web 端能力探测靠 `canPlayType()` 生成 profile 随 PlaybackInfo 请求上报。实践中最常见的"意外转码"来源是客户端默认码率上限和图形字幕烧录。

### HLS 按需转码的通用实现模式（参考 advplyr/hls-media-server、mifi/hls-vod、webtor/content-transcoder）
- **预生成完整 VOD 播放列表**：ffprobe 取总时长，按固定切片长度（如 hls_time=3~6s）算出总段数，立即生成完整 `.m3u8`（`#EXT-X-PLAYLIST-TYPE:VOD`），客户端认为整片"已就绪"，进度条完整可拖。
- **关键帧对齐**：ffmpeg 用 `-force_key_frames "expr:gte(t,n_forced*N)"` 强制每 N 秒一个关键帧，保证切片时长与预生成播放列表一致（ffmpeg 的 hls muxer 在 hls_time 后的下一个关键帧才切）。
- **按需启动 ffmpeg**：客户端请求 segment i；若已生成则直接返回；若未生成且与当前转码位置相近则等待；若相距较远（**seek 检测**：请求段号与 currentSegment 差值超过阈值）→ kill 当前 ffmpeg，以 `-ss (i * seg_len)`（放在 `-i` 前做快速 seek）+ `-start_number i` 重启。
- **Session 管理**：每个播放会话一个 session（sessionId → 临时目录 + ffmpeg 子进程 + 最后访问时间），客户端心跳/segment 请求刷新活跃时间，超时（如 60s 无请求）kill 进程并清理临时切片（webtor 的 "quit after inactivity" 模式）。
- 输出格式建议 fMP4 HLS（`-hls_segment_type fmp4`）以便 HEVC/现代浏览器兼容；remux 路径用 `-c:v copy -c:a aac`。

### sidecar 子进程 vs FFI 绑定
- **结论：转码路径用 sidecar ffmpeg 子进程（`tokio::process::Command` 或 [ffmpeg-sidecar](https://github.com/nathanbabcock/ffmpeg-sidecar) crate）。** 理由：
  - Jellyfin 本身就是 spawn ffmpeg CLI；转码/HLS 切片是"整条流水线"操作，CLI 完全够用。
  - 进程隔离：解码器 crash 不会带崩服务器；kill 进程即实现 seek/取消，天然契合 session 管理。
  - 构建零负担：FFI 绑定（ffmpeg-sys）需要 ffmpeg dev headers + pkg-config，跨平台/交叉编译痛苦。
  - ffmpeg-sidecar 提供 stderr 进度解析（frame/time/speed/bitrate），有配套 async-ffmpeg-sidecar；也可自己解析 `-progress pipe:1`。
- FFI 绑定仅在需要帧级处理（缩略图墙、自定义滤镜、低延迟）时考虑：**rsmpeg**（larksuite，v0.18 跟进 FFmpeg 8.0，活跃）优于 **ffmpeg-next**（维护放缓，上次更新约 11 个月前）。折中层有 video-rs / ez-ffmpeg。缩略图/章节图等简单需求同样可用子进程完成。
- 媒体探测：直接 `ffprobe -v quiet -print_format json -show_format -show_streams`，serde 反序列化即可，无需绑定。

### 硬件加速（均为 ffmpeg CLI 参数，运行时探测可用编码器：`ffmpeg -encoders` / 试转一小段）
- **VideoToolbox (macOS)**：`-hwaccel videotoolbox -hwaccel_output_format videotoolbox_vld -i in -c:v h264_videotoolbox`（或 hevc_videotoolbox），质量 `-q:v 0-100` 或 `-b:v`。本项目在 macOS 上的首选。
- **NVENC (NVIDIA)**：`-hwaccel cuda -hwaccel_output_format cuda -i in -c:v h264_nvenc/hevc_nvenc/av1_nvenc -preset p4 -cq 24`；GPU 上缩放用 scale_cuda/scale_npp。
- **VAAPI (Linux Intel/AMD)**：`-hwaccel vaapi -hwaccel_device /dev/dri/renderD128 -hwaccel_output_format vaapi -i in -c:v h264_vaapi`；AMD 需 `LIBVA_DRIVER_NAME=radeonsi` 且 `-bf 0`。
- **QSV (Intel)**：`-hwaccel qsv -hwaccel_output_format qsv -i in -vf scale_qsv=... -c:v h264_qsv`。
- 通用要点：`-hwaccel_output_format` 保持帧在 GPU 内存实现零拷贝解码→滤镜→编码；硬件编码约为 libx264 medium 的 10 倍速但压缩效率略差；HDR→SDR tone-mapping 需要额外滤镜链（如 tonemap_vaapi / libplacebo）。

## 3) 已有 Rust 开源媒体服务器

- **[Dim](https://github.com/Dusk-Labs/dim)**（约 4.1k stars，GPL-2.0）：**已实质弃坑**。最后 release 为 v0.3.0-rc6（2021-10），issue 大量积压，仓库已转移到原作者个人账号 vgarleanu/dim。技术栈偏旧（早期用 Rocket/Diesel，后期 nightly Rust）。可借鉴：整体架构（library 扫描、元数据刮削 TMDB、流 manifest API `/api/v1/stream/{id}/manifest`）、web UI 编译后 embed 进二进制的做法。
- **[zenith (hasali19)](https://github.com/hasali19/zenith)**：个人项目但持续活跃（1600+ commits），Rust 后端 + Flutter 客户端 + Chromecast + Docker。可借鉴：Rust workspace 组织、SQLite + sqlx 的媒体库 schema、ffmpeg 转码会话实现。
- 其他（xiu、atm0s-media-server、media-rs）都是 RTMP/WebRTC 直播服务器，与 VOD 媒体库场景无关。
- **结论**：没有活跃且功能完整的类 Jellyfin Rust 实现，此项目定位有空间；判定逻辑与 ffmpeg 参数最佳参考仍是 Jellyfin 本体（C#，MediaEncoding/TranscodingJobHelper 部分）。

## 4) 内嵌 BT 下载：librqbit

- **结论：完全可行，rqbit 官方架构就是"librqbit 库 + 薄 CLI 壳"，专为嵌入设计。** Apache-2.0，活跃维护（当前 8.x/9.x，9.0 有 SessionOptions 破坏性调整），tokio 原生 async。
- 核心 API（[docs.rs/librqbit](https://docs.rs/librqbit)）：
  ```rust
  let session = Session::new(download_dir).await?;
  let handle = session.add_torrent(AddTorrent::from_url("magnet:?..."), None)
      .await?.into_handle()?;
  handle.wait_until_completed().await?;
  ```
  - `Session::new_with_opts` 可配置 DHT、监听端口、限速、持久化（session 恢复）等。
  - `Api` 类型提供面向 UI 的可序列化 facade（rqbit 桌面版即用它），适合直接接到 axum 的 JSON API。
  - **流式播放**：支持 `TorrentHandle` 的文件流（stream），按需优先下载被读取的 piece 并阻塞等待——**可实现边下边播/拖动**，与转码管线可以串联。
  - 附带 DHT、UPnP 端口映射、uTP、magnet 解析等子 crate。资源占用低（几十 MB 内存，树莓派可跑）。
- 集成方式：librqbit Session 作为 axum AppState 的一个组件，下载完成事件驱动媒体库扫描/重命名。

## 5) 动漫 RSS 自动追番（Mikan + AutoBangumi 思路）

- **Mikan（蜜柑计划）RSS 工作方式**：
  - 聚合订阅：`https://mikanani.me/RSS/MyBangumi?token=xxx`（用户在网站订阅番剧+字幕组后生成个人 token）；单番剧：`https://mikanani.me/RSS/Bangumi?bangumiId=ID&subgroupid=ID`。备用域名 mikanime.tv；大陆访问常需反代（常见方案 Cloudflare Workers 反代 RSS 与种子链接）。
  - RSS `<item>` 内含标题（字幕组原始文件名）+ `<enclosure url="....torrent" type="application/x-bittorrent"/>` 指向种子文件，另有条目详情链接。消费方式：定时拉取 RSS → 解析新条目 → 下载 .torrent（或转 magnet）→ 推给下载器。
- **AutoBangumi（EstrellaXD/Auto_Bangumi，Python）实现思路**，流程「订阅 → 解析 → 下载 → 重命名整理 → 媒体库识别」：
  1. 轮询 RSS，用**正则/规则引擎解析字幕组发布文件名**（提取番名、季、集、字幕组、分辨率、语言）——这是核心难点，AB 维护了一套复杂的 raw parser；
  2. 按番剧+字幕组自动生成订阅规则，去重（同番剧建议只订一个字幕组）；
  3. 推送 qBittorrent/aria2 下载（本项目可替换为内嵌 librqbit）；
  4. 通过 **TMDB 匹配元数据**，按 `番剧名/Season XX/番剧名 SxxExx.mkv` 重命名落盘，使 Jellyfin/Plex 免二次刮削直接识别。
- 对本项目：Rust 侧用 `rss`/`feed-rs` crate 解析 feed + `tokio` 定时任务 + librqbit 下载 + 自研文件名解析器（可参考 AB 的正则集），即可闭环，无需外部下载器。

## 6) SQLite 访问层

- **rusqlite**：同步、轻量、最贴近 SQLite 本身（可 bundled 编译 libsqlite3）。适合 CLI/桌面；在 async Web 服务中需 `spawn_blocking` 或连接池包装，略别扭。
- **sqlx**：纯 async SQL 工具箱（非 ORM），`query!` 宏可对开发库做**编译期 SQL 校验**，与 axum/tokio 天然契合，内置迁移（`sqlx migrate`）。SQLite 驱动为纯 Rust 侧封装，写并发需注意 WAL + 单写连接模式。zenith 等项目即用此组合。
- **sea-orm**：构建于 sqlx 之上的完整 async ORM（2.0 于 2026-01 发布，25 万+周下载），ActiveRecord 风格、关系处理和实体生成 CLI 齐全；代价是抽象"魔法"多、性能开销、且其公开依赖 sqlx 的版本升级会造成 semver 破坏。
- **结论：本项目推荐 sqlx + SQLite（WAL 模式）**——媒体库 schema 不复杂、查询手写 SQL 更可控、async 生态一致；不想写 SQL 才考虑 sea-orm；rusqlite 不适合 async 服务主路径。

## 推荐技术栈汇总

| 层 | 选型 |
|---|---|
| Web/API | axum 0.8 + tower-http（ServeFile 做 Direct Play range 服务） |
| 探测/转码 | sidecar ffprobe/ffmpeg 子进程（tokio::process，可选 ffmpeg-sidecar），HLS fMP4 按需转码 + session 管理 + seek 重启，硬件加速按平台探测（macOS 用 VideoToolbox） |
| BT 下载 | librqbit（内嵌 Session，支持流式边下边播） |
| 追番 | feed-rs 解析 Mikan RSS + 文件名规则解析 + TMDB 刮削 + 自动重命名 |
| 数据库 | sqlx + SQLite（WAL） |

## Sources
https://tokio.rs/blog/2025-01-01-announcing-axum-0-8-0
https://sharpskill.dev/en/blog/rust/rust-actix-web-vs-axum-comparison
https://deepwiki.com/jellyfin/jellyfin/3.3-dlna-and-stream-selection
https://deepwiki.com/jellyfin/jellyfin/3-media-streaming
https://jellyfin.org/docs/general/post-install/transcoding/
https://jellyfin.org/docs/general/clients/codec-support/
https://deepwiki.com/jellyfin/jellyfin-web/5.1-device-profile-system
https://github.com/advplyr/hls-media-server
https://github.com/mifi/hls-vod
https://hub.docker.com/r/webtor/content-transcoder
https://github.com/nathanbabcock/ffmpeg-sidecar
https://github.com/larksuite/rsmpeg
https://lib.rs/crates/ffmpeg-sidecar
https://developer.nvidia.com/blog/nvidia-ffmpeg-transcoding-guide/
https://gist.github.com/Brainiarc7/95c9338a737aa36d9bb2931bed379219
https://github.com/Dusk-Labs/dim
https://github.com/Dusk-Labs/dim/releases
https://github.com/hasali19/zenith
https://docs.rs/librqbit
https://github.com/ikatson/rqbit/blob/main/README.md
https://crates.io/crates/librqbit
https://github.com/EstrellaXD/Auto_Bangumi
https://www.autobangumi.org/config/rss.html
https://www.appinn.com/autobangumi-v2/
https://zhuanlan.zhihu.com/p/663368543
https://github.com/EstrellaXD/Auto_Bangumi/issues/196
https://aarambhdevhub.medium.com/rust-orms-in-2026-diesel-vs-sqlx-vs-seaorm-vs-rusqlite-which-one-should-you-actually-use-706d0fe912f3
https://byteiota.com/rust-orms-2026-sqlx-vs-diesel-vs-seaorm-comparison/
https://github.com/tokio-rs/axum/discussions/3254
https://github.com/tokio-rs/axum/discussions/608