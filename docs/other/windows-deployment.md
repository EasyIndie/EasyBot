# EasyBot Windows 部署指南

> 适用版本：v0.0.38+（此前版本请先升级）。验证环境：Windows 10 (10.0.26200) + PowerShell 5.1 / pwsh 7 + NSSM 2.24。

本指南描述在 Windows 上将 EasyBot 安装为后台服务的完整路径：下载校验 → 初始化配置 → 配置凭据与适配器 → 用 NSSM 注册服务 → 验证。

EasyBot 是普通控制台程序，**不是**原生 Windows 服务程序（无 `ServiceMain`/SCM 入口），因此用 `sc.exe` 直接注册会超时报 1053。正确做法是用 **NSSM** 包装，让服务以 `easybot --dir <home>` 启动。服务启动后 EasyBot 会自行从 `<home>/.env` 加载凭据、从 `<home>/gateway.local.yaml` 合并本地配置，数据与日志都落在 `<home>` 下。

## 1. 前置条件

1. 下载对应架构的二进制（GitHub Releases 页）：
   - `easybot-x86_64-pc-windows-msvc.exe`（Intel/AMD）
   - `easybot-aarch64-pc-windows-msvc.exe`（ARM64）
2. 校验 SHA256（示例，实际以对应版本 Release 附带的 `checksums.txt` 为准）：
   ```powershell
   Get-FileHash .\easybot-x86_64-pc-windows-msvc.exe -Algorithm SHA256
   ```
   与 `checksums.txt` 中记录值比对一致后再继续。
3. 安装 NSSM（服务包装器）。EasyBot 是普通控制台程序、非原生 Windows 服务，NSSM 负责把它注册为后台服务并在崩溃时自动重启。`manage-service.ps1` 会自动检测 NSSM（查找 PATH / `ProgramFiles\NSSM` / Chocolatey 目录），未安装会明确提示，因此**部署前必须装好 NSSM**。

   任选一种安装方式：
   ```powershell
   # 方式一：Chocolatey（需先安装 choco 包管理器）
   choco install nssm

   # 方式二：winget
   winget install nssm.nssm

   # 方式三：手动（不依赖包管理器）
   #   1. 从 https://nssm.cc/download 下载最新版 zip
   #   2. 解压后把 64 位 nssm.exe 放到固定目录，如 C:\tools\NSSM\
   #   3. 将 C:\tools\NSSM\ 加入系统 PATH，或后续改用绝对路径调用
   ```

   验证安装（终端能找到命令即成功）：
   ```powershell
   nssm version
   ```
4. 将 `easybot.exe` 加入 PATH 或放在将要初始化的配置目录中（`manage-service.ps1` 会自动检测）。

## 2. 初始化配置目录

```powershell
# 指定配置目录（强烈建议显式 --dir，不要依赖默认的 %APPDATA%\easybot\）
easybot.exe --init --dir C:/Users/<你>/​.easybot
```

`--init` 会生成 `gateway.yaml`、`.env`、`gateway.local.yaml` 以及 `manage-service.ps1`。

> **关键：服务的启动参数必须用 `--dir <home>`，不要用 `--config`。**
> `--config` 只改变 gateway.yaml 的读取位置，`.env` / `gateway.local.yaml` / 数据 / 日志始终从 `--dir`（或 `EASYBOT_HOME`）解析。用 `--config` 注册服务会导致这些文件从错误目录加载。

## 3. 配置凭据（.env）

编辑 `<home>/.env`，取消注释并填入令牌：

```bash
TELEGRAM_BOT_TOKEN=123456:ABC-xxx
DISCORD_BOT_TOKEN=your_token
FEISHU_APP_ID=cli_xxx
FEISHU_APP_SECRET=xxx
QQ_APP_ID=your_qq_app_id
QQ_CLIENT_SECRET=your_qq_client_secret

# 管理后台登录密码（生产环境至少 12 个字符；未设置时管理后台登录被拒绝）
EASYBOT_ADMIN_PASSWORD=replace-with-a-long-random-password
```

设置令牌后对应适配器自动启用，无需在 gateway.yaml 中声明。

## 4. 控制适配器（gateway.local.yaml）

不需要的适配器（如个人微信，需扫码登录不适合服务环境）在此显式禁用：

```yaml
adapters:
  wechat:
    enabled: false
```

`--init` 生成的模板已列出全部适配器的注释示例，取消注释即可。

## 5. 安装为 Windows 服务

用**管理员身份**打开 PowerShell：

```powershell
cd <home>
PowerShell -ExecutionPolicy Bypass -File .\manage-service.ps1 install
```

脚本会：检测 NSSM 与 easybot.exe → 用 NSSM 注册服务（`AppParameters=--dir <home>`、崩溃自动重启、stdout/stderr 重定向到 `logs/easybot.out.log` / `.err.log`、开机自启）→ 启动服务。

## 6. 验证

```powershell
# 服务状态
Get-Service EasyBot

# 健康检查
curl.exe http://localhost:8080/api/v1/health
# {"status":"healthy", ...}

# 管理后台登录验证（返回 session key 即成功）
curl.exe -X POST http://localhost:8080/admin/login `
  -H "Content-Type: application/json" `
  -d '{"password":"你的密码"}'
```

## 7. 服务管理命令

```powershell
.\manage-service.ps1 status    # 查看状态
.\manage-service.ps1 logs      # 实时日志（tail logs/easybot.out.log）
.\manage-service.ps1 restart   # 重启
.\manage-service.ps1 uninstall # 卸载
.\manage-service.ps1 enable    # 开机自启（默认已开启）
.\manage-service.ps1 disable   # 取消开机自启
```

> **NSSM 命令与脚本对照**：`manage-service.ps1` 是 NSSM 的友好封装（自动检测 NSSM 与 exe、参数裸传）。第 9 节升级流程直接使用 NSSM 命令，二者等价：

| 操作 | NSSM 直接命令（服务名 EasyBot） | manage-service.ps1 |
|---|---|---|
| 查看状态 | `nssm status EasyBot` / `sc query EasyBot` | `.\manage-service.ps1 status` |
| 停止 | `nssm stop EasyBot` | `.\manage-service.ps1 stop` |
| 启动 | `nssm start EasyBot` | `.\manage-service.ps1 start` |
| 重启 | `nssm restart EasyBot` | `.\manage-service.ps1 restart` |
| 卸载 | `nssm remove EasyBot confirm` | `.\manage-service.ps1 uninstall` |

## 8. 常见问题对照

| # | 现象 | 根因 | 解决 |
|:--|:-----|:-----|:-----|
| 1 | `manage-service.ps1` 报 "Cannot overwrite variable HOME" | `$Home` 是 PowerShell 只读自动变量 | 已改名为 `$HomeDir`（v0.0.32 修复） |
| 2 | `Start-Service` 报 1053 启动超时 | EasyBot 非原生服务程序，sc.exe 直连 SCM 会超时 | 用 NSSM 包装（本脚本默认方式） |
| 3 | 服务启动即停，事件日志"找不到路径"，退出码 1066 | NSSM 的 Application/AppParameters 被加了引号，引号被当作路径一部分 | 参数裸传（本脚本已内置） |
| 4 | gateway.local.yaml 的适配器禁用失效 / .env 不加载 | 用 `--config` 注册，.env 与 local 配置从错误目录解析 | 服务用 `--dir <home>`（本脚本已内置） |
| 5 | `setx` 设置密码后服务仍报"密码未设置" | `setx` 写用户级环境变量，服务以 LocalSystem 运行读不到 | 密码写入 `<home>/.env`，EasyBot 启动时自行加载 |
| 6 | gateway.yaml 配置 adminPassword 仍提示"未配置密码" | 多为上述 #4 导致服务跑错目录；`server.adminPassword` 为 camelCase 键名 | 确认服务用 `--dir <home>`；键名写 `adminPassword` |
| 7 | Defender 隔离删除 exe | 新 Rust 二进制可能被 ML 误判 | 见「10. Windows Defender 误报处理」 |
| 8 | `update` 报拒绝访问 / exe 没被替换 | 正在运行的服务锁定 exe（os error 5），原地覆盖失败 | 先 `manage-service.ps1 stop` 再 `update`（新版走两步替换，见「9. 升级」） |
| 9 | 更新失败后残留 `.bak`/`.update_manifest.json` | 旧版失败/回滚路径不清理 | 新版失败/回滚自动清理；成功路径保留备份供 `rollback` |
| 10 | `update --dir <home>` 报参数错误 | 旧版 `--dir` 不是全局参数，置于子命令后无法解析 | 新版 `--dir` 为全局参数，`update/check-update/rollback` 均支持且显示在各子命令 `--help` |
| 11 | `manage-service.ps1` 报"未找到 NSSM" | NSSM 未安装或不在 PATH | 按「1. 前置条件」第 3 步安装 NSSM（choco / winget / 手动三选一），并确认 `nssm version` 可运行 |
| 12 | 自定义服务名部署（如 `EasyBotTest`）时 `rollback` 不拒绝运行中的服务，产生"DB 已回滚但 exe 未变"半状态 | updater 的 data-safety 保护按 `server.serviceName` 检测服务是否运行中；默认 `EasyBot`，自定义名未配置则检测不到 | 在 `<home>/gateway.yaml` 配置 `server.serviceName: "<你的服务名>"` 与 NSSM 注册名一致（或用 `manage-service.ps1` 标准名 `EasyBot`） |

## 9. 升级

Windows 上**正在运行的 exe 被进程锁定，无法原地覆盖**。`easybot update` 采用「暂存 → 分离辅助脚本两步替换」：

1. 新 exe 下载并校验后暂存为独立文件，**先运行校验**（`check-update`）通过后，才安排交换。
2. 二进制交换在本进程**退出后**由分离的 `.cmd` 批处理完成（写结果到 `{home}/.update/swap-result-<ver>.txt`）。
3. **交换要求 exe 未被占用**——请先停止服务再启动，让批处理独占 exe。

完整流程：

```powershell
# 1. 停止服务（释放 exe 锁）
.\manage-service.ps1 stop

# 2. 更新（--dir 与部署目录一致；--yes 跳过确认）
easybot.exe update --dir <home> --yes

# 3. 等待交换完成：marker 出现 OK（约几秒；失败会写 TIMEOUT）
Get-Content <home>/.update/swap-result-*.txt

# 4. 启动服务
.\manage-service.ps1 start

# 5. 确认已是最新
easybot.exe check-update --dir <home>
```

> **为什么不能原地替换**：Windows 映射正在运行的可执行文件，`rename` 会得到 os error 5（拒绝访问）。Unix（systemd/launchd）的 `rename` 允许覆盖运行中二进制，因此 Linux/macOS 仍是「运行 `update` → 重启服务」两步。Docker 部署请用 `docker compose pull && docker compose up -d`。

> **回滚**：`easybot rollback --dir <home> --yes` 同样走延迟交换（先停服务再执行）。成功后旧 exe 的 `.bak` 由交换脚本一并清理。注意：服务运行中执行 `rollback` 会被拒绝（防止旧 DB 覆盖活动库），请先停服再回滚。
>
> **服务名检测**：updater 的 data-safety 保护按 `server.serviceName` 配置（`gateway.yaml`，默认 `EasyBot`）检测服务是否运行中。用 `manage-service.ps1` 标准安装（服务名 `EasyBot`）无需配置；**自定义 NSSM 服务名部署必须把 `server.serviceName` 设为与 NSSM 注册名一致**，否则运行中的服务检测不到，回滚保护失效（详见 FAQ #12）。

> **真机验证结论**：分离辅助脚本两步替换（U2）机制已通过 Windows 真机 + NSSM 验收（2026-08-14：U1 `--dir`、分离交换、含 TIMEOUT 兜底、`!`/空格路径、回滚与迁移确认场景 C/E/F、A·B 前置路径；场景 A/B/D/G 完整端到端在 v0.0.37 发版后补全回归）。

## 10. Windows Defender 误报处理

新编译的 Rust 二进制可能被 Defender 的机器学习误判为恶意软件，导致隔离删除 exe。

```powershell
# 1. 校验下载的 exe 未被篡改（与 Release 附带的 checksums.txt 比对）
Get-FileHash .\easybot-x86_64-pc-windows-msvc.exe -Algorithm SHA256

# 2. 对 exe 所在目录加排除项（防后续更新误删）
Add-MpPreference -ExclusionPath "<home>"
Add-MpPreference -ExclusionPath "C:\Program Files\EasyBot"

# 3. 若已被隔离：在「Windows 安全中心 → 病毒和威胁防护 → 保护历史记录」中
#    选择「还原」，还原后复核哈希一致再启动
```

> Defender 隔离的是可执行文件本体，不影响 `.env` 等配置；还原后务必复核 SHA256 与官方 checksums.txt 一致再继续。
