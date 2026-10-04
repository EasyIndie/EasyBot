# 进程外插件架构 — 任务清单（供经济模型执行）

> 依据 `docs/other/rfc-out-of-process-plugins.md` 拆解。每个任务尽量小、自包含、可独立验收。
> 执行模型请**逐任务执行并验证**，不要一次做完再测；**不要 git commit**（除非用户明确要求）。
> 证据与 PoC 见 `docs/other/plugin-out-of-process-poc.md`。

## 执行状态（更新 2026-10-03）

| 阶段 | 任务 | 状态 |
|---|---|---|
| P0 | P0-1..P0-4 协议 crate + mock 插件成员 | ✅ |
| P1 | P1-1..P1-4 IPC 适配器进 core + 测试 + 事件/兜底 + env | ✅ |
| P2 | P2-1..P2-5 清单 command/安全 / 进程外加载器 / 安装可执行 / 签名 / 监管 | ✅ |
| P3 | P3-1 `run_plugin!` + 服务端运行时 | ✅ |
| P3 | P3-2 测试宿主适配进程外（`ProcessPluginTestHost`） | ✅ |
| P3 | P3-3 SDK 文档/示例（`cargo doc` 0 告警） | ✅ |
| P4 | P4-1..P4-3 分发（publish 工作流 / 元数据 `command` / 迁移指引） | ✅ |
| P5 | P5-1..P5-4 切换与清理（默认走进程外 / 移除 cdylib / 文档 / 回归） | ✅ |

**全部 23 个任务完成。**

**回归**：`cargo test --workspace --features "default,plugin-system" --locked` → **862 passed / 0 failed**；
`cargo fmt --all --check` 干净；`cargo clippy --workspace --all-targets -- -D warnings` 干净；
静态 musl 宿主（`static-pie`）端到端驱动进程外插件通过（handshake/init/connect/事件/send/shutdown）。

**已落地位置**：
- `crates/easybot-plugin-protocol/`（协议类型 + 方法常量，13 测试）
- `tests/plugins/ipc-mock-plugin/`（测试用插件进程，workspace 成员）
- `crates/easybot-core/src/plugin/ipc.rs` + `loader.rs`（进程外）；`manifest.rs` 增 `command/protocol/runtime`
- `crates/easybot-core/tests/ipc_adapter.rs`（6 端到端测试）
- `crates/easybot-plugin-sdk/src/runner.rs` + `run_plugin!`；`testing.rs` 的 `ProcessPluginTestHost`
- `.github/workflows/plugin-publish.yml`（产物改 bin）；`docs/other/plugin-cdylib-to-process-migration.md`
- `plugin/loader.rs` 已改为进程外（dlopen/FFI/`declare_plugin!`/`mock-adapter` 全部移除）

---

## 环境 / 约定

| 项 | 说明 |
|---|---|
| 仓库根 | WSL `/mnt/e/EasyBot` |
| cargo | 先 `export PATH="/root/.cargo/bin:$PATH"` |
| 工作区成员 | **显式列表**在根 `Cargo.toml` 的 `[workspace].members`，新增 crate 必须在此登记 |
| 验证命令 | `cargo build -p <crate>` / `cargo test -p <crate> [--features plugin-system]` / `cargo fmt --all --check` / `cargo clippy ... -- -D warnings` |
| 提交 | 不 commit；每任务完成后报告「改了哪些文件 + 验收结果 + 偏离」 |
| 禁止 | 不要改动 `docs/16,17,18` 以外的既有文档语义；不要删除现有 cdylib 路径，**直到 P5-2 任务** |

---

## 决策门（✅ 已确认 2026-10-03）

| # | 决策 | **已定** | 阻塞任务 |
|---|---|---|---|
| **D1** | 媒体传输：文件旁路 vs base64 内联 | **A：文件旁路** | P1-4、P3-1 |
| **D2** | 过渡策略：直接切换 vs 保留 cdylib 门 | **A：直接切换** | P5-1、P5-2 |
| **D3** | v1 沙箱：仅进程隔离 vs 默认上沙箱 | **A：仅进程隔离** | P2-5 |

**决策含义（执行时须遵守）**

- **D1=A**：媒体走「宿主写入专用临时目录 + 协议只传路径」，**禁止 base64 内联大 payload**；临时文件由宿主创建与清理，且只把自己创建的路径交给插件。
- **D2=A**：**不保留**旧 cdylib 过渡门；P5-1 与 P5-2 在一个周期内直接完成（P5-1 仍先加「旧式清单告警」便于存量迁移）。
- **D3=A**：v1 **不做** seccomp/降权。**P5-3 必须在 `docs/18` 明确写出「进程外 v1 = 崩溃隔离，非安全沙箱」**，避免被误读为已做安全隔离。

---

## 阶段 0 — 协议 crate（不依赖其它任务，可最先做）

### P0-1 新建 `easybot-plugin-protocol` 骨架
- **文件**：新建 `crates/easybot-plugin-protocol/{Cargo.toml,src/lib.rs}`；修改根 `Cargo.toml` 的 `members`
- **步骤**：
  1. `Cargo.toml`：`[package] name = "easybot-plugin-protocol"`，`version/edition/license` 用 `workspace = true`，`publish = false`；依赖 `serde = { workspace = true, features = ["derive"] }`、`serde_json.workspace = true`、`[lints] workspace = true`
  2. `lib.rs`：`pub const PROTOCOL_VERSION: u32 = 1;` + `#![allow(missing_docs)]`（或补文档）+ 一句话模块说明
  3. 根 `Cargo.toml` members 追加 `"crates/easybot-plugin-protocol"`
- **验收**：`cargo build -p easybot-plugin-protocol` 通过；`cargo fmt --all --check` 通过

### P0-2 协议核心报文类型 + serde 往返测试
- **文件**：`crates/easybot-plugin-protocol/src/{lib.rs,message.rs}`（可拆模块）
- **步骤**：定义并 `#[derive(Serialize, Deserialize)]`
  - `Request { id: u64, method: String, params: serde_json::Value }`
  - `Response { id: u64, ok: bool, #[serde(skip_serializing_if="Option::is_none")] result: Option<serde_json::Value>, #[serde(skip_serializing_if="Option::is_none")] error: Option<ErrorPayload> }`
  - `Notification { method: String, params: serde_json::Value }`（无 id）
  - `ErrorPayload { message: String, #[serde(skip_serializing_if="Option::is_none")] kind: Option<String> }`
  - 方法名常量模块：`pub mod methods { pub const HANDSHAKE: &str = "handshake"; … }`
  - 提供 `Response::ok(id, value)` / `Response::err(id, message, kind)` 构造器
- **测试**：每个类型至少 2 条 serde 往返（含 `skip_serializing_if` 不输出 `None` 字段的断言）
- **验收**：`cargo test -p easybot-plugin-protocol` 全绿

### P0-3 生命周期方法 params/result 类型
- **文件**：`crates/easybot-plugin-protocol/src/lifecycle.rs`
- **步骤**：定义强类型（供宿主与 SDK 共用），至少覆盖：
  - `HandshakeResult { protocol: u32, platform: String, display_name: String, sdk_version: u32, capabilities: Vec<String> }`
  - `ConnectOutcome { ok: bool, error: Option<String>, error_kind: Option<String> }`
  - `SendParams { chat_id: String, text: String, parse_mode: Option<String>, reply_to: Option<String> }`
  - `SendOutcome { success: bool, message_id: Option<String>, error: Option<String> }`
  - `InitParams { config: serde_json::Value, home: Option<String> }`
- **测试**：serde 往返各 ≥1 条
- **验收**：`cargo test -p easybot-plugin-protocol` 全绿

### P0-4 mock 插件独立成 workspace 成员并改用 protocol crate
- **文件**：新建 `tests/plugins/ipc-mock-plugin/{Cargo.toml,src/main.rs}`；根 `Cargo.toml` members 追加；可删除 `tests/integration/src/bin/ipc_mock_plugin.rs` 与对应 `[[bin]]`
- **步骤**：
  1. crate 依赖 `easybot-plugin-protocol`（path）+ `serde_json.workspace = true`；`publish = false`
  2. `main.rs`：把 PoC 的 `tests/integration/src/bin/ipc_mock_plugin.rs` 迁移过来，用 protocol crate 的类型/常量替代字面量
- **验收**：`cargo build -p ipc-mock-plugin` 通过；产物 `target/debug/ipc-mock-plugin` 存在
- **依赖**：P0-1/P0-2/P0-3

---

## 阶段 1 — 宿主侧代理（`easybot-core`，feature `plugin-system`）

### P1-1 `IpcPluginAdapter` 提炼进 core
- **文件**：新建 `crates/easybot-core/src/plugin/ipc.rs`；修改 `crates/easybot-core/src/plugin/mod.rs`（`mod ipc;` + `pub use`，`#[cfg(feature = "plugin-system")]`）；`crates/easybot-core/Cargo.toml` 加 `easybot-plugin-protocol`（可选/feature 依赖）
- **步骤**：把 PoC 的 `IpcPluginAdapter` 迁入，用 protocol crate 类型替换手写 JSON；保持 `PlatformAdapter` 实现完整（`platform_name/display_name/capabilities/set_event_bus/init/connect/disconnect/state/health/send/get_chat_info/runtime_config/status_summary`）
- **验收**：`cargo clippy -p easybot-core --features plugin-system --all-targets -- -D warnings` 通过
- **依赖**：P0-3

### P1-2 core 集成测试：完整生命周期
- **文件**：`crates/easybot-core/tests/ipc_adapter.rs`（新建 `tests/` 目录）
- **步骤**：用 `tests/plugins/ipc-mock-plugin` 的产物路径（自行定位 `target/debug/ipc-mock-plugin`，参照 `tests/integration/src/lib.rs::find_mock_lib` 的健壮查找法）；断言 `init → connect → 收到事件 → send(message_id 正确) → disconnect(状态 Stopped)`
- **验收**：`cargo test -p easybot-core --features plugin-system --test ipc_adapter` 通过
- **依赖**：P0-4、P1-1

### P1-3 事件总线对接 + 异常兜底
- **文件**：`crates/easybot-core/src/plugin/ipc.rs`、`crates/easybot-core/tests/ipc_adapter.rs`
- **步骤**：
  1. 插件 `event` 通知 → `GatewayEvent::new(event_type, platform, data)` → `EventBus::publish`
  2. 子进程提前退出：reader 任务唤醒所有未决请求（返回错误而非永久挂起）
  3. `kill_on_drop(true)`；Linux 下把子进程放入独立进程组并在退出时回收
- **验收**：新增 2 条测试：① 事件能被 `event_bus.subscribe` 收到；② 插件进程被杀后，后续 `send()` 在合理时间内返回错误（不挂起）
- **依赖**：P1-2

### P1-4 配置与环境传递
- **文件**：`crates/easybot-core/src/plugin/ipc.rs`、protocol crate 的 `InitParams`
- **步骤**：`init` 时把 `AdapterConfig` 序列化进 `InitParams.config`，并注入 `EASYBOT_HOME`/`--dir` 对应的插件数据目录环境变量（供插件持久化凭据，如微信扫码）；子进程工作目录设为插件目录
- **验收**：mock 插件能读到传入的 `config` 与 env（测试断言其一）
- **依赖**：P1-1、D1

---

## 阶段 2 — 清单 / 加载 / 安装 / 监管

### P2-1 清单支持可执行文件入口
- **文件**：`crates/easybot-core/src/plugin/manifest.rs`
- **步骤**：
  1. `PluginManifest` 新增 `protocol: Option<u32>`、`command: Option<String>`、`runtime: Option<String>`（默认 `"process"`）；保留 `library` 但文档标注 **deprecated**
  2. 新增 `command_path(&self, plugin_dir) -> Result<PathBuf, String>`：安全校验**复用** `library_path` 的规则（禁绝对路径、禁 `..`、必须位于插件目录内）；`command` 为空时回退 `library`
  3. 至少 3 条单测：正常、拒绝绝对路径、拒绝 `..`
- **验收**：`cargo test -p easybot-core --features plugin-system manifest` 通过

### P2-2 `ProcessPluginLoader`（⚠️ 该模块已在 P5-2 中删除）

> **后续修订（P5-2）**：实现时发现 `ProcessPluginLoader` 与 `PluginLoader` 形成两套并行加载器，
> 且前者**不做签名/信任校验**（仅告警），属重复且危险的路径。最终**删除 `process_loader.rs`**，
> 把进程外逻辑直接落在 `loader.rs`（保留既有策略/验签/信任链），并新增
> `PluginError::LegacyCdylibPlugin` 给出明确迁移指引。下文为原始任务描述，仅作历史记录。
- **文件**：新建 `crates/easybot-core/src/plugin/process_loader.rs`；`crates/easybot-core/src/plugin/mod.rs` 导出
- **步骤**：扫描 `plugins/<name>/plugin.yaml` → 校验 `command_path` 存在且可执行 → 记录 `(name, display, command)`；提供 `register_all(&registry, event_bus)`：为每个插件构造 `IpcPluginAdapter` 工厂并 `registry.register(..., &[])`
- **验收**：单元/集成测试：临时目录放一个插件（指向 mock 二进制）+ 清单 → `register_all` 后 `registry.has_platform` 为真
- **依赖**：P2-1、P1-1

### P2-3 安装流水线支持可执行文件
- **文件**：`crates/easybot-core/src/plugin/install.rs`（及 `manager.rs` 中合成 `plugin.yaml` 的逻辑）
- **步骤**：
  1. artifact 选择从"库文件后缀"改为"可执行文件"（保留 `.so/.dylib/.dll` 的旧匹配以兼容过渡）
  2. 落位后 Unix 设 `0o755` 执行位（Windows 无需）
  3. 合成 `plugin.yaml` 时写入 `command`（与 `protocol`），不再写 `library`
- **验收**：新增测试：用 `--file` 安装一个可执行"插件目录/文件"，落位后 `plugin.yaml` 含 `command` 且文件可执行
- **依赖**：P2-1

### P2-4 签名校验对象 = 可执行文件
- **文件**：`crates/easybot-core/src/plugin/signing/*`、`install.rs`、`process_loader.rs`（启动时验签）
- **步骤**：核对 `verify_artifact` 是否为"路径无关的字节校验"；若是则仅调整调用点（把校验目标从 `library_path` 换成 `command_path`）；补一条"篡改可执行文件 → 验签失败"的测试
- **验收**：`cargo test -p easybot-core --features plugin-system signing` 通过；新增篡改测试通过

### P2-5 进程监管
- **文件**：`crates/easybot-core/src/plugin/ipc.rs`
- **步骤**（D3 决定是否含沙箱）：
  1. 启动超时（握手/连接超时 → 报明确错误）
  2. 崩溃检测 + 退避重启（复用 `compute_backoff` 思路）
  3. 宿主退出时回收子进程（进程组 + `kill_on_drop`）
- **验收**：测试：① 启动一个"握手即退出"的假插件 → 返回错误而非挂起；② 杀掉子进程 → 适配器状态转为 Failed/Down 并可被上层重启
- **依赖**：P1-3

---

## 阶段 3 — SDK

### P3-1 `run_plugin!` 宏 + IPC 服务端运行时
- **文件**：`crates/easybot-plugin-sdk/src/{lib.rs,runner.rs,ffi.rs}`
- **步骤**：
  1. 新增 `runner.rs`：读取 stdin 逐行 JSON，按 method 调用一个 `PlatformAdapter` 实例，写回响应；把适配器产生的事件通过 `event` 通知推送
  2. 新增 `run_plugin!(T, ctor)` 宏：生成 `fn main()` 调用 runner（替代 `declare_plugin!` 的 C ABI）
  3. `declare_plugin!` 保留但标注 deprecated（P5-2 移除）
- **验收**：`cargo build -p easybot-plugin-sdk --features testing`；用 mock 适配器写一个"SDK 侧 runner"测试（进程内直接喂 stdin/stdout 缓冲）通过
- **依赖**：P0-3、D1

### P3-2 测试宿主适配进程外模式
- **文件**：`crates/easybot-plugin-sdk/src/testing.rs`
- **步骤**：`PluginTestHost` 支持"以进程外方式启动一个插件可执行文件并断言生命周期/事件"；保留旧的进程内测试能力（deprecated）
- **验收**：`cargo test -p easybot-plugin-sdk --features testing` 通过

### P3-3 SDK 文档与示例
- **文件**：`crates/easybot-plugin-sdk/src/lib.rs` 文档注释；`docs/16` 的 SDK 片段（若涉及）
- **步骤**：给出最小 `run_plugin!` 示例（10 行内）
- **验收**：`cargo doc` 无告警（至少本 crate）

---

## 阶段 4 — 分发

### P4-1 `plugin-publish.yml` 改为构建可执行文件
- **文件**：`.github/workflows/plugin-publish.yml`
- **步骤**：产物从 cdylib 改为 bin（`cargo zigbuild --release --target <triple> --bin <name>`）；产物命名 `{name}-{triple}[.exe]`；`easybot-plugin.json` 的 `library` 字段改为 `command`；**保持 6 target 与 Action SHA 固定**
- **验收**：`bash scripts/test-actions-pinning.sh` 通过；YAML 语法正确（`bash -n` 不适用 YAML，可目测或装 actionlint）

### P4-2 清单/元数据格式同步
- **文件**：`plugin-publish.yml` 生成的 `easybot-plugin.json`、`crates/easybot-core/src/plugin/registry/*`（解析处）
- **步骤**：registry 解析支持 `command`；`easybot-plugin.json` 字段与宿主解析对齐
- **验收**：`cargo test -p easybot-core --features plugin-system registry` 通过

### P4-3 `easybot-hello-adapter` 迁移（独立仓库）
- **文件**：本仓仅更新链接与说明（`docs/16`、`CLAUDE.md` 的样例仓库引用）
- **步骤**：输出一份"迁移指引"（改用 `run_plugin!`、产物改为 bin、发布配置改动）
- **验收**：指引文档存在且与 P4-1 产物命名一致

---

## 阶段 5 — 切换与清理（破坏性，D2 决定时机）

### P5-1 启动扫描默认走 `ProcessPluginLoader`（⚠️ 实现方式已调整）

> **实现结果**：未新增并行加载器，而是让既有 `PluginLoader`（`loader.rs`）**直接**成为
> 进程外加载器（它本就承载策略/验签/信任链），并删除重复的 `ProcessPluginLoader`。
> 启动路径不变：`main.rs` → `PluginManager::load_all()` → `PluginLoader`。
> 旧式清单（仅 `library`）现在返回 `PluginError::LegacyCdylibPlugin`，错误信息内含
> 迁移指引链接。
- **文件**：`bin/src/main.rs`、`crates/easybot-core/src/plugin/manager.rs`
- **步骤**：插件加载默认使用 `ProcessPluginLoader`；检测到旧式 cdylib 清单（仅有 `library`）时**告警**并给出迁移提示
- **验收**：启动日志显示进程外加载；旧式清单触发明确告警

### P5-2 移除 cdylib 加载路径
- **文件**：`crates/easybot-core/src/plugin/loader.rs`（删除或仅留进程外）、`crates/easybot-plugin-sdk/src/ffi.rs` 的 `declare_plugin!`、`tests/plugins/mock-adapter`（改为 bin 插件或删除）
- **步骤**：删除 dlopen 路径与 C ABI 导出；清理 `libloading` 依赖（若无其它使用）
- **验收**：`cargo build --workspace --features "default,plugin-system"` 通过；全量测试通过

### P5-3 文档修订
- **文件**：`docs/16 plugin-guide.md`、`docs/17 plugin-methodology.md`、`docs/18 plugin-security.md`、`CLAUDE.md`、`CHANGELOG.md`
- **步骤**：
  - `docs/17`：**删除**「Linux 插件必须 musl 静态编译（宿主 musl-static）」错误结论；**删除**「FFI 分配器契约」整节（不再适用）；新增协议与进程监管章节
  - `docs/18`：威胁模型改为“插件=子进程”；签名对象改为可执行文件；沙箱章节更新；**必须明确写出「进程外 v1 = 崩溃隔离，非安全沙箱」（D3=A）**
  - `docs/16`：`run_plugin!`、产物命名、发布流程
  - `CLAUDE.md`：更新 Crate Layout / Key Patterns 中与插件相关的描述
  - `CHANGELOG.md`：`[Unreleased]` 记录破坏性变更与迁移说明
- **验收**：文档间交叉引用无死链；关键结论与实现一致

### P5-4 全量回归 + 静态宿主端到端
- **步骤**：`cargo test --workspace --features "default,plugin-system" --locked`；`cargo fmt --all --check`；`cargo clippy --workspace --features "default,plugin-system" --all-targets -- -D warnings`；重跑 PoC 的"静态 musl 宿主加载进程外插件"验证
- **验收**：全绿；静态宿主演示成功

---

## 执行顺序与并行

```
P0-1 ─▶ P0-2 ─▶ P0-3 ─┬─▶ P0-4 ─▶ P1-1 ─▶ P1-2 ─▶ P1-3 ─▶ (P1-4)
                      │
                      └─▶ P3-1 ─▶ P3-2 ─▶ P3-3
P2-1 ─▶ P2-2 ─▶ P2-3 ─▶ P2-4
P1-3 ─▶ P2-5
P4-1 ─▶ P4-2 ─▶ P4-3
P2-2 + P2-3 + P2-5 ─▶ P5-1 ─▶ P5-2 ─▶ P5-3 ─▶ P5-4
```

- P0 阶段可独立推进，不影响现有插件路径。
- P2 与 P3 可在 P0 完成后**并行**。
- P5 是破坏性切换，必须在 P2/P3 都验证通过后、且 D2 决策确认后执行。

## 风险与回滚

| 风险 | 缓解 |
|---|---|
| 子进程泄漏 | 进程组 + `kill_on_drop` + 退出回收；测试覆盖 |
| 协议版本漂移 | `PROTOCOL_VERSION` 常量 + 握手校验；不兼容时明确报错 |
| 安装流水线回归 | 保留旧 library 匹配做过渡；P2-3 单测覆盖两种形态 |
| 破坏性切换导致存量插件失效 | 生态仅 hello-adapter；P5-1 先告警一个周期（若 D2 选"保留门"） |
| 每任务改动过大 | 每任务独立 `cargo test` + `fmt` + `clippy`；失败即回退该任务 |

---

## 每任务完成报告格式

```
任务：Px-y <名称>
改动：<文件列表>
验收：<命令> → <结果>
偏离：<无 / 说明>
```
