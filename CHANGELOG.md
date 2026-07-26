# Changelog

本项目遵循 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.1.0/) 的结构，并计划在稳定发布后采用语义化版本。

## [Unreleased]

### Added

- M3 播放后端：签名 Direct Play、Range、PlaybackInfo 与按需 HLS；
- WebUI 视频播放器、续播、播放进度和常用 OSD 控件；
- M4 `librqbit 8.1.1` 下载器与 Mikan RSS 订阅；
- 下载投影对账、RSS 持久去重和下载完成幂等入库；
- 下载管理 WebUI：任务进度、暂停/恢复/删除，以及 RSS 订阅的完整 CRUD 与手动检查；
- 开源项目文档、CI、容器与社区维护文件。
- 正式版本化 `nipa-agent v0.1.1`，以中文作为主要项目文档，主仓通过 submodule commit 和 Cargo 精确版本双重固定。

### Security

- 流媒体 HMAC 密钥使用系统 CSPRNG；
- 媒体路径 canonicalize 防逃逸；
- RSS 与 torrent 出站请求包含 SSRF、防 DNS rebinding 和响应体上限保护。

## [0.1.0] - Unreleased

首个开发版本。尚未建立稳定 API 或配置兼容性承诺。
