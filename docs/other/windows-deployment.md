# EasyBot Windows 部署指南

> 适用版本：v0.0.32+（此前版本请先升级）。验证环境：Windows 10 (10.0.26200) + PowerShell 5.1 / pwsh 7 + NSSM 2.24。

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
3. 安装 NSSM（服务包装器）：
   ```powershell
   choco install nssm
   # 或从 https://nssm.cc/download 下载后放入 PATH
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

## 8. 常见问题对照

| # | 现象 | 根因 | 解决 |
|:--|:-----|:-----|:-----|
| 1 | `manage-service.ps1` 报 "Cannot overwrite variable HOME" | `$Home` 是 PowerShell 只读自动变量 | 已改名为 `$HomeDir`（v0.0.32 修复） |
| 2 | `Start-Service` 报 1053 启动超时 | EasyBot 非原生服务程序，sc.exe 直连 SCM 会超时 | 用 NSSM 包装（本脚本默认方式） |
| 3 | 服务启动即停，事件日志"找不到路径"，退出码 1066 | NSSM 的 Application/AppParameters 被加了引号，引号被当作路径一部分 | 参数裸传（本脚本已内置） |
| 4 | gateway.local.yaml 的适配器禁用失效 / .env 不加载 | 用 `--config` 注册，.env 与 local 配置从错误目录解析 | 服务用 `--dir <home>`（本脚本已内置） |
| 5 | `setx` 设置密码后服务仍报"密码未设置" | `setx` 写用户级环境变量，服务以 LocalSystem 运行读不到 | 密码写入 `<home>/.env`，EasyBot 启动时自行加载 |
| 6 | gateway.yaml 配置 adminPassword 仍提示"未配置密码" | 多为上述 #4 导致服务跑错目录；`server.adminPassword` 为 camelCase 键名 | 确认服务用 `--dir <home>`；键名写 `adminPassword` |
| 7 | Defender 隔离删除 exe | 新 Rust 二进制可能被 ML 误判 | 校验 SHA256 后对 exe 所在目录加白名单 |

## 9. 升级

运行 `easybot.exe update` 即可。NSSM 指向的 exe 路径不变，更新替换二进制后重启服务即生效：

```powershell
.\manage-service.ps1 restart
```
