# EasyBot 插件快速上手（15 分钟教程）

> 从零写一个 EasyBot 适配器插件：脚手架 → 接一个平台 HTTP API → 本地运行 → 测试 → 发布。
> 全程不需要 clone EasyBot 主仓。
>
> 对照示例：仓库内 [`plugins/example-hello-adapter/`](../plugins/example-hello-adapter/) 是本节教程的产物。
> 完整参考见 [`docs/plugin-guide.md`](plugin-guide.md)，方法论见 [`docs/plugin-methodology.md`](plugin-methodology.md)。

## 什么是插件

EasyBot 通过动态库（`.so` / `.dylib` / `.dll`）加载第三方 IM 适配器。插件作者只需依赖 `easybot-plugin-sdk` crate、实现 `PlatformAdapter` trait、调用一次 `declare_plugin!`，即可为 EasyBot 添加新的 IM 平台支持——不改主仓、不 fork。

```
plugins/<name>/
├── plugin.yaml          # 清单（name / sdk_version / author）
└── lib<name>.so         # 编译产物（宿主进程内 dlopen）
```

**前提**：安装了已编译好 `plugin-system` 特性的 `easybot`（`easybot --help` 里能看到 `plugin` 子命令）；本地有 Rust 工具链（`rustc --version` ≥ 1.94）。

---

## 第 1 步：脚手架（1 分钟）

```bash
easybot plugin new my-adapter
cd my-adapter
```

生成一个**独立可构建**的工程：

```
my-adapter/
├── Cargo.toml                  # cdylib + SDK git tag 依赖（tag 按当前 EasyBot 版本生成）
├── .cargo/config.toml          # 说明 [patch] 属于 Cargo.toml（本地联调用）
├── src/lib.rs                  # 完整 PlatformAdapter 骨架（TODO 占位）
├── plugin.yaml                 # 清单（name / display_name / sdk_version / author）
├── tests/unit.rs               # 单元测试：身份 / 能力 / 状态
├── tests/host_test.rs          # PluginTestHost 集成测试（离线）
├── README.md
├── .gitignore
├── LICENSE                     # MIT（占位，按需替换）
└── .github/workflows/
    └── plugin-publish.yml      # 发布者 CI 模板（copy 即用）
```

常用选项：`--target-dir <dir>` 指定父目录（默认当前目录）、`--author <name>` 署名（写入 plugin.yaml / LICENSE）。

> **要点**：`Cargo.toml` 的 SDK 依赖是 `git` + `tag`：
> ```toml
> easybot-plugin-sdk = { git = "https://github.com/EasyIndie/EasyBot.git", tag = "v0.0.33", package = "easybot-plugin-sdk", features = ["testing"] }
> ```
> 构建时自动拉取主仓为依赖，**不需要本地有 EasyBot**。首次拉取后由 cargo 缓存，之后只重编插件 crate，秒级迭代。
>
> 脚手架 `plugin new` 不依赖市场/宿主状态，`--dir` 指定配置目录对它没有影响。

---

## 第 2 步：先跑起来（2 分钟）

```bash
cargo build --release
cargo test
```

- `cargo build --release` 产物：`target/release/libmy_adapter.so`（Linux）/ `.dylib`（macOS）/ `my_adapter.dll`（Windows）。
- `cargo test` 跑两类测试：`tests/unit.rs`（纯逻辑）+ `tests/host_test.rs`（内存宿主 `PluginTestHost` 模拟 `attach → init → connect → send → 事件流`），**无需启动真实网关**。

此时生成的骨架 `send()` 会把收到的文本回显为 `message.inbound` 事件——这就是一个能编译、能测、能装入宿主的「最小适配器」。

---

## 第 3 步：接一个平台 HTTP API（5 分钟）

以最常见的「发送文本 + 轮询收消息」为例。打开 `src/lib.rs`，在 `HelloAdapter` 上加入 HTTP client 与 token 字段：

```rust
pub struct MyAdapter {
    state: AdapterState,
    event_bus: Option<Arc<EventBus>>,
    token: Option<String>,
    base_url: String,
    client: reqwest::Client,
}
```

`init()` 读取配置（凭据来自 `gateway.yaml` 的 `adapters.<name>` 段或适配器 env）：

```rust
async fn init(&mut self, config: AdapterConfig) -> Result<InitResult, GatewayError> {
    self.token = config.token.clone();
    if self.token.is_none() {
        return Ok(InitResult {
            ok: false,
            error: Some("my-adapter token required".into()),
        });
    }
    self.base_url = config.base_url.unwrap_or_else(|| "https://api.example.com".into());
    self.state = AdapterState::Starting;
    Ok(InitResult { ok: true, error: None })
}
```

`send()` 调用平台发送 API。**网络层失败必须用 `GatewayError::Transient` 包裹**（否则健康监测可能把瞬态网络错误误判为永久停用）：

```rust
async fn send(&self, params: SendTextParams) -> Result<SendResult, GatewayError> {
    let resp = self.client.post(format!("{}/send", self.base_url))
        .bearer_auth(self.token.as_deref().unwrap_or(""))
        .json(&serde_json::json!({ "chat_id": params.chat_id, "text": params.message.text }))
        .send().await
        .map_err(|e| GatewayError::Transient(e.to_string()))?;   // 网络失败 → Transient
    if !resp.status().is_success() {
        return Err(GatewayError::SendError(format!("platform returned {}", resp.status())));
    }
    Ok(SendResult::ok(format!("{}-{}", self.platform_name(), params.chat_id)))
}
```

收消息有两种模式（详见 guide）：

- **事件推送**：平台推 WebSocket / webhook → 在 `set_event_bus` 保存的 `EventBus` 上 `publish` `message.inbound` 事件。
- **长轮询**：`connect()` 里 `tokio::spawn` 一个轮询任务，每轮把新消息 publish 成事件。

> 后台轮询任务用 `tokio::spawn` 时**必须带并发上限**（`Semaphore` 或 `JoinSet`）——见 methodology「并发上限」。

---

## 第 4 步：本地运行（2 分钟）

把产物装入宿主 plugins 目录（dev 环境自动加载）：

```bash
mkdir -p ~/.easybot/plugins/my-adapter
cp target/release/libmy_adapter.so ~/.easybot/plugins/my-adapter/     # .dylib / .dll 同理
cp plugin.yaml ~/.easybot/plugins/my-adapter/
```

在宿主配置 `~/.easybot/gateway.yaml` 的 `adapters:` 段启用（如需凭据）：

```yaml
adapters:
  my-adapter:
    enabled: true
    token: "${MY_PLATFORM_TOKEN}"     # 凭据优先走 export / Docker env > .env
```

启动宿主并观察：

```bash
easybot --debug                       # 打开 debug 日志
# 看到类似：Loaded plugin 'my-adapter' (My Adapter) from .../plugins/my-adapter
easybot plugin list                   # 查看加载状态
easybot plugin inspect my-adapter     # 清单 / 签名状态 / 加载错误
```

调试期把 `MY_PLATFORM_TOKEN` 写进 `~/.easybot/.env`（chmod 600）即可。**永远不要**把真实凭据写进 `plugin.yaml` 或提交到 git。

> **生产模式**：`--production`（或 `EASYBOT_ENV=production`）会做签名扫描——手放的未签名插件会被拒绝。要解锁生产动态插件，走 `easybot plugin install`（签名校验）；确要信任未签名插件，设 `plugins.allow_untrusted: true`（见 SECURITY.md）。

---

## 第 5 步：发布（3 分钟）

发布走 `plugin-publish.yml`（脚手架已生成），交叉编译 6 个 target + gitleaks 扫描 + ed25519 签名 + GitHub Releases：

```bash
# 1. 生成发布者密钥对（私钥只存 GitHub Actions secret）
easybot-plugin-sign gen-keypair
#    输出 PUBLIC_KEY 行 → 通过 PR 提交给 EasyBot 官方登记 trusted_publishers（maintainer 审核）

# 2. 私钥存进插件仓库 Actions secret：PUBLISHER_PRIVATE_KEY
# 3. 按需改 .github/workflows/plugin-publish.yml 顶部的 env（CHANNEL / EASYBOT_REQUIRES / ...）

# 4. 打 tag 推送，CI 自动交叉编译 + 签名 + 发布
git tag v0.1.0
git push origin v0.1.0
```

用户侧安装：

```bash
easybot plugin install my-adapter            # 首次装第三方发布者需确认信任
easybot plugin install easybot/my-adapter    # 支持 publisher/name 限定
easybot plugin update my-adapter             # 更新默认 pin 当前版本
easybot plugin trust <publisher> --public-key <PUBLIC_KEY>   # 显式信任发布者
```

> **签名 ≠ 安全**：签名只证明「代码来自该发布者、未被篡改」，不证明代码无害。插件以宿主权限进程内运行，无沙箱。安装第三方发布者插件前请自行评估；生产环境建议把 EasyBot 跑在容器里（见 `docs/docker-compose.plugin-dev.yml` 与 `docs/SECURITY.md`）。

### 离线分发（`install --file`，空气隔离环境）

不走 GitHub Releases 时，直接给用户一个**带签名**的插件目录即可——`sign` 的 `--sig-json`
额外写出 `install --file` 读取的 `plugin.sig.json`：

```bash
# 1. 签名产物：--name 必须传 plugin.yaml 的 name（kebab-case 插件库名推导是下划线 crate 名）
easybot-plugin-sign sign --key "$PRIVATE_KEY" \
  --publisher my-org --version 0.1.0 --triple x86_64-apple-darwin \
  --artifact ./target/release/libmy_adapter.dylib --name my-adapter \
  --sig-json ./plugin.sig.json

# 2. 把 { plugin.yaml, libmy_adapter.dylib, plugin.sig.json } 组成一个目录交给用户
# 用户侧（与市场安装同一流水线：ABI 预检 + 信任确认 + 验签 + 原子落位，仅跳过下载）：
easybot plugin install my-adapter --file ./my-adapter-dist/
```

---

## 后续

- **完整参考**：`docs/plugin-guide.md` —— 生命周期状态机、capability 声明、媒体/交互/编辑消息、事件发布 vs 轮询、配置、错误分类、心跳、资源上限、ABI 纪律、调试技巧。
- **方法论**：`docs/plugin-methodology.md` —— 适配器设计原则、Transient/Permanent 错误分类、心跳语义、缓存/并发上限、安全边界。
- **本地联调 SDK**：不想每次让 cargo 拉 git，可把 SDK 指向本地主仓 checkout——在 `Cargo.toml` 取消注释 `[patch."https://github.com/EasyIndie/EasyBot.git"]` 并填本地路径。
