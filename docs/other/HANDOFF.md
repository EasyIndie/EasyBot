# 交接上下文（压缩版）

> 目的：替代长对话历史。**保持简短，勿扩写。** 最后更新：2026-10-05。

## 环境 / 命令速查

| 项 | 值 |
|---|---|
| 仓库根 | WSL `/mnt/e/EasyBot` |
| cargo | 先 `export PATH="/root/.cargo/bin:$PATH"` |
| 全量测试 | `cargo test --workspace --features "default,plugin-system" --locked`（当前 **861 passed**） |
| 插件测试 | 先 `cargo build -p ipc-mock-plugin`；再 `cargo test -p easybot-core --features plugin-system --test ipc_adapter` |
| 前端测试 | `cd crates/easybot-api/templates && npm test`（29）/ `npm run lint` |
| 发布预检 | `bash scripts/release-preflight.sh`（**禁止 `\| tail`**，用 `> /tmp/log 2>&1; echo $?`） |
| 推送门禁 | `.githooks/pre-push` → `scripts/verify-push.sh`（需 PATH 有 cargo，约 20 min） |
| 规范 | **不 git commit 除非用户要求**；`cargo fmt --all --check` + `clippy -D warnings` 必须干净 |

**`/tmp` 是 7.8G tmpfs 且常被他人占满** —— 大文件/构建请放 `/mnt/e/...`（盘子大）。

## GitHub 操作（已打通）

| 事项 | 说明 |
|---|---|
| 凭据 | git credential helper 可用（40 字符 token）；`git push` / API 均可 |
| `main` 保护 | **必须走 PR**（`pull_request` 规则，0 必需审批 + 必需状态检查）。直接 push main 被拒 |
| 可用流程 | 推分支 → `POST /repos/EasyIndie/EasyBot/pulls` → 等 CI 全绿 → `PUT .../pulls/<n>/merge`（`merge_method: rebase`）→ tag push |
| 取 token | `printf 'protocol=https\nhost=github.com\n\n' \| git credential fill \| sed -n 's/^password=//p'` |
| 无 `gh` CLI | 一律用 curl + GitHub REST API |

**rebase-merge 陷阱**：分支若基于**旧 main** 再 rebase，merge 时会重放导致提交标题错配（本项目已踩）。**正确做法**：分支从 `origin/main` 最新处切出。

## 当前版本状态

| 仓库 | main | tag | 状态 |
|---|---|---|---|
| `EasyIndie/EasyBot` | `aedc4e7` | **v0.0.42 / v0.0.43** | 均已发布（各 15 产物：6 平台二进制 + SPDX SBOM + deploy-kit + version.json） |
| `EasyIndie/easybot-hello-adapter` | `2d4ba06` | **v0.2.0** | 已发布（6 平台可执行文件 + ed25519 签名） |

**已合并 PR**：#148（v0.0.42 主体，118 文件）、#153（release.yml 修 ABI 路径）、#154（v0.0.43 双重 init 修复）。

**已知瑕疵（用户选择「不修」= 方案 C）**：`main` 上 `67f203b` 标题是「feat!: migrate plugins…」但内容只有 `release.yml` 的 19 行修复。原因：rebase-merge 重放。**修复它需要改写已被 v0.0.42/v0.0.43 tag 锚定的提交 → 不划算，保持现状。**

## 本会话完成的大事（均已发布验证）

1. **进程外插件架构**（破坏性）：`crates/easybot-plugin-protocol`（协议）+ `plugin/ipc.rs`（宿主 `IpcPluginAdapter`）+ `plugin/loader.rs`（进程外扫描/验签/工厂）+ SDK `run_plugin!`（替代 `declare_plugin!`/FFI）。**dlopen / libloading / FFI / PluginAdapterProxy / mock-adapter 全部移除**（含 Dockerfile、CI、脚本残留）。
2. 全量审计跟进（模块拆分）、前端切片重构、依赖升级（utoipa 6 / serde_norway）。
3. hello-adapter 迁移为进程外 + 发版 v0.2.0。

### 发布过程中修掉的 4 个真实缺陷

| 缺陷 | 说明 |
|---|---|
| `Dockerfile` COPY 已删除的 `tests/plugins/mock-adapter` | Docker 作业失败 |
| `release.yml` 从已删除的 `ffi.rs` 读 ABI 常量 | v0.0.42 发布失败 |
| `breaking_changes` 硬编码空 | 升级用户看不到破坏性提示 → 改为从 CHANGELOG 自动提取 |
| 🔴 **`IpcPluginAdapter::init` 非幂等** | `AdapterManager::start()` 在工厂 init 后又 init 一次 → 第二个子进程替换第一个 → 旧 reader 在 EOF 时把「进程已退出」投给新握手 → **v0.0.42 上插件永远无法启动**。v0.0.43 修复（init 幂等 + pending 表按 spawn 换代 + 回归测试） |

### 终验（发布产物 + 市场安装）

```
v0.0.43 musl 静态二进制 → plugin install EasyIndie/easybot-hello-adapter（验签通过）
→ Adapter 'easybot-hello-adapter' connected → health {total:1, connected:1}
→ POST /messages/send 回环 OK → 1 个插件子进程（进程隔离）
```

## 关键结论（勿重复踩）

1. **官方 Linux Release 是 musl 全静态** → `dlopen` 永远不可能成功（与插件怎么编译无关）。
2. `docs/17:159` 的错误结论已修；「FFI 分配器契约」整节已删（架构废止）。
3. 发布版二进制跑明文 HTTP 需 `EASYBOT_ALLOW_PLAINTEXT=true`（发布版的硬门禁）。
4. Admin 登录是 **`POST /admin/login`**（不在 `/api/v1` 下），返回 `{"key":"eb_..."}`，用作 `Authorization: Bearer <key>`。
5. 新增 workspace 成员必须登记根 `Cargo.toml`；**非成员包必须进 `workspace.exclude`**（否则 `cargo build` 报 workspace 错误）。
6. `templates/gen/` 是 build.rs 产物，禁改；改源文件后 `cargo build -p easybot-api`。
7. **`/mnt/e` 上 drvfs 会把文件权限变成 755** → 在 `/mnt/e` 的仓库里 `git status` 会显示大量 mode 变更，用 `git config core.fileMode false` 屏蔽。
8. 修改 `Cargo.lock` 后必须重跑一次非 `--locked` 构建，否则 `--locked` 测试会挂。

## 最终审计结论：旧动态加载逻辑已清理干净

`libloading` 在 Cargo.lock = **0**；`dlopen`/FFI 只剩注释与错误文案；`declare_plugin!` = 0；`cdylib` 声明 = 0。

## 待办（`docs/other/todo-list.md`）

| 项 | 状态 |
|---|---|
| ~~GHCR org PAT~~ | ✅ **不需要了** —— `GHCR_PRUNE_TOKEN` 已配置且 `ghcr-prune.yml` 8 次运行全 success。用途：`docker.yml` merge 作业 M1 + `ghcr-prune.yml` 周任务 M2，调用 `scripts/ghcr-reconcile.py --apply`（org 级包，repo GITHUB_TOKEN 无权删） |
| **macOS 公证 secrets** | ⏳ 未配置（5 个 `APPLE_*`）→ 每次发布只打 warning，**发布未签名 macOS 二进制**（v0.0.42/v0.0.43 已实证）。用途：`codesign --options runtime` + `notarytool submit`，消除 Gatekeeper 拦截 |
| 飞书视频封面 | 待产品决策（`SendMediaParams` 无 `cover` 字段） |
| Discord >2500 guild 分片 | `/gateway/bot` 失败回退单分片时超大集群可能被 4010 拒连 |
| 商业运营证据 | 法务/支付/告警/容量/灾备，需真实环境验收 |

### 两处待修的文档问题（已发现，未修）

1. `docs/other/macos-notarization.md` §「不设置凭据时」声称**「商业 Release 会失败并停止发布」** —— **错误**，实际是 warning 后继续发布未签名产物（全仓无强制门禁）。应改文档，或补真门禁。
2. README / `docs/01 user-guide.md` 未告知 macOS 用户当前的 Gatekeeper 拦截绕过方式（`xattr -d com.apple.quarantine`）。

## 临时文件

已清理干净（`/tmp/e2e42`、`/tmp/rel43`、`easybot-validate` 镜像均已删除）。插件工作副本保留在 `/mnt/e/_plugin-work/hello-adapter`。
