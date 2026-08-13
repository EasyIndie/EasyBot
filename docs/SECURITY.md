# EasyBot 安全边界

> 本文说明插件系统与自动更新的**信任模型**：什么被保护、什么明确不保护、以及部署侧应该做什么兜底。

## 一句话结论

**签名 / verified 徽标只证明「代码来自该发布者、未被篡改」，不证明代码安全。** 插件没有沙箱，以宿主进程相同权限运行。真正的安全边界是：**只装可信来源 + 显式更新 + 容器化兜底**。

---

## 插件信任模型

### 签名覆盖对象

插件分发链路上有两样东西：**产物字节**（`.so`/`.dylib`/`.dll`）与**元数据**（`easybot-plugin.json`，含 `sha256`/`library`/`version`/`publisher`）。

- 签名的对象 = **产物字节本身**（`base64(ed25519_sign(lib_bytes))`）。
- 元数据被产物的 sha256 间接锚定：篡改元数据的 `sha256` 但保留签名 → 下载的产物与篡改后的 sha256 不匹配 → 拒；篡改 `sha256` **并**换产物 → 需要伪造发布者私钥 → 不可行。
- 元数据经 HTTPS（GitHub Releases，TLS）拉取。

结论：**产物字节签名 + TLS 元数据**足够，无需再签摘要清单。

### 验签时点（双重校验）

1. **安装**（`easybot plugin install`）：下载后 `sha256` 比对 → `verify_artifact` 验签，双通过才原子落位。
2. **加载**（启动）：读 `plugin.sig.json`，对磁盘上的库文件**重新验签**——防「安装后被替换」。

### 生产模式门禁

- **dev**：lenient——有签名则验，无签名 warn。
- **生产**（`--production` / `EASYBOT_ENV=production`）：启动时扫描 `plugins/` 目录，发现未签名插件且未设 `plugins.allow_untrusted: true` → 拒绝启动，提示走市场重装或显式信任。
- 这是**行为变更**：此前手放的未签名插件在生产模式可加载；现在必须 `plugin install`（带签名）或设 `allow_untrusted`。

### 信任语义（对齐 VS Code 1.97）

- 信任是**按发布者**而非按插件，存 `{home}/plugins/.trust`（用户级状态文件）。
- `--yes` **不自动**把发布者加入 `.trust`——CLI 安装 ≠ 自动信任，避免一次 `--yes` 后该发布者所有插件静默免检。
- 显式 `easybot plugin trust <publisher> --public-key <PUBLIC_KEY>` 才加入 `.trust`。
- 首次安装未信任发布者的插件 → 提示确认；API/UI 返回 `needsTrustConfirmation`，用户确认后带 `trust: true` 重试。

### 密钥生命周期

- `easybot-plugin-sign gen-keypair` 生成 ed25519 密钥对。
- **私钥只存发布者 GitHub Actions secret**（`PUBLISHER_PRIVATE_KEY`），不进仓库、不进任何配置。
- 公钥（verifying key）通过 PR 提交给 EasyBot 官方登记进 `trusted_publishers` 默认列表（maintainer 审核：源码审查 + gitleaks 扫描）后合入，随版本内置。v1 无证书链——官方仓库的维护权即信任根。

### 密钥泄露响应

- 官方从 `trusted_publishers` 移除该发布者公钥 → 发布新版（内置列表更新）。
- 客户端更新后：该发布者插件验签失败 → 加载拒绝并标记。
- **不自动卸载已装插件**（防供应链投毒机制被滥用为批量卸载工具），但启动时明确告警。

---

## 自动更新信任（主程序）

主程序更新链路同样有签名/校验（SHA256 + GitHub Releases + 原子替换 + 回滚），见 `docs/04 architecture.md` 自动更新一节。核心点：

- 更新是**显式触发**（`easybot update`），不是静默自动。
- 失败自动回滚（备份 + 配对恢复）。
- 启动时版本锁定：`DB.schema_version == SCHEMA_VERSION` 不匹配则拒绝启动。

---

## 容器化兜底（推荐部署）

因为插件无沙箱，生产部署建议把 EasyBot（含插件）跑在容器里，配合 Docker 的安全能力：

- **最小文件系统**：只挂载 `data/`、`plugins/` 与配置文件；`configs/`、`secrets/` 按需。
- **只读 rootfs**：`read_only: true` + 需要的目录单独 tmpfs。
- **非 root 用户**：容器内跑非 root UID。
- **cap_drop**：`cap_drop: [ALL]`。
- **网络**：仅出网访问平台 API 与市场（GitHub Releases），入站只开 API 端口。

Dev 循环示例见 `docs/docker-compose.plugin-dev.yml`（该示例偏开发，生产请收紧上述项）。

---

## 威胁模型摘要

| 防御 | 不防御（明确声明） |
|---|---|
| 恶意发布者伪造签名（ed25519 不可伪造）| 可信发布者本身作恶（签名无解，靠审核 + 容器兜底）|
| 下载/元数据篡改（sha256 + 签名 + TLS）| 本机已攻破用户替换已装文件（本地攻破无解）|
| 伪装插件名（`publisher/name` 限定 + curated 目录）| — |
| 密钥泄露 → 停用该发布者 | — |
| 更新攻击（显式更新 + 默认 pin 版本）| 静默自动更新推送恶意代码（不提供静默自动更新）|

### 边界用例速查

| 用例 | 行为 |
|---|---|
| 恶意元数据 `name` 含 `..`/`/` | 白名单 `[A-Za-z0-9_-]` 拒绝 |
| 恶意 `library` 文件名 | `PluginManifest::library_path()` 已拒绝对路径/`..` |
| 中间人换产物 | sha256 不匹配拒绝 |
| 中间人换元数据+产物 | 验签失败（私钥不可伪造）|
| 插件伪装内置平台名（`telegram`）| loader `PlatformConflict` 拒绝 |
| 两个插件同名不同发布者 | `publisher/name` 唯一限定 |
| `catalog.json` 不可达 | install 明确报错；已装插件不受影响 |
| 发布者密钥泄露 | 官方移除公钥 → 新版客户端拒绝该发布者 |

---

## 供应链安全建议（VS Code 教训）

验证徽章/签名是**弱缓解**——被滥用为信任信号的历史教训来自 VS Code 扩展生态（伪造 badge 的恶意扩展）。因此：

1. **默认信任官方**：EasyBot 官方发布者预置信任。
2. **显式更新**：`plugin update` 默认 pin 当前版本，`--latest`/`--channel` 才跨版本——不给静默推送恶意更新的机会。
3. **密钥扫描**：`plugin-publish.yml` 内置 gitleaks 扫描源码与 `.env`，含密钥即阻断发布。
4. **容器兜底**：即使某个可信发布者翻车，容器限制能把爆炸半径压到最小。

长期看，理想边界是插件沙箱（独立进程/seccomp）与 crates.io 审计，均在路线图之外。
