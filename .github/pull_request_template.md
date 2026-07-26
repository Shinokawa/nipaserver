## 变更说明

<!-- 为什么需要这个改动？实现范围是什么？ -->

## 验证

- [ ] `cargo test --workspace --all-targets --locked`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `make fmt-check`
- [ ] `cd webui/app && npm run check && npm run build`（涉及 WebUI 时）
- [ ] 完成真实 ffmpeg / 浏览器 / 下载端到端验证（涉及对应链路时）

## 兼容性与风险

<!-- 数据库迁移、配置变化、API 兼容性、安全影响、已知限制。无则写“无”。 -->

## UI

<!-- UI 改动请附截图或录屏；无 UI 改动可删除本节。 -->
