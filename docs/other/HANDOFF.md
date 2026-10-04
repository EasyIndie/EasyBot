# 交接上下文（压缩版）

> 目的：替代长对话历史。**保持简短，勿扩写。** 最后更新：2026-10-03。

## 环境 / 命令速查

| 项 | 值 |
|---|---|
| 仓库根 | WSL `/mnt/e/EasyBot` |
| cargo | 先 `export PATH="/root/.cargo/bin:$PATH"` |
| 全量测试 | `cargo test --workspace --features "default,plugin-system"`（当前 **864 passed**） |
| 插件相关测试 | 先 `cargo build -p ipc-mock-plugin`；再 `cargo test -p easybot-core --features plugin-system --test ipc_adapter` |
| 前端测试 | `cd crates/easybot-api/templates && npm test`（29）/ `npm run lint` |
| 规范 | **不 git commit**（除非用户要求）；每任务改完跑 test+fmt+clippy |

## 当前任务：进程外插件架构（`docs/other/rfc-out-of-process-plugins.md`）

进度：**P0–P5 全部 23 个任务完成 ✅**（`docs/other/plugin-oop-tasks.md` 顶部有执行状态表）。
决策：**D1=A（媒体文件旁路）D2=A（直接切换，不保留 cdylib 过渡门）D3=A（v1 仅进程隔离）**。

**收尾状态**：`cargo test --workspace --features "default,plugin-system" --locked` → **862 passed / 0 failed**；
`fmt`/`clippy` 干净；静态 musl（`static-pie`）宿主端到端跑通进程外插件。
异步任务队列已清空——下一步等用户新指令。

### 已落地的关键文件

| 路径 | 作用 |
|---|---|
| `crates/easybot-plugin-protocol/` | 协议类型 + 方法常量（13 测试），workspace 成员 |
| `tests/plugins/ipc-mock-plugin/` | 测试用插件进程（workspace 成员） |
| `crates/easybot-core/src/plugin/ipc.rs` | 宿主侧 `IpcPluginAdapter` |
| `crates/easybot-core/src/plugin/manifest.rs` | 新增 `command`/`protocol`/`runtime` + `command_path()` |
| `crates/easybot-core/src/plugin/install.rs` | `synthesize_manifest` 写 `command` + `make_executable` |
| `crates/easybot-core/tests/ipc_adapter.rs` | 6 端到端测试 |
| `crates/easybot-plugin-sdk/src/runner.rs` | `run_plugin!` 服务端运行时 |

## 其它已完成（未提交）

1. 前端迭代 T0–T5 → `docs/other/frontend-tasks.md`（29 测试 + CI `frontend.yml`）
2. 修复 `gateway.local.yaml` 空文件重置全部配置 → `core/src/config/mod.rs` merge_configs Null guard
3. 微信适配器改 opt-in（需 `adapters.wechat.enabled: true`）→ `adapter/registry.rs` + `bin/src/main.rs`
4. 依赖升级/去重 + `utoipa` 6 + `serde_norway`（`Cargo.toml`/`Cargo.lock`）

## 关键结论（勿重复踩）

1. **官方 Linux Release 是 musl 全静态** → `dlopen` 不可能 → 进程内插件全部不可用（与插件编译方式无关）。这也是本任务的根因。
2. 静态宿主**连零依赖 `.so` 都打不开**（已用 `/tmp/libnoop.so` 实证）。
3. `dlopen failed` 日志**不含 dlerror**（libloading 限制）；诊断用 `ldd`/`ctypes.CDLL`。
4. `docs/17:159` 的"宿主 musl-static 可加载 musl 插件"结论**错误**，P5-3 需修。
5. `templates/gen/` 是 build.rs 产物，**禁止手改**；改源文件后 `cargo build -p easybot-api`。
6. 新增 workspace 成员必须登记到根 `Cargo.toml` 的 `[workspace].members`（显式列表）。
7. `PluginManifest` 是结构体字面量构造（无 `Default`）；加字段需同步补所有字面量。

## 临时文件（可清理，非必需）

- `/tmp/easybot-accept`（8081 验收实例，**当前已停**）、`/tmp/static-poc`、`/tmp/easybot-musl`、`/tmp/noop-*`
- `crates/easybot-api/templates/node_modules/`（已 gitignore）
