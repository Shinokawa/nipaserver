# NipaServer

[![CI](https://github.com/AimesSoft/nipaserver/actions/workflows/ci.yml/badge.svg)](https://github.com/AimesSoft/nipaserver/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

NipaServer 是面向 NipaPlay 的智能媒体服务器：扫描本地媒体库，结合弹弹play、Bangumi、TMDB 与可选 AI Agent 完成识别和刮削，并提供 WebUI、Direct Play、HLS 转码、BT 下载及 Mikan RSS 追番。

> [!WARNING]
> 项目仍处于 `0.1.x` 开发阶段，统一登录与角色鉴权尚未完成。请只在可信局域网或反向代理认证之后使用，**不要直接暴露到公网**。

## 当前能力

- 媒体扫描、文件指纹与弹弹play hash 匹配；
- Bangumi/TMDB 元数据、AI 辅助识别和人工复核队列；
- 海报墙、详情页、观看进度、继续观看与 Next Up；
- HMAC 临时播放 URL、HTTP Range Direct Play；
- ffmpeg fMP4 HLS remux/转码、seek 重启及会话回收；
- 内嵌 NipaPlay 同款 `librqbit 8.1.1` 下载器；
- Mikan RSS 订阅、字幕组/分辨率/排除规则过滤及下载完成自动入库；
- 常驻“管家”对话、巡检报告与 Agent 过程事件流。

尚未完成的主要内容包括：统一认证、完整 Jellyfin 兼容层、下载管理 WebUI、发布安装包及真实公网磁力的自动化端到端测试。

## 快速开始

### 环境要求

- Git（需要拉取 `nipa-agent` submodule）；
- Rust 1.88 或更新版本；
- Node.js 20 或更新版本；
- ffmpeg 与 ffprobe（缺失时仍可 Direct Play，但不能探测和转码）。

### 本地运行

```bash
git clone --recurse-submodules https://github.com/AimesSoft/nipaserver.git
cd nipaserver
cp nipaserver.example.toml nipaserver.toml

cd webui/app
npm ci
npm run build
cd ../..

cargo run -p nipa-server
```

打开 [http://127.0.0.1:11810](http://127.0.0.1:11810)。首次启动后可在 WebUI 中添加媒体库。

如果已经普通 clone：

```bash
git submodule update --init --recursive
```

### Docker Compose

```bash
cp nipaserver.example.toml nipaserver.toml
cp .env.example .env
docker compose up --build -d
```

Compose 默认使用 `nipa-data` named volume 保存数据，并把 `./media` 只读挂载到 `/media`。请按实际媒体路径修改 `compose.yaml`；Compose 会通过 `NIPA_BIND=0.0.0.0` 覆盖示例配置中的回环监听地址。

## 配置

配置优先级为环境变量覆盖 TOML，默认读取当前目录下的 `nipaserver.toml`。完整安全示例见 [`nipaserver.example.toml`](nipaserver.example.toml)。

常用环境变量：

| 变量 | 说明 |
|---|---|
| `NIPA_CONFIG` | 配置文件路径 |
| `NIPA_BIND` | HTTP 监听地址 |
| `NIPA_PORT` | HTTP 监听端口 |
| `NIPA_DATA_DIR` | SQLite、图片、下载与会话数据目录 |
| `RUST_LOG` | tracing 日志过滤器 |

模型 API key 建议通过 `model.api_key_env` 引用环境变量，不要提交真实 key。TMDB token 同样不要写入公开配置。

## 开发

```bash
make check       # Rust 测试 + Clippy + WebUI 检查
make build       # release server + WebUI
make run         # 开发运行
```

也可分别执行：

```bash
cargo test --workspace --all-targets --locked
cargo test --manifest-path nipa-agent/Cargo.toml --all-targets
cargo clippy --workspace --all-targets -- -D warnings
make fmt-check
cd webui/app && npm ci && npm run check && npm run build
```

## 架构

```text
crates/nipa-core       领域模型与配置
crates/nipa-scanner    文件扫描、hash、diff 与 evidence
crates/nipa-match      弹弹play 匹配客户端
crates/nipa-providers  Bangumi/TMDB provider 与 Agent tools
crates/nipa-stream     ffprobe、播放判定与 HLS session
crates/nipa-download   librqbit 与 Mikan RSS
crates/nipa-server     SQLite、业务编排与 Axum API
nipa-agent             v0.1.0 独立 submodule：工具调用 Agent runtime
webui/app              Svelte 5 WebUI
```

设计与实现细节：

- [开发文档](docs/01-开发文档.md)
- [Agent 接口契约](docs/03-agent接口契约.md)
- [Jellyfin 精读与实现指南](docs/04-jellyfin精读与实现指南.md)
- [WebUI 设计](docs/05-webui设计.md)
- [管家设计](docs/06-管家设计.md)

## API

当前自研 API 使用 `/api/v1` 前缀，主要端点包括：

- `/libraries`、`/items`：媒体库与条目；
- `/playback/info`、`/stream/*`：播放协商与流媒体；
- `/downloads`、`/subscriptions`：下载与订阅；
- `/playback/progress`：观看进度；
- `/chat`、`/events`：管家与 SSE 事件。

在统一认证层完成前，这些端点只适合可信网络。

## 参与贡献

请先阅读 [`CONTRIBUTING.md`](CONTRIBUTING.md) 与 [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)。漏洞请按照 [`SECURITY.md`](SECURITY.md) 私下报告，不要公开披露可利用细节。

## 许可证与第三方服务

本仓库代码以 [MIT License](LICENSE) 发布。`nipa-agent` 是固定到 `v0.1.0` 的独立 MIT submodule。第三方组件、服务条款与署名见 [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md)。

This product uses the TMDB API but is not endorsed or certified by TMDB.
