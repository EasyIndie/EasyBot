# cdylib → 进程外插件 迁移指引

> 适用：把旧的 **cdylib（`dlopen` 动态库）插件**迁移为**进程外插件**（独立可执行文件 + stdio JSON 协议）。
> 面向插件作者（含官方样例仓库 [`EasyIndie/easybot-hello-adapter`](https://github.com/EasyIndie/easybot-hello-adapter)）。

## 为什么要迁移

EasyBot 官方 Linux 发行版是 **musl 全静态二进制**（`statically linked`，无 `INTERP`/`NEEDED`）。
静态二进制**没有动态加载器**，因此 `dlopen()` 在任何情况下都不可能成功——这与插件自己怎么编译无关：
即使插件是零依赖的 `.so`，静态宿主也打不开。

结论：**进程内插件在 Linux 发行版上无法工作**。进程外插件把插件作为独立子进程启动，
宿主通过 stdin/stdout 逐行 JSON 通信，静态宿主同样可用，并额外获得崩溃隔离。

## 1. Cargo.toml：cdylib → bin

```toml
# ❌ 旧（cdylib）
[lib]
crate-type = ["cdylib"]

# ✅ 新（bin）
[[bin]]
name = "easybot-my-plugin"   # 产物名 = 这个名字（连字符保留）
path = "src/main.rs"
```

依赖不变（SDK 换成新版本 tag 即可）：

```toml
[dependencies]
easybot-plugin-sdk = { git = "https://github.com/EasyIndie/EasyBot.git", tag = "v0.0.42" }
```

## 2. 代码：`declare_plugin!` → `run_plugin!`

适配器实现（`impl PlatformAdapter for ...`）**完全不用改**，只改入口。

```rust
// ❌ 旧：src/lib.rs
use easybot_plugin_sdk::prelude::*;
struct MyAdapter;
impl PlatformAdapter for MyAdapter { /* ... */ }
declare_plugin!(MyAdapter, MyAdapter::new);
```

```rust
// ✅ 新：src/main.rs
use easybot_plugin_sdk::prelude::*;

#[derive(Default)]
struct MyAdapter;
impl PlatformAdapter for MyAdapter { /* ... 原样保留 ... */ }

easybot_plugin_sdk::run_plugin!(MyAdapter, MyAdapter::default);
```

要点：

- `run_plugin!` 内部会建 tokio 运行时、读握手、驱动 `init/connect/send/...`、
  把适配器发布的 `EventBus` 事件回传给宿主，并在收到 `shutdown` 时退出。
- 适配器里通过 `event_bus.publish(...)` 推送的事件**会自动转发**给宿主，无需额外代码。
- 媒体发送收到的是**宿主写好的本地文件路径**（`MediaAttachment.url`），按普通本地文件读取即可。

## 3. 测试：用 SDK 的测试宿主

```toml
[dev-dependencies]
easybot-plugin-sdk = { git = "...", tag = "v0.0.42", features = ["testing"] }
tokio = { version = "1", features = ["macros", "rt"] }
```

```rust
// 内存宿主（快，不启进程）
let mut host = easybot_plugin_sdk::testing::PluginTestHost::new();

// 进程外宿主（真实 stdio 协议，覆盖静态宿主场景）
let host = easybot_plugin_sdk::testing::ProcessPluginTestHost::spawn(
    "target/debug/easybot-my-plugin", "my-platform", "My Plugin",
).await.unwrap();
```

## 4. 发布：产物改为可执行文件

`plugin-publish.yml` 是**自包含模板**——把主仓最新版整个覆盖过去即可（只引用公开 action + 40 位 SHA）。
相对旧版的改动：

| 项 | 旧（cdylib） | 新（bin） |
|---|---|---|
| 构建 | `cargo build --release`（默认产 lib） | `cargo zigbuild/build --release --target <triple> --bin <name>` |
| 产物文件名 | `lib{name_snake}.{so,dylib,dll}` | `{name}-{triple}[.exe]`（连字符保留，Windows 带 `.exe`） |
| Release asset | `lib{name}-{triple}.{so,dylib,dll}` | `{name}-{triple}[.exe]` |
| 元数据字段 | `artifacts.<triple>.library` | `artifacts.<triple>.command`（宿主仍兼容 `library` 别名） |

仍固定 **6 个 target triple**（Linux 两项为 musl，macOS 两项，Windows 两项），
macOS 的 `MACOSX_DEPLOYMENT_TARGET` 需 ≤ 宿主。进程外模式下**不再要求**插件静态链接。

签名流程不变（`easybot-plugin-sign`，ed25519 覆盖产物字节本身）。

## 5. 兼容性说明

- 宿主**同时**支持读取 `command`（新）与 `library`（旧别名）字段，存量元数据不会立刻失效。
- 但旧 cdylib 产物在**静态发行版**上依旧无法加载（见上），迁移是必需的。
- 安装/信任/更新语义不变：`easybot plugin install <publisher>/<name>`、
  `plugin trust <publisher> --public-key <k>`。

## 参考

- 架构与决策：`docs/other/rfc-out-of-process-plugins.md`
- 任务清单：`docs/other/plugin-oop-tasks.md`
- 协议定义：`crates/easybot-plugin-protocol/`
- 宿主侧实现：`crates/easybot-core/src/plugin/{ipc.rs,loader.rs}`
- 官方教学样例（含插件开发指南）：<https://github.com/EasyIndie/easybot-hello-adapter>
