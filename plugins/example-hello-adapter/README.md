# Hello Adapter

EasyBot 插件入门示例（DX-3）：一个最小可跑的 **echo 适配器**——演示声明 capability、
收文本回文本（事件发布）、日志，与 PluginTestHost 离线测试。

> **教程对照物**：本示例与 `easybot plugin new hello-adapter` 脚手架产出的工程
> **内容一致**，只是用仓库内 path 依赖替代了 SDK git 依赖（以作为 workspace 成员
> 被 CI 持续编译）。对照 `docs/plugin-quickstart.md` 阅读即可。

## 构建 / 测试（仓库内）

```bash
cargo test -p hello-adapter                  # 单元 + PluginTestHost 离线测试
cargo build -p hello-adapter --release       # 产出自包含 cdylib（libhello_adapter.so/dylib）
```

## 本地装入宿主（dev）

```bash
mkdir -p ~/.easybot/plugins/hello-adapter
cp target/release/libhello_adapter.{so,dylib} ~/.easybot/plugins/hello-adapter/
cp plugin.yaml ~/.easybot/plugins/hello-adapter/
```

宿主（dev 环境）启动后即自动加载；`easybot plugin list` 可查看加载状态。

## 结构与要点

- `src/lib.rs` — 完整 `PlatformAdapter` 骨架：所有必需方法 + `declare_plugin!`
  FFI 入口。`send()` 把收到的文本回显为 `message.inbound` 事件（演示事件发布）。
- `tests/unit.rs` — 单元测试：平台身份 / capability 声明 / 初始状态。
- `tests/host_test.rs` — PluginTestHost 集成测试：`attach → init → connect →
  send → 事件流`，离线跑通，无需启动真实网关。

## 发布

对外发布请用 `easybot plugin new <name>` 生成独立工程（SDK 走 git tag），
参考 `docs/plugin-quickstart.md`「发布」一节：gen-keypair → 登记公钥 → push tag，
`.github/workflows/plugin-publish.yml` 交叉编译 6 target + ed25519 签名 + Release。
