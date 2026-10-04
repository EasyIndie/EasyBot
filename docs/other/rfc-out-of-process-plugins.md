# RFC-0001：统一进程外插件架构

> 状态：**Draft（待评审）** ｜ 2026-10-03
> 前置证据与 PoC：`docs/other/plugin-out-of-process-poc.md`
> 适用范围：全平台（Linux / macOS / Windows），P6 插件市场

---

## 1. 摘要

把插件从「**进程内 dlopen cdylib**」改为「**进程外可执行文件 + stdio IPC**」，并在**全平台统一**。
宿主保持单文件静态可分发；插件是独立进程，通过一份版本化协议通信。协议层预留 **WASM 运行时**作为第二种实现（同一协议、产物全平台通用）。

## 2. 动机

| 问题 | 现状 | 本 RFC |
|---|---|---|
| Linux 官方 Release 是 **musl 全静态**，`dlopen` 不可能 → 插件完全不可用 | ❌ | ✅ 进程外无需 dlopen |
| "静态可移植" 与 "进程内动态库" 不可兼得 | ❌ | ✅ 二者兼得 |
| 插件无沙箱、以宿主权限运行（`docs/18` 头号风险） | ❌ | ✅ 进程边界 + 可选沙箱 |
| FFI 分配器契约（`docs/17` 深坑：禁用 `#[global_allocator]`、跨 FFI 堆所有权） | ❌ | ✅ 进程外不存在 |
| 三端机制不一致（macOS/Windows 能加载，Linux 不能） | ❌ | ✅ 统一 |

**非目标**：不改动宿主 CLI/REST/WS 对外契约；不改变签名/信任语义；不追求"插件产物全平台通用"（那是 WASM 选项，见 §10）。

## 3. 设计

### 3.1 进程模型

```
宿主（单文件、可静态）
  └─ spawn（stdin/stdout 管道）─▶ 插件进程（可执行文件，任意语言）
        请求/响应 + 通知（逐行 JSON）
```

- **一插件一进程**（v1；多账号/多实例留待后续）。
- 生命周期完全由宿主管理：启动超时 → 健康（存活/心跳）→ 崩溃重启（退避）→ 随宿主退出回收。
- Linux 侧用进程组保证子进程被连带回收；`kill_on_drop` 兜底。

### 3.2 协议（`easybot-plugin-protocol`）

一行一个 JSON 对象；`protocol` 版本在握手中协商。

| 方向 | 报文 |
|---|---|
| 宿主 → 插件 | `{"id":<u64>,"method":"<name>","params":<json>}` |
| 插件 → 宿主（响应） | `{"id":<u64>,"ok":true,"result":<json>}` |
| 插件 → 宿主（错误） | `{"id":<u64>,"ok":false,"error":{"message":"…","kind":"transient\|permanent\|unsupported"}}` |
| 插件 → 宿主（通知） | `{"method":"<name>","params":<json>}`（无 id） |

**方法集**（映射 `PlatformAdapter`）：

| method | 说明 |
|---|---|
| `handshake` | 返回 `{protocol, platform, display_name, sdk_version, capabilities}` |
| `init` / `connect` / `disconnect` / `retry_transport` | 生命周期；`connect` 失败带 `error_kind` |
| `send` / `send_media` / `send_media_group` / `send_interactive` / `send_typing` / `send_draft` `answer_callback` / `edit_message` / `delete_message` | 出站 |
| `get_chat_info` / `list_chats` / `enrich_source` | 查询/富化 |
| `health` / `cursor_state` / `restore_cursor_state` | 健康与游标 |
| `shutdown` | 宿主请求退出 |

**通知方法**：`event`（发布 `GatewayEvent` 到宿主事件总线）、`log`（可选结构化日志）。

**错误模型**：`kind` 直接映射 `GatewayError` 分类（`transient`/`permanent`/`unsupported`），与现有健康监测分类对齐。

**媒体传输**（待决，见 §9）：优先"文件路径 + 共享临时目录"，避免大 payload 走 JSON。

### 3.3 清单（`plugin.yaml`）

```yaml
name: easybot-hello-adapter
display_name: Hello Adapter
description: …
version: 0.2.0
protocol: 1                 # 协议版本
sdk_version: 1
command: ./easybot-hello-adapter      # 相对插件目录；Windows 为 .exe
args: []                              # 可选
env: {}                               # 可选，注入环境变量
runtime: process                      # process（默认）| wasm（预留）
requires:
  easybot: '>=0.0.28'
```

变更点：`library: libxxx.so` → **`command: ./xxx`**；新增 `protocol`、`runtime`。

### 3.4 目录布局

```
plugins/<name>/
├── plugin.yaml
├── plugin.sig.json
├── easybot-hello-adapter          # 当前平台可执行文件（+x）
└── …（插件自带资源）
```

产物命名：`{name}-{triple}[.exe]`（如 `easybot-hello-adapter-x86_64-unknown-linux-musl`），安装端按宿主 triple 取用。**每个产物的 libc 由插件作者自定**——宿主不受影响（这是关键改进：坏插件只导致自身启动失败，不再拖垮整个插件系统）。

### 3.5 宿主侧实现

- `easybot-core/src/plugin/ipc.rs`：`IpcPluginAdapter`（从 PoC 提炼：spawn、oneshot 派发、事件发布、退出唤醒兜底）。
- `PluginLoader` → 进程外实现（直接落在 `loader.rs`，保留策略/验签/信任链；不新增并行加载器）
- `PluginManager`：安装/校验/落位**可执行文件**；签名校验对象改为可执行文件字节（ed25519 + sha256 流程不变）。
- 进程监管：`ConnectTimeout`、存活检测、崩溃退避重启（复用现有重连退避策略）。
- **安全护栏**：启动时若检测到"静态构建 + 存在插件目录"，明确告警并给出原因。

### 3.6 SDK

```rust
// 插件作者只改这一处：declare_plugin! → run_plugin!
run_plugin!(MyAdapter, MyAdapter::new);
```

`run_plugin!` 生成 `main()`，把 `PlatformAdapter` 暴露为 IPC 服务端。插件作者的 `PlatformAdapter` 实现**几乎不用改**。

## 4. 签名与信任

- 签名对象：**可执行文件字节**（与今天签 `.so` 同理，`plugin.sig.json` 结构不变）。
- 信任语义不变：按 publisher 粒度、`.trust`、`--yes` 不自动信任、生产门禁。
- 预留：清单可声明**权限需求**（网络/文件），为沙箱铺路。

## 5. 安全模型演进（对齐 `docs/18`）

- 进程边界本身即是隔离层（崩溃不互相拖垮）。
- 可选沙箱（v1 至少 Linux）：seccomp / 降权用户 / `bubblewrap`；macOS（`sandbox-exec`）、Windows（Job Object + 低权限令牌）后续。
- 威胁模型表更新：从"插件=宿主体内代码"改为"插件=子进程"，明确"签名只证作者+完整性，不证安全"这一条**保持不变**。

## 6. 迁移步骤

1. 新建 crate `easybot-plugin-protocol`（消息类型 + 版本 + `PlatformAdapter` ⇄ 协议映射）。
2. 提炼 PoC → `easybot-core/src/plugin/ipc.rs` + `loader.rs`（进程外扫描/工厂）。
3. SDK 增加 `run_plugin!`（与 `easybot-plugin-sdk` 的 `testing` 宿主对齐）。
4. 改写 `plugin-publish.yml`：产物从 cdylib 改为**可执行文件**（6 target 保持不变；Linux 产物可由插件作者自选静态/动态）。
5. 迁移 `EasyIndie/easybot-hello-adapter` 为可执行插件（含文档/指南同步）。
6. 移除 cdylib 加载路径（**破坏性**：`declare_plugin!` 废弃）。
7. 修订文档（见 §8）。

## 7. 影响与兼容性

| 项 | 影响 |
|---|---|
| 插件 ABI | **破坏性**（cdylib → 可执行文件）；插件生态目前仅 hello-adapter，成本可控 |
| 宿主 CLI / REST / WS | 不变 |
| 签名 / 信任 | 不变 |
| 单二进制分发 | 不变（宿主仍单文件；插件本就是独立产物） |
| 平台 | Linux 由"完全不可用"变为可用；macOS/Windows 由进程内切到进程外（机制统一） |
| 性能 | 进程边界 ≈ 数十 µs~亚 ms，IM 场景可忽略 |

## 8. 对现有文档的修订点

| 文档 | 修订 |
|---|---|
| `docs/16 plugin-guide.md` | 插件不再编译 cdylib，而是**可执行文件**；`run_plugin!`；产物命名与分发说明 |
| `docs/17 plugin-methodology.md` | **删除**「Linux 插件必须 musl 静态编译（宿主 musl-static）」错误结论；**删除**「FFI 分配器契约」整套（不再适用）；新增协议与进程监管章节 |
| `docs/18 plugin-security.md` | 威胁模型表改为进程边界 + 可选沙箱；签名对象改为可执行文件 |

## 9. 待决问题（Open Questions）

1. **媒体传输**：文件路径 + 共享临时目录，还是 base64 内联？（倾向文件路径）
2. **协议编码**：逐行 JSON vs length-prefixed（大 payload / 二进制）。v1 列 JSON lines，媒体走文件旁路。
3. **进程粒度**：一插件一进程是否够（多账号场景是否需要多实例）。
4. **沙箱默认**：v1 是否默认启用 Linux 沙箱，还是先"仅隔离不开箱"。
5. **是否保留进程内快路径**：建议 **不保留**（避免双套维护）；如未来确有需求再作为可选优化。
6. **过渡策略**：是否给 cdylib 保留一个 feature 门做一次性过渡，还是直接切换（倾向直接切换，生态小）。

## 10. WASM 扩展点（同一协议的第二运行时）

- 同一 `easybot-plugin-protocol`；宿主可加载 `runtime: wasm` 的插件（内嵌 wasmtime，静态可链接）。
- 收益：**一份 `.wasm` 全平台通用** + 最强沙箱（capability 模型）。
- 成本：插件网络 I/O 需宿主按能力代理；插件语言受限。
- 结论：作为**协议层的可选运行时**，而非替代进程外方案。

## 11. 验收标准（落地情况，2026-10-03）

- [x] `easybot-plugin-protocol` 有版本协商与单元测试 — `PROTOCOL_VERSION` + 13 测试
- [x] `IpcPluginAdapter` 通过端到端测试（含事件推送、崩溃恢复、退出回收）— `crates/easybot-core/tests/ipc_adapter.rs` 6 测试（崩溃检测 `health_status → Down`、退出唤醒未决请求）
- [x] **musl 静态宿主**可加载进程外插件 — `static-pie` 宿主驱动 handshake/init/connect/事件/send/shutdown 全通（本次回归重验）
- [ ] `easybot-hello-adapter` 以可执行插件形态**发布**并安装/加载成功 — 仓库**已本地适配并提交**（bin + `run_plugin!` + `command` + 发布模板 + 进程外测试），**待 EasyBot v0.0.42 发布后推送**（其 SDK 依赖指向 `v0.0.42`）
- [x] 签名/信任/生产门禁在可执行文件形态下行为不变 — 验签对象改为入口可执行文件字节；`PluginManager` 策略与测试未变
- [x] `docs/16/17/18` 同步修订 — 并含 `docs/04/15`、`CLAUDE.md`、`CHANGELOG.md`（Breaking Changes）
