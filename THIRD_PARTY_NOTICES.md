# 第三方组件与服务声明

NipaServer 本体采用 MIT License。依赖项仍分别受其自身许可证约束，发布二进制或容器镜像时应保留对应许可证与通知。

## 主要组件

- [librqbit](https://github.com/ikatson/rqbit)（Apache-2.0）：内嵌 BitTorrent 会话；
- [ffmpeg](https://ffmpeg.org/)：外部 sidecar，容器镜像使用发行版提供的构建；具体许可证取决于构建选项；
- [hls.js](https://github.com/video-dev/hls.js)（Apache-2.0）：Web 浏览器 HLS 播放；
- [Svelte](https://svelte.dev/)（MIT）：WebUI；
- [Axum](https://github.com/tokio-rs/axum)、[Tokio](https://tokio.rs/) 与 [sqlx](https://github.com/launchbadge/sqlx)：Rust 服务端基础设施。

完整依赖列表以 `Cargo.lock`、`webui/app/package-lock.json` 和构建产物的许可证扫描结果为准。

## 外部数据服务

### TMDB

This product uses the TMDB API but is not endorsed or certified by TMDB.

使用者必须自行遵守 TMDB API 与图片服务条款。商业使用前请确认授权要求。

### Bangumi、弹弹play与 Mikan

项目只在用户配置或实际媒体操作所需范围内调用相关服务。使用者应遵守各服务的 API、抓取、内容和商业使用条款；不得利用本项目批量抓取或重新分发受限制的数据。

## 参考实现

项目研究 Jellyfin 的公开协议和播放判定行为，但不复制 Jellyfin 的 GPL 实现代码。`reference/` 仅为本地研究目录并被 Git 忽略，不属于发布内容。

[`nipa-agent v0.1.0`](https://github.com/AimesSoft/nipa-agent/tree/v0.1.0) 是独立 Git submodule，采用 MIT License；主仓固定其版本但不重新授权其代码。
