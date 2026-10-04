# 进程外插件 PoC 结论（Linux 静态宿主下的插件路线）

> 状态：**PoC 已验证**（2026-10-03）。本文件记录问题、验证与推荐方向，供决策。
> 相关代码：`tests/integration/src/bin/ipc_mock_plugin.rs`、`tests/integration/tests/ipc_poc.rs`。

## 1. 问题：官方 Linux Release 无法加载任何插件

`file` 官方 Release 二进制（`easybot-x86_64-unknown-linux-musl`）：

```
ELF 64-bit LSB executable, x86-64, statically linked, stripped
```

**完全静态**（无 `.interp`、无 `DT_NEEDED`）。而**静态二进制没有动态加载器，`dlopen` 在原理上不可能工作**。

实测（决定性）：

| 宿主 | 加载「零依赖」的 `libnoop.so` | 结果 |
|---|---|---|
| 官方 musl 静态二进制 | `dlopen failed` | ❌ 连无任何依赖的 `.so` 都打不开 |
| gnu 开发版（`target/debug`） | `dlsym failed`（缺 ABI 符号） | ✅ dlopen 成功 |

因此：
- Linux Release（`.deb`/裸二进制/Alpine Docker）**永远加载不了插件**，与插件怎么编译无关；
- macOS / Windows 的 Rust 二进制默认动态链接，插件反而是好的 → 跨平台行为不一致。

`docs/17 plugin-methodology.md` 原表述「Linux 插件必须 musl 静态编译（宿主 musl-static，glibc `.so` 无法 dlopen）」的假设**不成立**——它只考虑了"glibc 插件不能加载"，没意识到**静态宿主连 musl 插件也无法 dlopen**。

## 2. 结论：把插件移出进程

"静态（可移植）" 与 "进程内 dlopen" 在同一进程里不可兼得：

| 宿主 | 可移植 | 进程内 dlopen |
|---|---|---|
| musl 静态 | ✅ | ❌ |
| glibc 动态 | ❌（绑定 glibc 版本） | ✅ |
| musl 动态 | ❌（需 musl loader） | ✅ |

要**同时**保住「可移植静态宿主」与「插件能力」，插件边界必须移到**进程外**（sidecar 可执行文件 + IPC），这也是 Terraform provider、LSP、go-plugin 等采用的范式。

## 3. PoC 内容与结果

### 3.1 组件（PoC 已升级进 `easybot-core`）

| 位置 | 作用 |
|---|---|
| `tests/plugins/ipc-mock-plugin/` | 插件进程（workspace 成员）：纯 std IO，逐行 JSON 协议，处理 `handshake/init/connect/send/disconnect/shutdown`，并在 `connect` 后**主动推送 `message.inbound`**（`init` 后推 `plugin.ready`） |
| `crates/easybot-core/src/plugin/ipc.rs` | 宿主侧 `IpcPluginAdapter`（实现 `PlatformAdapter`）：spawn 子进程、reader 派发响应/发布事件、媒体落盘（D1=A）、请求超时、退出唤醒 |
| `crates/easybot-core/tests/ipc_adapter.rs` | 端到端测试：生命周期 / 事件 / 进程退出兜底 / env 注入 / 缺少二进制报错 |
| `crates/easybot-plugin-protocol/` | 宿主与插件共享的协议类型与方法常量 |

### 3.2 协议（v0，逐行 JSON）

```
宿主 → 插件: {"id": <u64>, "method": "<name>", "params": {...}}
插件 → 宿主: {"id": <u64>, "result": {...}} | {"id": <u64>, "error": "<msg>"}
插件 → 宿主: {"event": "<type>", "data": {...}}      // 主动推送，无 id
```

### 3.3 验证结果

1. **端到端测试**（gnu 宿主，`cargo test`）：`init → connect → 收到插件推送事件 → send 请求/响应 → disconnect`，全绿；全工作区 `831 passed / 0 failed`。
2. **静态宿主证明**：把最小宿主用 `x86_64-unknown-linux-musl` 编成 **`static-pie linked`** 的**完全静态**可执行文件，spawn 进程外插件并跑完整生命周期——**成功**：

```
$ file host   → ELF 64-bit ... static-pie linked
$ ./host ./ipc-mock-plugin
handshake -> {"result":{"protocol":1,"platform":"ipc-mock","display_name":"IPC Mock Adapter","capabilities":[]}}
init      -> {"result":{"ok":true}}
connect   -> {"result":{"ok":true}}
event     -> {"event":"message.inbound","data":{"chat_id":"chat-1","text":"hello from out-of-process plugin",...}}
send      -> {"result":{"message_id":"ipc-c1-4","success":true}}
shutdown  -> {"result":{"ok":true}}
```

即：**静态可移植宿主 + 插件能力，二者可兼得**，且全程不用 dlopen。

## 4. 推荐迁移路径（若采纳）

1. **协议 crate**：抽出稳定的 `easybot-plugin-protocol`（消息类型 + 版本号 + 能力协商）。
2. **宿主代理**：把 PoC 的 `IpcPluginAdapter` 提炼进 `easybot-core/src/plugin/ipc.rs`（保留 `plugin-system` feature 门）。
3. **加载与安装**：`PluginManager`/`PluginLoader` 从"dlopen cdylib"改为"可执行文件 + 清单"；`install` 流水线下载/校验/落位的是**可执行文件**而非 `.so`/`.dylib`/`.dll`。签名机制可原样复用（对可执行文件字节签名）。
4. **SDK 运行时**：`easybot-plugin-sdk` 提供 `run_plugin!(MyAdapter)`——把 `PlatformAdapter` 暴露为 IPC 服务端，插件作者实现方式基本不变（只是从 `declare_plugin!` 改为 `run_plugin!`）。
5. **进程监管**：拉起/健康/重启/随宿主退出（PoC 已用 `kill_on_drop` + reader 退出唤醒兜底）。
6. **迁移现有插件**：`easybot-hello-adapter` 等改为可执行产物。

## 5. 收益与取舍

**收益**
- 宿主保持单文件静态、任意 Linux 可跑；插件不再受 libc 约束
- **崩溃隔离**（插件崩溃不再拖垮宿主）
- **可沙箱**（seccomp / 降权 / 容器），补上 `docs/18` 承认的"无沙箱"最大风险
- **消除 FFI 分配器契约**（进程外无跨 FFI 堆所有权，`docs/17` 的全局分配器约束整套消失）
- **插件可用任意语言**编写（只要实现协议）
- 跨平台行为统一（不再有"Linux 用不了、macOS 能用"）

**取舍**
- 插件 ABI 破坏性变更，现有 cdylib 插件需迁移
- 引入 IPC 协议与进程监管（启动略慢几十 ms；进程边界对 IM 场景可忽略）
- 需处理协议版本兼容与插件进程存活检测

**与 WASM 方案对比**：WASM（wasmtime）同样能保持静态宿主、沙箱最强、产物全平台通用；但插件的网络 I/O 必须由宿主按能力提供，对"自己要连各平台 API 的长连接适配器"改造量更大，且插件语言受限于 WASM 生态。**当前更推荐进程外方案**。

## 6. 过渡期建议（迁移落地前）

1. 不再对外宣称 Linux 支持市场插件；加载失败时输出明确诊断（含 `dlerror` 与"当前为静态构建"判定）。
2. 修正 `docs/17:159` 的错误假设。
3. 加护栏：检测到静态构建 + 存在插件目录 → 启动告警。
