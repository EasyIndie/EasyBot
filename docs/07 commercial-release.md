# 商用发布与供应链验证

## 不可变发布流程

Release 工作流不会修改主分支或移动版本标签。发布前必须先在受保护分支完成版本号、`Cargo.lock`、快照和 CHANGELOG 更新，通过评审与 CI 后，再从该提交创建并推送标签：

```bash
scripts/release-preflight.sh 0.1.0
git tag -s v0.1.0 -m 'EasyBot v0.1.0'
git push origin v0.1.0
```

工作流会拒绝标签与 `Cargo.toml` 或 CHANGELOG 不一致的发布。失败后可以重新运行工作流或删除未完成的 GitHub Release，但不得删除、移动或复用已公开的 Git 标签；需要修复时发布新的补丁版本。

## 发布产物

每个平台发布包应包含：

- 签名/公证条件允许时处理过的 `easybot-*` 二进制；
- 每个目标对应的 SPDX JSON SBOM；
- 覆盖二进制和 SBOM 的 `checksums.txt`；
- GitHub Sigstore/SLSA 构建来源证明；
- 与容器 digest 绑定的镜像来源证明及 SBOM 证明。

Release 创建必须等待完整测试、Clippy、依赖/许可证策略和灾备演练通过。构建 job 使用标签中的只读源码，不在构建期间改写版本文件。

容器构建推送后，工作流按不可变 digest 使用 Trivy 扫描 OS 与应用库漏洞；任何 `CRITICAL` 或 `HIGH`（包括尚无修复版本的漏洞）都会以非零状态停止工作流。provenance 与 SBOM 证明只在扫描通过后生成，因此即使注册表短暂存在失败构建留下的 tag，`production-up.sh` 也无法把没有两类证明的镜像部署到生产。发布负责人仍应删除失败构建的 tag，并通过有期限、列明补偿控制和批准人的风险接受记录处理确需例外的 CVE；不得把扫描改成 `continue-on-error` 或降低退出码。

`.github/workflows/container-audit.yml` 每日重新扫描当前 `latest` 正式镜像，用于发现发布后新披露的高危漏洞。仓库必须为该工作流失败配置安全值班通知；失败后应定位受影响 digest、暂停新部署并按严重等级执行补丁发布或有期限的风险接受，不能仅重新运行工作流消除告警。

`Dockerfile` 与 `Dockerfile.release` 的 Rust、Debian 基础镜像同时保留可读 tag 和不可变的多架构 manifest digest。升级基础系统或 Rust 工具链时，应从 Docker Hub 官方仓库重新取得 manifest digest，审核 amd64/arm64 子清单及漏洞扫描结果后在同一变更中更新；发布预检会拒绝任何没有 `@sha256:` 的 `FROM` 引用。

所有第三方 GitHub Actions 均固定到 40 位 commit SHA，行尾版本注释仅用于可读性，不参与解析。升级 Action 时必须从其官方仓库解析目标标签（注释标签需取解引用后的 commit），审阅变更记录与权限需求后更新 SHA；`test-actions-pinning.sh` 会阻止分支、版本标签或短 SHA 重新进入 CI。

每个工作流还必须显式声明顶层 `permissions`，默认仅授予 `contents: read`；只有发布镜像、创建 Release、生成证明或标记 PR 的具体 job 才声明所需写权限。门禁会拒绝依赖仓库默认 Token 权限的工作流。

OpenAPI v1 的完整生成结果保存在 `crates/easybot-api/src/snapshots/` 契约快照中。端点、方法、认证、参数、响应码、schema 类型或必填字段变化都会令测试失败。兼容性新增也必须审阅快照 diff；删除、重命名、收紧输入或改变响应类型等破坏性变更不得直接修改 v1，应设计新版本路径和迁移/弃用窗口。禁止使用无审阅的自动快照更新掩盖契约变化。

仓库必须创建名为 `commercial-release` 的 GitHub Environment，至少配置一名不属于提交作者的 required reviewer，并禁止管理员绕过保护规则。发布工作流完成构建、测试和证明后会停在 `approve-commercial-release`；只有该 Environment 留下审批记录后，GitHub Release 与 GHCR 镜像两个公开发布 Job 才会获得执行资格。不得把该 Environment 改成无审核规则的空壳，审批人应核对版本、变更记录、迁移/回滚报告、法律批准和发布窗口。

## 客户端验证

下载同一 Release 的所有目标文件和 `checksums.txt` 到一个目录：

```bash
scripts/verify-release-assets.sh ./downloaded EasyIndie/EasyBot

# 先从 Release/部署批准记录取得 digest；禁止使用 latest 或版本 tag 部署
scripts/verify-container-image.sh \
  ghcr.io/easyindie/easybot@sha256:<64位digest> \
  EasyIndie/EasyBot
```

第一个命令先校验 SHA-256，再用 GitHub CLI 验证每个二进制的来源证明。容器验证脚本拒绝可变 tag，并分别验证 SLSA provenance 与 SPDX SBOM 证明，同时要求签名者是本仓库的 `.github/workflows/release.yml` 且拒绝自托管 runner 产生的证明。验证成功后，生产部署清单也必须保留同一个 digest，不能重新解析版本 tag。二进制 SBOM 证明可通过 SPDX predicate 验证：

```bash
gh attestation verify ./downloaded/easybot-x86_64-unknown-linux-musl \
  -R EasyIndie/EasyBot \
  --predicate-type https://spdx.dev/Document/v2.3
```

## 发布负责人清单

- 确认 CI、依赖审计、许可证策略、备份恢复和目标平台构建均通过；
- 检查 CHANGELOG 只描述实际交付内容，并列出不兼容变更和迁移步骤；
- 核对容器 digest、二进制校验和、SBOM 和证明均可验证；
- macOS 商用分发必须配置 Developer ID 和公证；Windows 对外分发应配置组织代码签名证书；
- 在隔离环境按文档执行一次全新安装、升级、回滚和数据恢复；
- 保存发布批准记录、测试结果、已知风险和回滚负责人；
- 若产物、标签或证明存在不一致，停止发布并创建新版本，不覆盖已发布资产。

Windows Authenticode 使用 `WINDOWS_CODE_SIGNING_CERT_BASE64` 和 `WINDOWS_CODE_SIGNING_CERT_PASSWORD` 两个仓库 Secret。macOS Secret 配置见[公证配置指南](other/macos-notarization.md)。缺少签名/公证凭据时，Release 会发布未签名桌面产物并在 Actions 中输出 warning；正式对外分发前必须补齐对应平台签名凭据，或在发布记录中明确标注该平台产物为 unsigned。
