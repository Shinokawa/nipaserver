# nipaserver WebUI

Svelte 5（runes）+ Vite + TypeScript。无 UI 框架 / 无 Tailwind——设计 token 与组件样式移植自 `webui/design/mockup.html`（见 `src/app.css`）。

## 开发

```sh
npm ci
npm run dev        # http://localhost:5173，/api 代理到 http://127.0.0.1:11810
```

需要先在本机启动 nipa-server（默认端口 11810）；代理配置见 `vite.config.ts`。

## 构建

```sh
npm run build      # 产物输出到 dist/，nipa-server 启动时直接伺服
npm run check      # svelte-check 类型检查
```

## 结构

```
src/
  app.css               # 设计 token + 全部组件样式（照抄 mockup）
  main.ts / App.svelte  # 入口、侧边栏/顶栏骨架、hash 路由分发
  lib/
    types.ts            # API 与 agent 事件协议类型（契约 docs/03 §4）
    api.ts              # REST 客户端
    sse.svelte.ts       # 单一 EventSource + 指数退避重连 + 事件分发 store
    nav.svelte.ts       # hash 路由（含 #/item 与 #/player）
    format.ts           # 格式化 & art-g1..g7 海报占位分配
  components/
    AgentTimeline.svelte    # 事件流 timeline 渲染器（SSE 实时 / 回放同构）
  views/
    LibraryView.svelte      # 媒体库（首页 sections + 海报墙）
    DownloadsView.svelte    # BT 下载任务与 Mikan RSS 订阅管理
    ItemDetailView.svelte   # 条目详情、季/集、文件版本
    PlayerView.svelte       # Direct Play / hls.js 播放器 OSD
    StewardView.svelte      # 管家对话 + 会话历史
    ConsoleView.svelte      # Agent 控制台（队列 / timeline / 待确认）
    SettingsView.svelte     # 库管理 + 系统信息
```
