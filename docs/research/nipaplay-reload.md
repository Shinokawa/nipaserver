# NipaPlay-Reload 调研报告

仓库现已迁移到组织 **AimesSoft/NipaPlay-Reload**（`MCDFsteve/NipaPlay-Reload` 301 重定向到它，MCDFsteve 为主要作者）。主语言 Dart（Flutter），核心模块 Rust（flutter_rust_bridge 2.12.0）。当前版本 v1.11.2（pubspec），最新 Release v1.11.1（2026-07-20）。

## 1) 功能清单

### 弹弹play 集成（深度集成，是弹幕/识别的核心）
- 专用 AppId：`nipaplayv1`，使用官方签名方案 `X-AppId` + `X-Signature`（md5/签名串 `appId+timestamp+apiPath+appSecret`），代码在 `lib/services/dandanplay_service_io.dart`（约千行，含 io/stub 条件导入以支持 web 编译）。
- 使用的弹弹play API：`/api/v2/match`（文件识别，参数 hash/fileName/fileSize）、弹幕获取/发送、`/api/v2/login`、`/api/v2/login/renew`、`/api/v2/register`、`/api/v2/bangumi/recent`（新番表）、Bangumi OAuth (`/api/v2/oauthprovider/bangumi/login`)、播放历史、收藏。
- 识别缓存：按文件 hash 缓存匹配结果，未匹配结果缓存 3 天（`_unmatchedVideoCacheDuration`）。
- 弹幕加载策略（1.9.x 重构后）：缓存命中 → 用户自定义弹幕服务器 → 弹弹play 主服务器与 "NipaPlay 代理" 竞速。即官方已自建了一个弹幕代理服务器。
- 对 Jellyfin/Emby 流媒体也做弹弹play 匹配：`lib/services/jellyfin_dandanplay_matcher.dart` / `emby_dandanplay_matcher.dart` —— 无法取文件前 16MB 时用空 hash + 构造的 `"$seriesName - $episodeName.mp4"` 文件名走 match 接口（文件名匹配兜底）；能取到时由 RemoteMediaFetcher 计算 hash（即弹弹play 标准：文件前 16MB 的 MD5）。
- 支持"弹弹play 远程库"（dandanplay 桌面版的远程访问协议）作为一种远程媒体源：`lib/services/dandanplay_remote_service.dart`、`providers/dandanplay_remote_provider.dart`，含外挂字幕识别。

### 本地媒体库
- 详见第 3 节。SQLite（sqflite / sqflite_common_ffi 桌面）+ SharedPreferences 存储。

### 远程媒体库 / 媒体服务器客户端
- **Jellyfin 与 Emby**：完整客户端支持（登录、库选择、详情页、观看标记、混合类型库文件夹导航、外挂字幕）。已知短板（issue #184）：播放走简化 URL 拼接、未实现 `POST /Items/{id}/PlaybackInfo` 协商，转码切换/多音轨选择/会话管理有缺陷 —— 这是配套服务器可以做得更好的点。
- **SMB**（smb_connect 包）、**WebDAV**（webdav_client 包，含快速访问、文件排序）。
- **NipaPlay 局域网共享**（见第 4 节）。
- 多地址服务器支持（`multi_address_server_service.dart`）、服务器连通性检测、观看历史与服务器同步（`server_history_sync_service.dart`）。

### BT 下载
- 内置种子下载器，Rust 侧用 **librqbit 8.1.1**（`rust/src/api/torrent.rs`），支持磁力预览（`torrent_magnet_preview.dart`）、任务管理、下载完成扫描汇总（`torrent_task_scan_summary.dart`）。Roadmap 中"内置下载器及远程控制"已勾选。

### 播放内核（多内核可切换）
- **Erika**：自研内核（github.com/AimesSoft/Erika），Rust + Metal，macOS/iOS 专用，支持 HDR/EDR。
- **FVP**（基于 libmdk，即 mdk-sdk）。
- **Media Kit**（基于 libmpv）。
- **video_player**（Flutter 官方，平台原生解码）。
- ffmpeg 不直接依赖 —— 通过 libmpv/libmdk 间接使用；Rust Cargo.toml 无 ffmpeg crate。
- 画质增强：Anime4K 超分、CRT 着色器；多音轨、倍速。

### 其他
- 弹幕渲染：滚动/顶底、防遮挡、轨迹记忆、本地 xml/json 挂载；自研 danmaku_canvas 包 + Rust wgpu 渲染管线（Cargo.toml 含 wgpu 27、Metal/DX12/Vulkan 后端、fdsm MSDF 字体渲染）。
- 字幕：ASS/SRT、多轨、样式自定义、SMB 流外挂字幕。
- Bangumi 进度同步、评分、评论；新番时间表。
- AI 防剧透（智能遮挡剧透内容）。
- JS 插件系统（flutter_js）。
- Roadmap 未完成项：FTP 挂载、GIF 导出、Webview 弹幕刮削、补帧、跨平台 HDR/杜比视界、鸿蒙/visionOS/tvOS。

## 2) 支持平台
Windows (x64)、macOS (Intel/Apple Silicon, Homebrew cask)、Linux (amd64/arm64, Gentoo ebuild；AUR 包被恶意接管勿用)、Android、iOS（App Store 上架，id6751284970）、Windows 应用商店（MSIX）。此外仓库含 `web/` 目录与大量 `*_web.dart` 条件实现 —— Flutter Web 是受支持的编译目标（用于内嵌 WebUI）。

## 3) 本地媒体库/扫描实现
- 扫描已从 Dart 迁移到 **Rust**（1.9.x 起转正）：`rust/src/api/file_scan.rs` + `lib/services/rust_file_scan_service.dart` + `lib/services/scan_service.dart`。
- 扫描逻辑：递归遍历用户指定文件夹（跳过 symlink），**仅识别 .mp4 和 .mkv 两种扩展名**。
- **变更检测 hash ≠ 弹弹play hash**：Rust 扫描用 `sha256("size|modified_millis")` 取前 16 个 hex 字符作为轻量指纹，仅用于 diff（新增/修改/删除文件定位），不读文件内容。缓存存 SharedPreferences（`nipaplay_subfolder_hash_cache`）。
- **内容识别完全依赖弹弹play**：真正的媒体识别/刮削是对每个文件计算弹弹play 标准 hash（前 16MB MD5）调 `/api/v2/match`，元数据来自弹弹play + Bangumi API。**没有本地文件名解析刮削、没有 TMDB/TVDB 集成** —— 官方文档甚至建议 Jellyfin 端装 kookxiang/jellyfin-plugin-bangumi 补足动漫元数据。这是"AI 刮削服务器"最大的互补空间（弹弹play match 对电影/非番剧、改名文件、冷门内容识别弱）。
- Android 有 SAF 目录扫描路径（`AndroidSafService.scanDirectory`）。
- 番剧组织按弹弹play 返回的 animeId/episodeId 聚合成"媒体库"视图，存 SQLite。

## 4) Web 端 / 服务器模式（已存在！）
- **内置 Web 服务器**：`lib/services/web_server_service.dart`，基于 Dart **shelf** (+shelf_router/shelf_static/shelf_cors_headers)，默认端口 **1180**，支持 IPv6、开机自启选项、局域网 UDP 发现（`nipaplay_lan_discovery.dart`，移动端可"扫描局域网"发现 PC）。
- **内嵌 WebUI**：Flutter Web 构建产物打包在 `assets/web`，由 shelf_static 服务，自动向 index 注入 `?api=<origin>` 参数指向 API base。即已有可用的浏览器远程访问界面。
- **HTTP API**（`lib/services/web_api_service.dart`），现有 endpoint：
  - `/info`
  - `/bangumi/calendar`、`/bangumi/detail/<id>`、`/bangumi/login_status`
  - `/danmaku/video_info`、`/danmaku/load`
  - `/image_proxy`、`/web_proxy`（全 HTTP method 的代理）
  - `/search/config`、`POST /search/by-tags`、`POST /search/advanced`
  - `/dandanplay/*`：login/logout/refresh_login/login_status/webtoken/bangumi OAuth/play_history/favorites/send_danmaku/add_play_history/add_favorite/remove_favorite
  - `/media/libraries`、`/media/local/items`、`/media/local/item/<animeId>`
  - `/history`、`POST /history/progress`
  - 挂载子路由：`/media/local/share/`（局域网媒体共享，含视频流）、`/media/local/manage/`、`/settings/network/`、`/remote/control/`（远程控制）
- **"NipaPlay 局域网共享"** 就是一种轻量服务器模式：PC 端开 web 服务器，移动端把它当远程媒体库挂载直接串流播放，"自动刮削：采用 hash 匹配刮削"（仍是弹弹play hash）。有独立的客户端模型/Provider（`shared_remote_library.dart`、`shared_remote_library_provider.dart`）—— **配套服务器如果实现这套 `/media/local/share/` 协议（或同为"远程媒体库"的一种 server_profile），客户端可近零改动接入**。相关模型 `lib/models/server_profile_model.dart`、`media_server_playback.dart`、`lib/services/media_server_service_base.dart`（Jellyfin/Emby 的公共基类）表明客户端已有多服务器类型抽象。

## 5) License 与活跃度
- **License：MIT**。
- **Stars 1640，Forks 97，Open issues 82**（2026-07-26 查询）。
- 创建于 2025-03-19；**最后 push 2026-07-26（当天）**，7 月内数十个 merge PR，Release 节奏约每周（v1.10.13 07-07 → v1.11.0 07-15 → v1.11.1 07-20），有多名外部贡献者（ASIjacket、makabaka11 等）与 CI，非常活跃。iOS App Store / MS Store 双上架，有爱发电赞助渠道。

## 对配套服务器设计的关键结论
1. 客户端已有成熟的"远程媒体库"抽象（Jellyfin/Emby/WebDAV/SMB/弹弹play远程库/NipaPlay共享库五类），新服务器可选择：a) 实现 Jellyfin 兼容 API（现成客户端支持，但要注意 #184：客户端目前不发 PlaybackInfo，只拼直链 URL，静态直连/DirectPlay 优先的服务器反而最匹配现状）；b) 实现 NipaPlay 自己的 `/media/local/share/` + LAN discovery 协议（端口 1180，UDP 发现），侵入最小。
2. 刮削端空白点明确：客户端只靠弹弹play 16MB-MD5 hash match + Bangumi 元数据，仅认 mp4/mkv；AI 刮削服务器可补文件名解析、电影/剧集/TMDB、更多容器格式，并以 Jellyfin/Bangumi 风格元数据回喂。
3. 可复用点：弹弹play API 签名方案与 appId 机制、弹幕代理竞速思路、Rust librqbit BT 下载（服务器端可直接用同 crate）、shelf API 路由结构可作为服务器 API 契约参考。
4. MIT 许可，服务器端复用/参考客户端代码无法律障碍。

## Sources
https://github.com/MCDFsteve/NipaPlay-Reload
https://github.com/AimesSoft/NipaPlay-Reload
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/README.md
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/Documentation/server-integration.md
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/pubspec.yaml
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/lib/services/web_server_service.dart
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/lib/services/web_api_service.dart
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/lib/services/scan_service.dart
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/lib/services/dandanplay_service_io.dart
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/lib/services/jellyfin_dandanplay_matcher.dart
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/lib/services/video_file_scanner.dart
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/rust/src/api/file_scan.rs
https://raw.githubusercontent.com/AimesSoft/nipaplay-reload/main/rust/Cargo.toml
https://github.com/MCDFsteve/NipaPlay-Reload/issues/184
https://github.com/MCDFsteve/NipaPlay-Reload/releases
https://api.github.com/repos/AimesSoft/nipaplay-reload