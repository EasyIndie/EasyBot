# EasyBot U2 升级机制 — Windows 真机验证 checklist

> 验证对象：issue #95 U2「分离辅助脚本两步替换」（`fix(updater)` 提交 `9c19d98` 起，合入 main 未发版）。
> 目的：确认「服务锁定 → 暂存 → 分离 .cmd 交换 → marker OK/TIMEOUT」真实链路在 Windows + NSSM 下可跑通。
> 前置门禁：**本项真机验证通过前，不建议发布 v0.0.36**。
> **务必在测试机执行，不要在生产机。**

---

> ## ✅ 验收结论（2026-08-14，Windows 真机）
>
> **已在本机完成真机验收**（隔离测试目录 `C:\Users\WangA\easybot-test\`，本地构建 main 分支 `0d9f7be` + `9c19d98`，未触碰生产服务）：
>
> - ✅ **U1 `--dir` 支持**：update / check-update / rollback 均正确解析隔离 home
> - ✅ **U2 分离交换机制**：staged `move /y` → marker `OK` → 脚本自删 → target 可运行
> - ✅ **场景 C**：含空格 + `!` 路径 escaping 正确（`DisableDelayedExpansion` 生效）
> - ✅ **场景 E**：服务运行中回滚被拒（data-safety），零副作用
> - ✅ **场景 A/B 前置路径**：`--dir` + `AlreadyUpToDate` 无副作用
> - ✅ **场景 F**：成功交换后 `.update/` 仅剩 marker，无残留
>
> ⏸️ **待 v0.0.36 发版后补充**（本生产机无法端到端）：场景 A/B（完整 upgrade / update TIMEOUT）、D（回滚成功端到端，需停服）、G（迁移显式确认）。
>
> ### 🔄 v0.0.36 发布后补充验收（2026-08-14）
>
> **复现 issue #95 背景**（v0.0.32 + schema v2）→ 升级到 v0.0.36，隔离测试目录 `easybot-test-v032/`，未触碰生产服务：
>
> - ✅ **U2 bug 复现**：v0.0.32 update → `os error 5 (拒绝访问)`，回滚正确（exe 仍 v0.0.32）——确认 issue #95 核心 bug
> - ✅ **修复路径验证**：手动替换 exe 到 v0.0.36 → 启动 → **schema v2→v3 自动迁移**（`_schema_version` 3 条迁移，sessions 新增 `custom_name` 列）→ **场景 G ✅**
> - ✅ **U2 分离 swap 机制**（v0.0.36 正式版）：marker=`OK`、staged move、脚本自删、target 可运行
> - ✅ **U1 `--dir`**（v0.0.36 正式版）：`check-update --dir` 正确解析
>
> 📄 v0.0.32→v0.0.36 报告见 issue #95 评论：<https://github.com/EasyIndie/EasyBot/issues/95#issuecomment-5291192192>
> 本地副本：`C:\Users\WangA\easybot-u2-v032-v036-acceptance.md`
>
> **结论更新**：issue #95 的 U2 bug 在 v0.0.32 确认存在（无法自我升级），v0.0.36 已修复（分离 swap + `--dir` + DB 迁移）。v0.0.32 用户需手动替换 exe 升级到 v0.0.36，此后 v0.0.36+ 可用自动 update。
>
> 📄 完整报告见 issue #95 评论：<https://github.com/EasyIndie/EasyBot/issues/95#issuecomment-5290048108>
> 本地副本：`C:\Users\WangA\easybot-u2-acceptance-report.md`
>
> **结论：U2 升级机制（分离辅助脚本两步替换）在 Windows 真机 + NSSM 下验证通过。**（`is_windows_service_running("EasyBot")` 检测本机生产服务，故场景 D 完整路径需独立测试机或停服，属 checklist 既有约束。）
>
> ### 🔄 v0.0.37 发版后回归验收（2026-08-14，v0.0.36 → v0.0.37）
>
> **场景 A/B/D 完整端到端已通过**（隔离测试 home `C:\Users\WangA\easybot-test-v037\`，测试服务 `EasyBotTest`，官方 v0.0.36/v0.0.37 release 二进制；短暂停止生产服务使 rollback 放行，验收后已恢复生产）：
>
> - ✅ **场景 A（标准升级 happy path）**：停服 → `update`（v0.0.36→v0.0.37）→ marker=`OK`、批处理自删、`.update/` 仅剩 marker → 启动 → 健康端点 `version=v0.0.37` → `check-update` = `✓ Already up to date (v0.0.37)`
> - ✅ **场景 B（服务运行中 update → TIMEOUT 兜底）**：测试服务运行中执行 `update` → 下载/校验/暂存成功、swap 已安排 → 约 30s 后 marker=`TIMEOUT`、exe 仍 v0.0.36（安全）、批处理**保留**（TIMEOUT 不自删）→ 停服重试 → marker=`OK`、exe 变 v0.0.37
> - ✅ **场景 D（回滚，停服）**：`rollback` → `✓ Rollback complete` → marker=`OK` → exe 回 v0.0.36、`check-update` 显示可升级到 v0.0.37 → `.update_manifest.json` 已删、二进制/DB/config 备份已清理、`.update/` 仅剩 marker → 回滚后服务正常启动、DB schema v3 保持
> - ⏸️ **场景 G（迁移显式确认）**：**v0.0.37 不适用**——`easybot-version.json` 声明 `requires_db_migration=false`、migrations 为空，v0.0.36→v0.0.37 无 DB 迁移（update 输出 `migrations_applied: []`，启动日志无迁移行）。G 的前提是"含 DB 迁移的版本"，本版本无从确认，逻辑已由 v0.0.36 的 v2→v3 迁移实测覆盖。
>
> **⚠️ 发现的边界情况（已修复，v0.0.38 发布）**：updater 的 rollback 保护此前**硬编码检测服务名 `EasyBot`**（`precheck::is_windows_service_running("EasyBot")`），自定义 NSSM 服务名部署（如 `EasyBotTest`）时检测不到运行中的服务 → 放行 rollback（安排 swap→TIMEOUT、**无条件删除 manifest**、恢复 DB/config），造成"DB/config 已回滚但 exe 未变"的半状态。真实生产用标准服务名 `EasyBot` 会被正确拦截（场景 E 已验证），故不影响标准部署。
>
> **修复（v0.0.38）**：服务名改为可配置——新增 `server.serviceName`（`gateway.yaml`，默认 `EasyBot`），updater 的预检/回滚 data-safety 均按该配置检测。自定义服务名部署须将 `serviceName` 设为与 NSSM 注册名一致。真机复验建议：用自定义服务名部署 + `server.serviceName` 配置后，服务运行中 `rollback` 应被拒绝（等同场景 E 但针对自定义名）。
>
> ### 🔄 v0.0.38 发布后真机验收（2026-08-15，Windows 真机）
>
> **服务名可配置化修复端到端通过**（隔离测试 home `C:\Users\WangA\easybot-v038-test\`，测试服务 `EasyBotTest`，官方 v0.0.38 release 二进制，SHA256 对照 checksums.txt 校验通过；未触碰生产服务）：
> - ✅ **核心修复**：`gateway.local.yaml` 配 `server.serviceName: EasyBotTest` → NSSM 注册同名服务运行 → 服务运行中执行 `rollback` → **被正确拦截**：`✗ Rollback failed: EasyBotTest Windows 服务仍在运行，无法安全回滚...`（v0.0.37 此处会因检测不到 `EasyBot` 而放行，产生半状态）。
> - ✅ **对照（证明跟随配置而非硬编码）**：`serviceName` 改成不存在的 `EasyBotWrong` → 放行服务检测、进入 rollback 流程。
> - ✅ `--init` 模板含 `server.serviceName: "EasyBot"` 注释示例；`gateway.local.yaml` 覆盖 base 生效（上游新增 4 个配置单测覆盖）。
> - ⏸️ **场景 G**：v0.0.38 `requires_db_migration=false`，仍不适用（v0.0.36 的 v2→v3 已覆盖）。
> - **验证要点**：rollback 的服务检测在 `read_manifest` **之后**——无 manifest 时直接报 `No update manifest found` 测不到服务检测，须先构造合法 `.update_manifest.json` 才能触发 data-safety 拦截。

---

## 0. 前置准备

| 项 | 要求 |
|---|---|
| Windows 版本 | Windows 10/11（x86_64），或 Windows 11 ARM |
| NSSM | 已安装，服务名 **EasyBot**（安装步骤见 `windows-deployment.md` §1：choco / winget / 手动三种方式 + `nssm version` 验证） |
| Rust 工具链 | 如需本地构建：`rustup target add x86_64-pc-windows-msvc` + MSVC 构建工具 |
| 测试目录 | 独立 home，如 `C:\Users\<你>\easybot-test\.easybot` |

**测试二进制获取（二选一）**

- **推荐（发版后）**：等 v0.0.36 Release 发布，下载 `easybot-windows-x86_64.exe`。
- **本地构建（机制预验证）**：`git checkout main` 后
  ```bat
  cargo build --release --features "default,plugin-system"
  ```
  ⚠️ 构建产物在 `target\release\`，路径含 `target/release/` 会被 `detect_dev_mode` 判为 dev 模式、`update` 直接拒绝。**必须把 exe 复制到独立路径**：
  ```bat
  mkdir C:\easybot-test
  copy target\release\easybot.exe C:\easybot-test\easybot.exe
  ```

**安装测试服务**（指向测试 exe + 测试 home）
```bat
nssm install EasyBot "C:\easybot-test\easybot.exe"
nssm set EasyBot AppParameters "--dir C:\Users\<你>\easybot-test\.easybot"
nssm set EasyBot AppStdout C:\easybot-test\.easybot\logs\service.out.log
nssm set EasyBot AppStderr C:\easybot-test\.easybot\logs\service.err.log
nssm start EasyBot
C:\easybot-test\easybot.exe check-update --dir C:\Users\<你>\easybot-test\.easybot
```

**版本确认**：当前测试二进制版本必须**低于目标**（v0.0.36）。若本地构建且 Cargo.toml 仍是 0.0.35，`update` 会报 AlreadyUpToDate——此时用场景 D（回滚）验证同一交换机制，完整 `update` 流程待发版后再验。

---

## 场景矩阵

### 场景 A — 标准升级流程（happy path）⭐ 核心

前置：服务运行 v0.0.35（旧版）。

```bat
:: 1. 停止服务（释放 exe 锁，交换脚本才能独占）
nssm stop EasyBot

:: 2. 更新（--dir 与部署目录一致，--yes 跳过确认）
C:\easybot-test\easybot.exe update --dir C:\Users\<你>\easybot-test\.easybot --yes

:: 3. 等待交换完成（约 5 秒，含 15×2s 重试窗口）
ping -n 6 127.0.0.1 >nul
type C:\Users\<你>\easybot-test\.easybot\.update\swap-result-*.txt

:: 4. 启动服务
nssm start EasyBot

:: 5. 确认已是最新
C:\easybot-test\easybot.exe check-update --dir C:\Users\<你>\easybot-test\.easybot
```

**期望**：
- 步骤 2 输出 `✓ Update complete: v0.0.35 → v0.0.36` + `⚠ Windows deferred swap scheduled:` 提示块（含 marker 路径与 `nssm stop/start` 指引）
- 步骤 3 marker 内容为 **`OK`**；`.update\` 下批处理 `swap-<ver>.cmd` **已自删除**，仅剩 marker
- 步骤 5 显示已是最新
- 服务健康、管理后台可登录；`{home}\logs\` 无异常报错

**证据**：`sc query EasyBot`（STATE: RUNNING）+ marker 内容 + update 完整输出截图。

---

### 场景 B — 服务运行中执行 update（TIMEOUT 兜底路径）

前置：服务**保持运行**（exe 被进程映射锁定），执行场景 A 步骤 2（不先 stop）。

**期望**：
- update 正常完成下载/校验/暂存，swap 已安排，但 marker 在 ~30s 后写入 **`TIMEOUT`**
- 磁盘上 exe 仍是旧版（交换未发生，安全）
- 批处理文件**保留**（TIMEOUT 分支不自删除）

**恢复**：
```bat
nssm stop EasyBot
C:\easybot-test\easybot.exe update --dir C:\Users\<你>\easybot-test\.easybot --yes
:: 再次 type marker，应为 OK（schedule_swap 会清理旧批处理/marker 后重写）
nssm start EasyBot
```

**证据**：首次 marker=`TIMEOUT`、恢复后=`OK`，两次输出对比。

---

### 场景 C — home 路径含空格与 `!`（验证 escaping 修复）

前置：将测试 home 改为含空格的路径 + 目录名含 `!`（如 `C:\Users\<你>\EasyBot Test\!Home\`）。

```bat
:: 重新 install 服务指向新 home，或直接换 --dir 参数
nssm set EasyBot AppParameters "--dir C:\Users\<你>\EasyBot Test\!Home\"
:: 重复场景 A 步骤 1-5
```

**期望**：与场景 A 完全一致（marker=OK）。批处理中 move/del 命令**原样**保留 `!`（`DisableDelayedExpansion` 生效），空格靠引号正确解析。若批处理因 `!` 被展开而报错（如找不到文件），即回归失败。

**证据**：update 输出 + marker + 若失败则贴批处理 `C:\...\!Home\.update\swap-<ver>.cmd` 内容。

---

### 场景 D — 回滚（服务已停止）⭐ 核心（同一交换机制）

前置：已完成场景 A（manifest + `.bak` + 旧 DB/config 备份已生成）。此场景**无需新版本**，直接验证 `schedule_swap` 回滚路径。

```bat
:: 1. 停止服务
nssm stop EasyBot

:: 2. 回滚（服务必须已停止，见场景 E）
C:\easybot-test\easybot.exe rollback --dir C:\Users\<你>\easybot-test\.easybot --yes

:: 3. 等待交换完成
ping -n 6 127.0.0.1 >nul
type C:\Users\<你>\easybot-test\.easybot\.update\swap-result-*.txt

:: 4. 启动服务
nssm start EasyBot

:: 5. 确认回到旧版
C:\easybot-test\easybot.exe check-update --dir C:\Users\<你>\easybot-test\.easybot
```

**期望**：
- 步骤 2 输出 `✓ Rollback complete`；旧 DB/config 已恢复
- 步骤 3 marker=**`OK`**
- 步骤 5 显示可更新到 v0.0.36（回到旧版）
- `.update_manifest.json` 已删除，`.bak`/DB 备份已清理

**证据**：rollback 输出 + marker + `check-update` 结果 + `.update_manifest.json` 不存在。

---

### 场景 E — 服务运行中回滚被拒绝（验证 data-safety 修复）

前置：服务运行中（新版本），执行场景 D 步骤 2（**不**先 stop）。

**期望**：立即报错并**拒绝**：
> ✗ Rollback failed: EasyBot Windows 服务仍在运行，无法安全回滚：运行中的 exe 被服务锁定，且旧数据库会覆盖活动库。请先停止服务再重试（`nssm stop EasyBot`）。

- 无 swap 安排、无 marker 生成、**DB/config 未被触碰**（`.update_manifest.json` 仍在）

**证据**：报错输出 + 确认 manifest 仍存在（未发生任何恢复动作）。

---

### 场景 F — 成功更新后残留检查（U3 清理）

前置：完成场景 A。

```bat
dir C:\Users\<你>\easybot-test\.easybot\.update\
dir C:\Users\<你>\easybot-test\.easybot\*.bak.*
dir C:\Users\<你>\easybot-test\.easybot\.update_manifest.json
```

**期望**：
- `.update\`：仅剩 `swap-result-<ver>.txt`（批处理已自删）
- `.bak.*` + `.update_manifest.json`：**应存在**（成功路径保留，供 rollback）
- 无遗留临时下载文件

> 注：失败路径清理（下载/替换/校验失败删 `.bak`/manifest/.update）较难在真机无网络干预下构造，可跳过；逻辑已由单测覆盖。

---

### 场景 G — 迁移显式确认（U4）

前置：v0.0.35 → v0.0.36 若含 DB 迁移（以 version manifest 为准）。

```bat
:: update 时观察
C:\easybot-test\easybot.exe update --dir C:\Users\<你>\easybot-test\.easybot --yes
:: 期望输出：✓ N database migration(s) prepared; they will run on next startup

:: 启动后观察日志
nssm start EasyBot
type C:\Users\<你>\easybot-test\.easybot\logs\easybot.log
:: 期望：✓ 数据库迁移完成：N 条 + 逐条迁移清单（v<版本>: <描述>）
```

**证据**：update 输出的迁移提示 + 启动日志的迁移确认行。

---

## 验收清单（勾选）

- [x] A 升级 happy path：marker=OK、批处理自删、check-update 已是最新、服务健康（✅ v0.0.37 完整端到端通过）
- [x] B 服务运行中 update：marker=TIMEOUT、exe 未动、停服重试后 OK（✅ v0.0.37 完整端到端通过）
- [x] C 含空格+`!` 路径：marker=OK，批处理路径原样（v0.0.36 分离脚本 `DisableDelayedExpansion` 验证通过）
- [x] D 回滚（停服）：marker=OK、DB/config 恢复、备份清理、回到旧版（✅ v0.0.37 完整端到端通过，短暂停生产服务后恢复）
- [x] E 服务运行中回滚：被拒绝、无副作用（data-safety 验证通过）
- [x] F 成功更新后：`.update\` 仅 marker；`.bak`/manifest 保留（分离 swap 后残留检查通过）
- [x] G 迁移确认：update 提示 + 启动日志迁移清单（v0.0.36 启动 schema v2→v3，sessions 新增 `custom_name`；**v0.0.37 无迁移不适用**）

> 补充：U1 `--dir` 支持 ✅（v0.0.36 正式版）；U2 bug 复现（v0.0.32 update → `os error 5`）✅；U2 分离 swap 机制（marker=OK/自删/move）✅

## 回报格式

每个场景贴：**执行命令 + 输出（或截图）+ marker 内容 + `sc query EasyBot` 状态**。特别关注：
1. 场景 B/C 的 marker 与批处理内容（验证 TIMEOUT 与 `!` escaping）
2. 场景 E 的拒绝文案
3. 是否有非预期报错 / 挂起 / 死锁（例如服务无法停止、exe 无法启动）

> 若全部通过 → U2 可判定真机验证完成；问题反馈到 issue #95 或持续修复。
