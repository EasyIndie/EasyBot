# EasyBot 前端改造修复计划

> 基于 2026-06-28 前端全面评估（综合评分 3.1/5），梳理 6 阶段 26 项改造任务。
>
> **状态**: Phase 1（移动端）、Phase 2（CSS 设计 Token、HTML 语义化、登录体验）、Phase 3（Toast、Modal、标签页注册表、AbortController）、Phase 4.2（highlight.js 离线化）、Phase 5.1（JS/CSS 模块拆分）、Phase 6（`:focus-visible`、`prefers-reduced-motion`、文档页搜索/回到顶部/复制按钮）已实现。
>
> **核心文件**：
> - `crates/easybot-api/templates/admin_layout.html` — 管理后台布局（JS/CSS 通过 build.rs 注入）
> - `crates/easybot-api/templates/js/admin.js` — 管理后台 JavaScript
> - `crates/easybot-api/templates/css/admin.css` — 管理后台样式
> - `crates/easybot-api/templates/home.html` — 首页
> - `crates/easybot-api/templates/docs_layout.html` — 文档模板
> - `crates/easybot-api/templates/vendor/` — highlight.js 等第三方资源（build.rs 内联）
> - `crates/easybot-api/build.rs` — 文档生成 + 管理后台拼接构建脚本
> - `crates/easybot-api/src/routes/home.rs` — 首页路由（版本替换）

---

## 评估摘要

| 维度 | 评分 | 关键发现 |
|------|------|---------|
| 架构设计 | ★★★★☆ | 零依赖、编译时嵌入、WebSocket 事件驱动，方向正确 |
| 代码质量 | ★★★☆☆ | 功能完备但单文件巨石 ~900行 JS 混在 HTML 中 |
| UI/UX | ★★★★☆ | 深色主题专业，乐观更新等交互细节到位 |
| 性能 | ★★★★☆ | 内联零开销、增量 DOM 更新；轮询频率偏高 |
| 安全性 | ★★★☆☆ | Argon2 + 401 登出正确；缺 CSP、token 存 localStorage |
| 可维护性 | ★★☆☆☆ | 最大短板：无模块化、大量重复模板、全局状态 15+ |
| 无障碍 | ★☆☆☆☆ | 几乎零 ARIA 支持，键盘导航缺失 |
| **综合** | **3.1/5** | MVP/内部工具合格，产品化需系统性改造 |

---

## 第一阶段：移动端适配 🔴 最高优先级

> 全部 8 项已完成 ✅

### 1.1 viewport meta & 移动端基础样式
- [x] 补充 `viewport-fit=cover` 支持刘海屏
- [x] 全局 `-webkit-tap-highlight-color: transparent` 消除 iOS 点击高亮
- [x] 全局 `touch-action: manipulation` 消除 300ms 延迟
- [x] 表单元素 `font-size: 16px` 防 iOS 缩放
- [x] body 添加 `overscroll-behavior: none`

### 1.2 标签栏移动端
- [x] 横向滚动 + 隐藏滚动条（`scrollbar-width: none`）
- [x] 左右渐变遮罩指示可滚动
- [x] 激活标签自动 `scrollIntoView({ inline: 'center' })`
- [x] `<480px 缩减 padding/font-size`

### 1.3 表格移动端
- [x] 所有 `<table>` 外层包裹 `overflow-x: auto` 容器
- [x] 表格 `min-width: 600px` 防止挤压
- [x] 首列 `position: sticky; left: 0`
- [x] <480px 消息表隐藏"角色"和"Chat"列
- [x] <480px 会话表隐藏"类型"和"创建时间"列

### 1.4 登录对话框移动端
- [x] <480px 全屏显示（`width: 100vw; border-radius: 0`）
- [x] iOS 键盘弹出时输入框自动上移
- [x] `autocomplete="current-password"`

### 1.5 消息发送表单移动端
- [x] <480px Target + Parse Mode 同行 flex 布局
- [x] textarea 行数适配
- [x] 发送按钮全宽

### 1.6 卡片/网格多断点
- [x] 900px 断点：3-4 列 → 2 列
- [x] 640px 断点：2 列 → 1 列
- [x] 480px 断点：额外间距优化

### 1.7 日志 & Config 编辑器移动端
- [x] 日志搜索/按钮在 <480px 垂直堆叠
- [x] Config textarea `font-size: 16px` 防缩放
- [x] 编辑器按钮组在窄屏换行

### 1.8 文档页移动端
- [x] 添加汉堡菜单按钮，默认折叠侧边栏
- [x] 折叠/展开动画
- [x] 点击链接自动折叠
- [x] 代码块 `overflow-x: auto`
- [x] 内容区 padding + 标题 font-size 缩减

---

## 第二阶段：安全修复 & CSS 工程化

> 已实现: 2.2 CSS 设计 Token, 2.4 HTML 语义化, 2.5 登录体验

### 2.1 Content-Security-Policy 响应头
- [ ] 在 `server.rs` 添加 CSP 中间件
- [ ] `default-src 'self'; script-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com; ...`

### 2.2 CSS 设计 Token
- [x] `:root` 定义 `--bg-primary`、`--text-secondary`、`--accent` 等 16 个变量
- [x] 三个 HTML 文件全部替换硬编码颜色为 `var(--xxx)`

### 2.3 公共渲染函数
- [ ] `renderMessageRow(m)` 统一消息行
- [ ] `renderStatusBadge(status, connected)` 统一状态徽章
- [ ] `renderProgressBar(percent)` 统一进度条

### 2.4 HTML 语义化 & 表单规范化
- [x] `<form>` + `<label>` 包裹登录和消息发送
- [x] `<nav>`、`<section>`、`<header>` 语义标签
- [x] `<meta name="description">`
- [x] `autocomplete` 属性

### 2.5 登录体验优化
- [x] 登录按钮 spinner + disabled 状态
- [x] 密码错误抖动动画
- [x] 成功 fade-out 过渡

---

## 第三阶段：交互升级

> 全部 5 项已完成 ✅

### 3.1 Toast 通知系统
- [x] HTML 添加 `#toast-container`（fixed 右上角）
- [x] `showToast(message, type)` + 滑入/滑出动画
- [x] 替换所有 `alert()` 调用

### 3.2 Modal 对话框
- [x] HTML 添加 `#detail-modal`（遮罩 + 居中卡片 + X 按钮）
- [x] `showDetailModal(title, data)` / `closeModal()`
- [x] ESC 关闭 + 遮罩关闭 + body 禁止滚动

### 3.3 标签页注册表
- [x] `tabRegistry` 对象：load / refresh / cleanup
- [x] `switchTab()` 注册表驱动
- [x] `handleGatewayEvent()` 基于注册表分发

### 3.4 AbortController 管理
- [x] `api()` 支持 `signal` 参数
- [x] `tabControllers` Map，切换标签 abort 旧请求

### 3.5 首页增强
- [ ] ~~"快速开始" 代码块~~（已移除）
- [ ] ~~"支持平台" 区块~~（已移除）
- [ ] 服务状态读取（可选）

---

## 第四阶段：性能 & 离线化

> 已实现: 4.2 highlight.js 离线化

### 4.1 系统信息轮询降频
- [ ] `refreshSystemInfo` 间隔 1s → 5s
- [ ] `tickUptime` 保持 1s

### 4.2 highlight.js 离线化
- [x] build.rs 内联 highlight.js + CSS 到 docs.html
- [x] 移除 CDN 依赖

### 4.3 请求缓存 + 日志优化
- [ ] `cachedApi()` 带 TTL 的 Map 缓存
- [ ] 日志 `DocumentFragment` 批量插入
- [ ] 截断上限 500 → 2000

---

## 第五阶段：架构演进

> 已实现: 5.1 JS/CSS 模块拆分

### 5.1 JS 模块化拆分
- [x] 新建 `templates/js/` 目录，拆分为 11 个文件
- [x] `build.rs` 按依赖顺序拼接 + 注入 `admin_layout.html`
- [x] CSS 同样拆分到 `templates/css/`
- [x] 原 admin.html → `admin_layout.html`（占位符）

### 5.2 前端错误监控
- [ ] `window.onerror` + `window.onunhandledrejection`
- [ ] 静默 catch 添加 `console.warn`

### 5.3 构建时模板组件化（可选）
- [ ] `{% include "header.html" %}` 语法
- [ ] 复用 toast-container、modal、header、footer

---

## 第六阶段：无障碍 & 细节打磨

> 已实现: 6.2 `:focus-visible`, 6.3 `prefers-reduced-motion`, 6.4 文档页增强

### 6.1 ARIA 标签 & 角色
- [x] 标签导航：`role="tablist/tab/tabpanel"` + `aria-selected/controls/labelledby`
- [x] 进度条：`role="progressbar"` + `aria-valuenow`
- [x] 对话框：`role="dialog"` + `aria-modal`

### 6.2 键盘导航
- [x] 标签页 ← → 方向键
- [x] Modal 焦点 Trap + ESC 关闭
- [x] `:focus-visible` 样式

### 6.3 色彩对比度修复
- [x] `--text-muted` 对比度 ≥ 4.5:1
- [x] `--text-faint` 对比度 ≥ 4.5:1
- [x] `prefers-reduced-motion` 禁用动画

### 6.4 文档页增强
- [x] 客户端文本搜索（过滤侧边栏）
- [x] "回到顶部" 浮动按钮
- [x] 代码块复制按钮

### 6.5 视觉一致性审查
- [x] 三页面统一 header/footer/间距/圆角

---

## 执行策略

| 阶段 | 任务 | 工时 | 已实现 | 剩余 |
|------|------|------|--------|------|
| 一：移动端 | 8 | 8-12h | 8 ✅ | 0 |
| 二：安全+CSS | 5 | 5-7h | 3 ✅ | 2 (2.1 CSP, 2.3 公共渲染函数) |
| 三：交互升级 | 5 | 6-9h | 5 ✅ | 0 |
| 四：性能 | 3 | 5-8h | 1 ✅ | 2 (4.1 轮询降频, 4.3 缓存) |
| 五：架构 | 3 | 8-12h | 1 ✅ | 2 (5.2 错误监控, 5.3 模板组件) |
| 六：无障碍 | 5 | 6-9h | 5 ✅ | 0 |
| **合计** | **29** | **38-57h** | **23 ✅** | **6** |

## 不纳入改造

| 项目 | 原因 |
|------|------|
| 引入 Vue/React | 与零依赖哲学冲突 |
| WebSocket token 加密 | wss 已提供传输层加密 |
| API key → httpOnly cookie | 需改造认证流程 |
| CSRF token | Bearer header 已防护 |
| 前端单元测试 | 模块化后重新评估 |
