# 参与贡献

感谢你参与 NipaServer。项目仍处于快速迭代期，较大的功能改动请先开 Issue 讨论边界和数据迁移方案。

## 开发准备

```bash
git clone --recurse-submodules https://github.com/AimesSoft/nipaserver.git
cd nipaserver
cp nipaserver.example.toml nipaserver.toml
make setup
make check
```

需要 Rust 1.88+、Node.js 20+ 和 ffmpeg/ffprobe。没有 ffmpeg 时相关集成测试会跳过，但提交播放链路改动时必须在有 ffmpeg 的环境完成测试。

## 提交改动

1. 从最新 `main` 创建主题分支；
2. 一次提交只解决一个清晰问题；
3. 新行为必须有相应测试；
4. 数据库 schema 只能通过新的 sqlx migration 演进，不能修改已发布 migration；
5. 不要提交媒体文件、数据库、API key、模型凭证或下载会话；
6. 更新用户可见行为时同步 README 或 `docs/`；
7. 提交前运行 `make check`。

建议使用清晰的命令式提交标题，例如：

```text
Fix HLS timeline near segment boundaries
Add Mikan subscription retry state
```

## 代码约定

- Rust 使用稳定工具链、rustfmt 和 Clippy；
- WebUI 使用 Svelte 5 与 TypeScript，必须保持 `svelte-check` 无错误和警告；
- server 端输入必须考虑路径逃逸、SSRF、响应体上限和日志中的敏感信息；
- `nipa-stream`、`nipa-download` 等可回流 crate 不应引入 Axum/Tower；
- 参考 Jellyfin 等 GPL 项目时只做行为和协议研究，不复制其实现代码。

## Pull Request

PR 描述应包含：动机、实现范围、测试证据、迁移/兼容性影响和已知限制。UI 改动建议附截图；播放或下载改动应说明是否完成真实文件端到端验证。

提交贡献即表示你有权提交该代码，并同意它按本项目 MIT License 发布。所有参与者需遵守 [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md)。
