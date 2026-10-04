# 前端迭代任务清单（供经济模型执行）

> 本文件由前端评估报告拆解而来，每个任务尽量小、自包含、可独立验收。
> 执行模型请**逐任务执行并验证**，不要一次做完再测；不要 git commit（除非用户明确要求）。

## 执行状态（2026-10-02）

| 任务 | 状态 | 结果 |
|---|---|---|
| T0-1 补 `--bg-subtle` | ✅ | `admin.css` 已定义（后收敛到 `tokens.css`） |
| T0-2 修 `var(--primary)` | ✅ | 改为 `--accent` |
| T0-3 `api()` 非 JSON 防御 | ✅ | 改 `res.text()` + try/parse |
| T0-4 指标渲染转义 | ✅ | 5 处插值套 `escapeHtml` |
| T1-1 抽 `utils.js` | ✅ | `js/00-utils.js`（13 函数），build.rs 内联 |
| T1-2 vitest 单测 | ✅ | **29 tests 通过**（jsdom 引导） |
| T1-3 eslint | ✅ | 宽松配置，0 error |
| T1-4 CI `frontend.yml` | ✅ | npm ci/test/lint，Action SHA 固定门禁通过 |
| T2-1 build.rs glob + 粗切 | ✅ | 4 文件，无损切分 |
| T2-2 每 Tab 一文件 | ✅ | 共 **11 个 js 文件**，全部 ≤683 行 |
| T3-1 `tokens.css` 单一来源 | ✅ | 三页共用，`:root` 仅剩 1 处 |
| T3-2 `types.d.ts` | ✅ | 手写类型契约（对照 OpenAPI） |
| T4-1 自研 modal | ✅ | 6 处原生 confirm/prompt 全部替换 |
| T4-2 Tab 深链 | ✅ | hash 路由 + hashchange + `restoreTab` 重写 |
| T4-3 Config 语义 | ✅ | 409 语义化 + “重启后生效”提示 |
| T5-1 emoji → STG（Tab 栏） | ✅ | 9 个 tab 图标改内联 SVG（`currentColor`，可主题化） |
| T5-2 文档全文搜索 | ✅ | 标题+正文匹配、隐藏不匹配章节、显示“匹配 N/总” |
| T5-3 Tab `aria-controls` | ✅ | `aria-controls`/`aria-labelledby` + roving tabindex |

**回归**：`cargo test --workspace --features "default,plugin-system"` → 827 passed / 0 failed；前端 29 tests 通过；`cargo fmt --check` 通过；eslint 0 error。

## 执行环境与通用约定

| 项 | 说明 |
|---|---|
| 仓库根 | WSL `/mnt/e/EasyBot`（Windows `E:\EasyBot`） |
| cargo | 先 `export PATH="/root/.cargo/bin:$PATH"` |
| node/npm | `npm`（11.11.0）可直接用；`node` 不在 PATH，需要时用 `node.exe`（v24.14.1）；`npm`/`npx` 内部会自动解析 node |
| 前端源文件 | `crates/easybot-api/templates/`（**源文件，改这里**） |
| 前端产物 | `crates/easybot-api/templates/gen/`（gitignore，由 build.rs 生成，**禁止手改**） |
| 构建脚本 | `crates/easybot-api/build.rs`（把 CSS/JS 内联进 gen/*.html） |
| 验证命令 | `cargo build -p easybot-api`（重新生成 gen 产物并编译）；前端测试在 `crates/easybot-api/templates/` 下跑 `npm test` / `npm run lint` |
| npm 工作目录 | `cd /mnt/e/EasyBot/crates/easybot-api/templates` 后执行 npm 命令 |
| 提交 | 不 commit；每个任务完成后报告「改了哪些文件 + 验收结果」 |

**改前端源文件后必须验证的固定动作**：`cd /mnt/e/EasyBot && cargo build -p easybot-api`，确认能编译、gen 产物更新、无报错。

---

## 阶段 0 — 正确性修复（无依赖，可立即执行）

### T0-1 补定义 `--bg-subtle` CSS 变量
- **文件**：`crates/easybot-api/templates/css/admin.css`
- **做法**：在 `:root { ... }` 块内（`--bg-tertiary: #11111e;` 之后）新增一行 `  --bg-subtle: #0d1117;`
- **背景**：`js/admin.js:1905` 的内联样式用了 `var(--bg-subtle)`，但 CSS 从未定义，导致"下一步：配置 Target 授权"提示框背景失效。
- **验收**：`grep -n -- '--bg-subtle' templates/css/admin.css` 能命中定义；`cargo build -p easybot-api` 通过。

### T0-2 修复 `var(--primary)` 未定义
- **文件**：`crates/easybot-api/templates/js/admin.js`
- **做法**：把 `else if (type === 'system') typeColor = 'var(--primary)';` 改为 `else if (type === 'system') typeColor = 'var(--accent)';`（当前在第 2455 行附近）
- **背景**：`--primary` 不存在，调试面板 system 类型日志颜色渲染失效。
- **验收**：`grep -n 'var(--primary)' templates/js/admin.js` 无命中；`cargo build -p easybot-api` 通过。

### T0-3 `api()` 增加非 JSON 响应防御
- **文件**：`crates/easybot-api/templates/js/admin.js` 顶部 `api()` 函数（约 28–38 行）
- **做法**：把
  ```js
  const data = await res.json();
  if (!res.ok) throw new Error(data.error?.message || data.message || res.statusText);
  return data;
  ```
  替换为（与 `debugRequest`/`loadMetrics` 的防御逻辑对齐）：
  ```js
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch (_) { data = { message: text }; }
  if (!res.ok) throw new Error(data?.error?.message || data?.message || res.statusText);
  return data;
  ```
- **背景**：`api()` 强假设响应一定是 JSON，遇到 204/纯文本会抛 `JSON.parse` 异常。
- **验收**：`cargo build -p easybot-api` 通过；逻辑上 204/空响应不再抛解析错误。

### T0-4 指标可视化渲染转义（消除存储型 XSS 隐患）
- **文件**：`crates/easybot-api/templates/js/admin.js` 的 `renderMetricsVisual()` 函数
- **做法**：对以下 4 处**未转义**的插值套 `escapeHtml(...)`：
  1. HTTP 明细循环里的 `${h.method}` → `${escapeHtml(h.method)}`
  2. 同一循环里的 `${h.path}` → `${escapeHtml(h.path)}`
  3. 消息按平台循环里的 `${plat}` → `${escapeHtml(plat)}`
  4. 请求平均耗时循环里的 `${p[0]||''}` → `${escapeHtml(p[0]||'')}`，以及 `${p.slice(1).join(' ')||'/'}` → `${escapeHtml(p.slice(1).join(' ')||'/')}`
- **注意**：适配器状态循环里的 `escapeHtml(String(a.platform || ''))` 已转义，不要重复改。
- **验收**：`cargo build -p easybot-api` 通过；`renderMetricsVisual` 内 method/path/platform 插值点全部经过 `escapeHtml`。

---

## 阶段 1 — 工具链 + 测试地基（依赖阶段 0）

### T1-1 抽出纯函数到 `js/utils.js`
- **文件**：
  - 新建 `crates/easybot-api/templates/js/utils.js`
  - 修改 `crates/easybot-api/templates/js/admin.js`（删除被搬走的定义）
  - 修改 `crates/easybot-api/build.rs`（在 admin.js 前内联 utils.js）
- **搬入 utils.js 的内容**（全部为「无顶层 DOM 访问」的纯函数/纯数据）：
  - 常量：`MSG_TYPE_LABELS`、`MSG_TYPE_BADGE`
  - 函数：`msgTypeLabel`、`msgTypeBadgeClass`、`statusBadgeClass`、`truncateText`、`fmtDuration`、`adapterRetryText`、`platformBadgeClass`、`chatTypeBadgeClass`、`msgRoleBadgeClass`、`renderProgressBar`、`parsePrometheus`、`mbar`、`escapeHtml`
- **保留在 admin.js**（含 DOM 访问或副作用）：`renderMessageRow`、`tickAdapterCountdown`、`showToast`、`showDetailModal` 等。
- **build.rs 改动**：把
  ```rust
  let admin_js = std::fs::read_to_string(js_dir.join("admin.js")).unwrap_or_default();
  ```
  改为：
  ```rust
  let utils_js = std::fs::read_to_string(js_dir.join("utils.js")).unwrap_or_default();
  let admin_js = std::fs::read_to_string(js_dir.join("admin.js")).unwrap_or_default();
  let admin_js = format!("{}\n{}", utils_js, admin_js);
  ```
- **原理**：`function` 声明会被提升（hoisting），所以 utils.js 放前面、admin.js 里照样能调用这些函数，行为不变。
- **验收**：`cargo build -p easybot-api` 通过；`gen/admin.html` 中 utils 函数只出现一次；手测任意一个用到这些函数的页面（如 Overview 的运行时间、Messages 的类型徽章）正常。

### T1-2 建立 vitest + jsdom 单测（≥30 条断言）
- **文件**：
  - 新建 `crates/easybot-api/templates/package.json`
  - 新建 `crates/easybot-api/templates/tests/utils.test.js`
- **package.json 内容**（devDependency，仅供开发期，不影响运行时产物）：
  ```json
  {
    "name": "easybot-frontend",
    "private": true,
    "type": "module",
    "scripts": {
      "test": "vitest run"
    },
    "devDependencies": {
      "vitest": "^2.1.0",
      "jsdom": "^25.0.0"
    }
  }
  ```
- **测试引导**（在测试文件顶部加载 utils.js 为全局函数；这是「无 bundler 也能测 plain script」的标准做法）：
  ```js
  import { describe, it, expect, beforeAll } from 'vitest';
  import { readFileSync } from 'node:fs';
  import { fileURLToPath } from 'node:url';
  import { JSDOM } from 'jsdom';

  const code = readFileSync(fileURLToPath(new URL('../js/utils.js', import.meta.url)), 'utf8');
  beforeAll(() => {
    const dom = new JSDOM('<!doctype html><html><body></body></html>', { runScripts: 'outside-only' });
    dom.window.eval(code);
    globalThis.escapeHtml = dom.window.escapeHtml;
    globalThis.fmtDuration = dom.window.fmtDuration;
    globalThis.msgTypeLabel = dom.window.msgTypeLabel;
    globalThis.statusBadgeClass = dom.window.statusBadgeClass;
    globalThis.parsePrometheus = dom.window.parsePrometheus;
  });
  ```
- **必须覆盖的用例**（每类若干条，合计 ≥30 断言）：
  - `parsePrometheus`：普通指标、`#` 注释行、带 `{}` 标签、科学计数法、非法行忽略
  - `fmtDuration`：0→`1s`、59→`59s`、60→`1m`、61→`1m1s`、3600→`60m`
  - `statusBadgeClass`：Connected+Healthy→green；Connected+Degraded/Down→yellow；Failed+permanent→red；Failed 非永久→yellow；Connecting/Starting→blue；Reconnecting→yellow；其余→gray
  - `msgTypeLabel`：已知类型中文标签；未知类型回退原文；空→`文本`
  - `escapeHtml`：`<script>`、`&`、引号被转义
- **执行**：`cd crates/easybot-api/templates && npm install && npm test`
- **验收**：`npm test` 全绿，`Tests` 计数 ≥30；`cargo build -p easybot-api` 仍通过（工具链不影响产物）。

### T1-3 （可选）eslint 接入并清零 error
- **文件**：`crates/easybot-api/templates/package.json`（加 script 与 devDependency `eslint`）、新建 `eslint.config.js`
- **做法**：用宽松规则（`no-undef`、`no-unused-vars`、`eqeqeq`、`no-redeclare` 等）跑 `js/` 目录，修到 0 error。若 config 折腾超过 30 分钟，可在报告中标记「eslint 暂缓」并回退（本任务为可选，不阻塞后续）。
- **验收**：`npm run lint` 退出码 0，无 error 输出。

### T1-4 （可选）CI 接入 `frontend.yml`
- **文件**：新建 `.github/workflows/frontend.yml`
- **做法**：jobs 依次 `setup-node@v4`（node 22）→ `npm ci`（在 `crates/easybot-api/templates`）→ `npm test` → `npm run lint`；再加一步 `cargo build -p easybot-api` 验证 gen 产物可生成。
- **验收**：workflow YAML 语法正确（可用 `actionlint` 或本地目测），与现有 CI 不冲突。
- **注意**：若尚未完成 T1-2/T1-3，本任务跳过，等工具链就绪再补。

---

## 阶段 2 — admin.js 垂直切片（依赖阶段 1）

> 原则：**保持原文件内各段的相对顺序不变**（`function` 声明有提升、顶层语句顺序敏感），build.rs 按文件名排序拼接即可零行为变化。

### T2-1 build.rs 改为「按文件名排序拼接 js/*.js」+ 第一次粗切
- **文件**：`crates/easybot-api/build.rs`、`crates/easybot-api/templates/js/`
- **build.rs 改动**：把「读 utils.js + admin.js」替换为按文件名排序读取 `js_dir` 下所有 `.js` 并拼接：
  ```rust
  let mut js_files: Vec<_> = std::fs::read_dir(&js_dir)
      .unwrap_or_else(|_| std::fs::read_dir(".").unwrap())
      .filter_map(|e| e.ok())
      .filter(|e| e.path().extension().is_some_and(|ext| ext == "js"))
      .collect();
  js_files.sort_by_key(|e| e.file_name());
  let admin_js = js_files.iter().fold(String::new(), |mut acc, e| {
      if let Ok(s) = std::fs::read_to_string(e.path()) { acc.push_str(&s); acc.push('\n'); }
      acc
  });
  ```
  （写法可自行调整，但**必须按文件名排序**且**排除非 .js 文件**。）
- **第一次切分 admin.js**（按文件内 `// ─── Xxx ───` 分隔注释切，保留相对顺序），切为 3 个文件：
  - `js/10-core.js`：从开头到 `// ─── Modal 详情弹窗 ──` 段结束（含 API 包装、公共渲染工具、Toast、Modal）
  - `js/20-tabs.js`：`// ─── Overview Tab ──` 到 `// ─── API Key 管理 Tab ──` 段结束（含 Overview/Metrics/Logs/Adapters/Config/Sessions/Messages/Plugins/APIKey）
  - `js/30-tail.js`：`// ─── Tab 切换 ──` 到文件末尾（含 tab 注册表、WebSocket、登录、Error monitoring、Initialize）
  - `js/00-utils.js`：即阶段 1 的 `utils.js` 改名而来（`00-` 前缀保证排最前）
  - 删除原 `admin.js`
- **注意**：`escapeHtml` 原本定义在 API Key 段末尾，现在它已在 utils.js（00-utils.js）中，20-tabs.js 里调用它没有问题（提升）。
- **验收**：`cargo build -p easybot-api` 通过；`ls templates/js/` 只有 `00-utils.js / 10-core.js / 20-tabs.js / 30-tail.js`（无 admin.js）；手测登录 + 9 个 Tab 全部正常。

### T2-2 二次细切：每 Tab 一个文件（可选但推荐）
- **文件**：`crates/easybot-api/templates/js/`
- **做法**：把 `20-tabs.js` 按 `// ─── Xxx Tab ──` 分隔注释进一步切分，保持顺序，文件名加前缀：
  - `21-overview.js`（Overview + Metrics）
  - `22-logs.js`
  - `23-adapters.js`
  - `24-config.js`
  - `25-sessions.js`
  - `26-messages.js`
  - `27-plugins.js`
  - `28-apikeys.js`（API Key 管理，含调试面板）
  - 删除 `20-tabs.js`
- **验收**：`cargo build -p easybot-api` 通过；每个文件 ≤600 行（`28-apikeys.js` 允许 ≤800）；手测 9 个 Tab 无回归。

---

## 阶段 3 — 设计 token 单一来源 + 类型契约（依赖阶段 2）

### T3-1 设计 token 收敛到单一 `tokens.css`
- **文件**：
  - 新建 `crates/easybot-api/templates/css/tokens.css`（唯一 `:root` 色板，含 `--bg-subtle`）
  - 修改 `crates/easybot-api/templates/css/admin.css`（删除自己的 `:root`，改为被 build.rs 在 admin.css 之前内联 tokens.css）
  - 修改 `crates/easybot-api/templates/home_layout.html`、`docs_layout.html`（删除内联 `:root`）
  - 修改 `crates/easybot-api/build.rs`（对 admin/home/docs 三个页面都在 `<style>` 前注入 tokens.css）
- **验收**：`grep -rn ':root' templates/`（排除 gen/）只剩 `tokens.css` 一处；三页视觉不变；`cargo build -p easybot-api` 通过。

### T3-2 前端类型契约（手写 d.ts）
- **文件**：新建 `crates/easybot-api/templates/js/types.d.ts`
- **做法**：手写关键接口的 JSDoc/TS 类型声明，至少覆盖：`AdapterInfo`（platform/status/connected/health/permanent_failure/retry_attempt/next_retry_in_ms/last_error）、`Session`（key/platform/chat_id/source/custom_name/last_message/created_at）、`StoredMessage`（id/platform/chat_id/text/timestamp/role/raw_data）、`ApiKeySummary`（id/name/prefix/permissions/requests_per_minute/revoked/created_at）、`PluginSummary`（name/display_name/version/publisher/signed/signature_valid/enabled/load_error）。
- **验收**：文件存在且被 eslint 识别（若 T1-3 已做）；类型字段与后端 OpenAPI 定义一致（可对照 `/openapi.json`）。

---

## 阶段 4 — 交互体验打磨（依赖阶段 2）

### T4-1 用自研 modal 替换原生 `confirm()`/`prompt()`
- **文件**：`crates/easybot-api/templates/js/`（涉及 sessions/messages/plugins/apikeys 相关文件）
- **做法**：复用现有 `modal-overlay`/`modal-card` 样式（CSS 已存在），实现一个可复用的 `confirmDialog(message)` / `promptDialog(title, initialValue)`，并补齐：打开聚焦首个可交互元素、ESC/遮罩关闭、关闭后焦点还原、`aria-modal`。替换现有 6 处：删会话、改会话名、删/吊销 Key、卸载插件、（如有遗漏一并替换）。
- **验收**：`grep -rn 'confirm(\|prompt(' templates/js/` 无命中；键盘可完整操作一轮（Tab/ESC/Enter）；`cargo build -p easybot-api` 通过。

### T4-2 Tab 深链 + 重写 `restoreTab`
- **文件**：`crates/easybot-api/templates/js/30-tail.js`
- **做法**：`switchTab(name)` 时写 `location.hash = '#/' + name`；`restoreTab()` 改为优先读 `location.hash`（映射到 `tabRegistry`，非法值回退 overview），其次读 `sessionStorage`；监听 `hashchange` 同步切 Tab。
- **验收**：刷新/前进/后退能直达对应 Tab；非法 hash 不报错。

### T4-3 Config 编辑体验改进
- **文件**：`crates/easybot-api/templates/js/24-config.js`、`templates/admin_layout.html`（如加表单元素）
- **做法**：提供「表单化常用项 + 高级 JSON」双模式；保存成功后文案明确标注"重启后生效"；保留现有 JSON 行列级错误定位。若时间有限，最小改动为：仅在保存成功提示里加"（重启后生效）"并正确呈现 409 语义。
- **验收**：保存成功/失败/409 三种文案语义清晰；`cargo build -p easybot-api` 通过。

---

## 阶段 5 — 视觉/无障碍增强（可选，长期，按需排期）

### T5-1 emoji 图标 → 内联 SVG（可选）
- 统一多平台渲染与主题化；逐个替换 `📊📈📋🔌⚙️💬✉️🧩🔑` 等。

### T5-2 文档全文搜索（可选）
- `build.rs` 生成 `search-index.json`（标题 + 正文摘要），docs 页前端改为全文过滤，替换当前"只过滤侧边栏标题"。

### T5-3 Tab 无障碍补全（可选）
- 为 `tablist/tab/tabpanel` 补 `aria-controls`/`aria-labelledby`。

---

## 依赖与执行顺序

```
T0-1..T0-4（并行，立即）
   └─▶ T1-1 ─▶ T1-2 ─▶ T1-3/1-4（可选）
         └─▶ T2-1 ─▶ T2-2（推荐）
               ├─▶ T3-1 ─▶ T3-2
               ├─▶ T4-1 / T4-2 / T4-3（彼此独立，可并行）
               └─▶ T5-x（可选，长期）
```

**每完成一个任务，报告**：改了哪些文件、验收命令与结果、是否有偏离。

---

## 风险备忘

1. **T1-1/T2 的拼接顺序**：`function` 声明可乱序（提升），但 `const/let` 与顶层语句（`addEventListener`、`setInterval`、`initAuth()`）顺序敏感——**必须保持相对顺序不变**，仅做"切块 + 按名排序拼接"。
2. **`escapeHtml` 的 hoisting**：它原本定义在文件末尾却到处被用，切分后放到 `00-utils.js` 仍成立（函数声明提升 + 全局作用域）。
3. **npm 在 /mnt/e（Windows 盘）上安装可能较慢**，属正常；若 `npm install` 权限报错，在 `templates/` 下重试即可。
4. **绝不手改 `templates/gen/`**；一切以 `cargo build -p easybot-api` 重新生成为准。
