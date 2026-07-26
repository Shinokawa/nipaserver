# Jellyfin 与刮削插件精读——nipaserver 实现指南

> 2026-07-26。五份完整精读报告在 `docs/research/jellyfin-extract/`，本文是索引 + 关键结论。
> 源码在 `reference/`：jellyfin（GPL-2.0）、jellyfin-plugin-bangumi、jellyfin-plugin-metashark。
> **License 红线：所有产出是逻辑/协议/参数提炼，禁止逐行翻译 GPL 代码进 MIT 仓库。**

## 报告索引

| 报告 | 内容 | 服务对象 |
|---|---|---|
| `jf-streambuilder.md` | Direct Play/Stream/Transcode 完整判定顺序、TranscodeReason 全枚举、DeviceProfile 结构、PlaybackInfo 协议、**19 份真实客户端 profile JSON 测试样本位置** | nipa-stream（M3） |
| `jf-transcode.md` | HLS ffmpeg 命令组装、seek 重启状态机、硬件加速参数矩阵、tone-mapping 滤镜链、临时目录清理与 throttle | nipa-stream（M3） |
| `jf-provider-api.md` | provider 接口抽象（ItemLookupInfo/MetadataResult）、TMDB 调用姿势与语言 fallback、三级刮削关联链、**tool 返回字段精简建议** | nipa-providers（M2a） |
| `plugin-bangumi.md` | Bangumi API 清单、文件名正则集、季/条目映射实践与失败模式、[bangumi-id] 强制绑定 | nipa-providers + agent prompt |
| `plugin-metashark.md` | 豆瓣 frodo API 姿势与反爬对策、豆瓣×TMDB 聚合策略、电影/剧集识别路径差异 | search_douban（可选 feature） |

## 对当前实现的直接影响（已消化项）

1. **播放判定**（M3 已据 `jf-streambuilder.md` 落地 Rust 判定器）：
   - 三种 PlayMethod 的语义与优先级；DirectStream 可容忍的失败原因集合（Audio* | ContainerNotSupported | VideoCodecTagNotSupported）——换容器 remux 即可解决的都不需要重编码；
   - **测试金矿**：`reference/jellyfin/tests/Jellyfin.Model.Tests/Test Data/DeviceProfile-*.json` 有 19 份真实客户端 profile（Chrome/Safari/AndroidTV/WebOS…），直接作为 nipa-stream 判定器的测试夹具；
   - Jellyfin 10.10 的坑：http direct-stream 标注 broken，DirectStream 实际经 TranscodingProfile 的 rank 机制产生（rank.video==1 → `-c:v copy`）。

2. **HLS 转码**（`jf-transcode.md`）：M3 已实现 `-ss` 前置 seek、对齐关键帧、fMP4 init segment、按需转码与 seek 重启；硬件加速和 HDR tone-mapping 仍留后续版本。

3. **agent 工具层**（`jf-provider-api.md` + `plugin-bangumi.md`，已在 nipa-providers 实施中）：
   - TMDB `append_to_response` 合并请求、`include_image_language=zh,null`；
   - Bangumi 每季独立 subject → 工具描述中已注明，agent 需按季搜索；
   - `[bangumi-id]` 强制绑定思路 → 未来在扫描层识别路径中的 id 标记，跳过 agent 直接绑定（写入开发文档 §4.2 的待办）。

4. **豆瓣**（`plugin-metashark.md`）：frodo API + 社区 apikey 是主流姿势，风险与对策已明确；聚合策略（中文标题优先豆瓣、集结构优先 TMDB）值得进 agent system prompt 而非代码——维持"可选 feature 默认关"决策。

5. **季映射失败模式**（`plugin-bangumi.md`）：合集命名（1-12Fin+SP）、SP 小数集号（SP0.89）是已知难点——加入 fixture 回归集的 hard 用例清单。
