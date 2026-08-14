//! 二进制安全替换与回滚
//!
//! 提供跨平台的二进制文件安全替换操作：
//! - 备份当前二进制（同文件系统复制）
//! - 原子替换（Unix: `rename()` 原地覆盖运行中二进制）
//! - Windows: 运行中的 exe 被进程映射锁定，无法原地覆盖。
//!   采用「暂存 → 分离辅助脚本两步替换」：下载/校验后的新 exe 暂存为独立文件，
//!   主进程退出释放锁后，由分离 spawn 的 `.cmd` 批处理把新 exe 移动到目标路径。
//! - 回滚到备份版本（Windows 走同一分离脚本机制）
//! - Unix 可执行权限设置

use super::types::UpdateError;
use std::path::{Path, PathBuf};

/// 备份文件信息
pub struct BinaryBackup {
    pub backup_path: PathBuf,
}

/// 二进制替换结果
///
/// - Unix：替换已在 `replace_binary` 内完成（原地 rename），`swap = None`。
/// - Windows：新 exe 已暂存为独立文件，`swap = Some`，需调用 `schedule_swap`
///   在主进程退出后完成交换；`verify_path` 指向暂存文件，供提交前校验。
pub struct ReplaceResult {
    pub backup: BinaryBackup,
    /// 用于 `verify_binary` 校验的路径（Unix = 替换后的 target；Windows = 暂存文件）
    pub verify_path: PathBuf,
    /// Windows 上待分离脚本完成的延迟交换
    pub swap: Option<PendingSwap>,
}

/// Windows 待执行的延迟二进制交换
///
/// 类型在 Unix 构建中亦存在（保持 `ReplaceResult` 形态统一），但仅 Windows 构造。
#[allow(dead_code)]
pub struct PendingSwap {
    /// 新 exe 暂存文件（`.exe` 后缀，保证 `CreateProcess` 可直接运行校验）
    pub staged: PathBuf,
    /// 最终 exe 路径（当前进程的 exe）
    pub target: PathBuf,
}

/// 安全替换当前运行中的二进制文件
///
/// 流程：
/// 1. 备份当前二进制到 `{exe}.bak.{version}`（保留，供 `easybot rollback`）
/// 2. 将新二进制移入目标目录并更名（暂存）
/// 3. 提交：
///    - **Unix**：`rename(staged, target)` 原子原地替换——运行中进程保留旧 inode，
///      新启动即用新文件（POSIX 语义，无需额外步骤）。
///    - **Windows**：运行中的 exe 被进程映射锁定，`rename` 会得到 os error 5。
///      因此**不覆盖 target**，返回 `swap: Some(PendingSwap{..})`，由主进程退出后
///      的分离辅助脚本完成 `move staged → target`（见 [`schedule_swap`]）。
/// 4. 设置可执行权限（Unix）
/// 5. 返回 `ReplaceResult`（含备份对象 + 待校验路径 + Windows 延迟交换信息）
///
/// 如果替换过程中任何步骤失败，自动尝试回滚。
pub fn replace_binary(new_bin: &Path, current_version: &str) -> Result<ReplaceResult, UpdateError> {
    let target = std::env::current_exe()
        .map_err(|e| UpdateError::BinaryReplaceFailed(format!("Cannot get current exe: {}", e)))?;

    // 1. 备份当前二进制（保留，供后续回滚）
    let backup_path = create_backup(&target, current_version)?;

    // 2. 暂存新二进制（同文件系统确保 rename 原子性）
    let staged = stage_path(&target, current_version);
    std::fs::rename(new_bin, &staged).map_err(|e| {
        // 暂存失败：清理已建的备份
        let _ = std::fs::remove_file(&backup_path);
        UpdateError::BinaryReplaceFailed(format!("Cannot stage new binary: {}", e))
    })?;

    // 3. 提交（平台自适应）
    #[cfg(windows)]
    {
        // Windows：目标 exe 被当前进程锁定，无法原地替换。
        // 校验在暂存文件上执行（verify_binary 会启动它），通过后由
        // schedule_swap 在主进程退出后完成交换。target 此刻保持旧版本。
        tracing::info!(
            "Windows: binary staged for deferred swap: {} -> {}",
            staged.display(),
            target.display()
        );
        Ok(ReplaceResult {
            backup: BinaryBackup { backup_path },
            verify_path: staged.clone(),
            swap: Some(PendingSwap { staged, target }),
        })
    }
    #[cfg(not(windows))]
    {
        // Unix：原子原地替换（运行中进程保留旧 inode，安全）
        match std::fs::rename(&staged, &target) {
            Ok(_) => {
                set_executable(&target)?;
                tracing::info!(
                    "Binary replaced: {} -> {}",
                    target.display(),
                    current_version
                );
                Ok(ReplaceResult {
                    backup: BinaryBackup { backup_path },
                    verify_path: target.clone(),
                    swap: None,
                })
            }
            Err(e) => {
                // 替换失败，回滚
                tracing::error!("Binary replace failed: {}, attempting rollback", e);
                let _ = std::fs::rename(&backup_path, &target);
                let _ = std::fs::remove_file(&staged);
                Err(UpdateError::BinaryReplaceFailed(format!(
                    "Cannot replace binary: {}. Rolled back to original.",
                    e
                )))
            }
        }
    }
}

/// 计算新二进制在目标目录中的暂存文件名
///
/// - Unix：`{stem}.tmp.new`（目标目录内，rename 原子）
/// - Windows：`{stem}.{version}.exe`（保留 `.exe` 后缀，保证 `CreateProcess`
///   可直接运行新 exe 做校验；目标文件被锁但暂存文件未锁，可正常启动）
fn stage_path(target: &Path, version: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let stem = target
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("easybot");
        target.with_file_name(format!("{}.{}.exe", stem, version))
    }
    #[cfg(not(windows))]
    {
        let _ = version;
        target.with_extension("tmp.new")
    }
}

/// Windows：安排分离辅助脚本在进程退出后完成延迟二进制交换
///
/// 生成 `{home}/.update/swap-{version}.cmd`（`DisableDelayedExpansion` + label 结构的
/// 有界重试，避免路径中的 `!` 被展开），以 `CREATE_NO_WINDOW` 分离 spawn `cmd /c`。
/// 父进程（正在被替换的 exe）退出释放文件锁后，批处理完成 `move /y "{staged}" "{target}"`。
///
/// 前置约束：目标 exe 在批处理执行时须未被占用（如 NSSM 服务已停止），否则 15 次
/// 重试（约 30s）后写 `TIMEOUT` 到 marker 并退出——目标保持旧版本，可安全重试。
///
/// marker 写入 `{home}/.update/swap-result-{version}.txt`：`OK` 表示交换成功，
/// `TIMEOUT` 表示目标仍被占用。
#[cfg(windows)]
pub fn schedule_swap(
    home: &Path,
    swap: &PendingSwap,
    version: &str,
    cleanup: &[PathBuf],
) -> Result<PathBuf, UpdateError> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    let update_dir = home.join(".update");
    std::fs::create_dir_all(&update_dir).map_err(|e| {
        UpdateError::BinaryReplaceFailed(format!("Cannot create {}: {}", update_dir.display(), e))
    })?;

    // 清理旧版本/旧尝试遗留的批处理与 marker，避免 `.update/` 跨版本累积。
    // 已完成的 marker/批处理不再需要；残留的（如上一轮 TIMEOUT）也会被下面的写入覆盖。
    if let Ok(entries) = std::fs::read_dir(&update_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if (name.starts_with("swap-") && name.ends_with(".cmd"))
                || (name.starts_with("swap-result-") && name.ends_with(".txt"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let batch = update_dir.join(format!("swap-{}.cmd", version));
    let marker = update_dir.join(format!("swap-result-{}.txt", version));

    // 覆盖式写入旧的批处理/marker（同一版本重试时复用）
    let script = build_swap_script(&swap.staged, &swap.target, &marker, cleanup);
    std::fs::write(&batch, script).map_err(|e| {
        UpdateError::BinaryReplaceFailed(format!("Cannot write swap script: {}", e))
    })?;
    let _ = std::fs::remove_file(&marker);

    std::process::Command::new("cmd")
        .arg("/c")
        .arg(&batch)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            UpdateError::BinaryReplaceFailed(format!("Cannot spawn swap helper: {}", e))
        })?;

    tracing::warn!(
        "Deferred binary swap scheduled (batch: {}, marker: {})",
        batch.display(),
        marker.display()
    );
    Ok(marker)
}

/// 生成 Windows 分离交换批处理脚本（纯函数，Linux CI 可直接单测）
///
/// 语义：尝试 `move /y staged target`，成功（staged 消失）即写 `OK` 到 marker；
/// 失败（目标被锁）重试最多 15 次（每次约 2s），超时写 `TIMEOUT` 并退出。
/// 成功后删除 `cleanup` 列表中的陈旧文件并自删除。
#[cfg_attr(not(windows), allow(dead_code))] // Unix 库构建下仅测试使用（Windows 由 schedule_swap 调用）
pub fn build_swap_script(
    staged: &Path,
    target: &Path,
    marker: &Path,
    cleanup: &[PathBuf],
) -> String {
    let mut s = String::new();
    s.push_str("@echo off\r\n");
    // 关键：禁用 delayed expansion（`setlocal DisableDelayedExpansion`）。
    // 若启用，路径中的 `!` 会被当作变量引用展开（如用户名含 `!` 的 home 目录），
    // 破坏 move/del 命令。本脚本是 label 结构（无括号块），`%tries%` 在逐行
    // 执行时求值，故用 `%tries%` 即可安全计数，无需 `!tries!`。
    s.push_str("setlocal DisableDelayedExpansion\r\n");
    s.push_str("set tries=0\r\n");
    s.push_str(":retry\r\n");
    // timeout 在 stdin 重定向（null）时失败，改用 ping 做等待
    s.push_str("ping -n 3 127.0.0.1 >nul\r\n");
    s.push_str(&format!(
        "move /y \"{}\" \"{}\" >nul 2>&1\r\n",
        staged.display(),
        target.display()
    ));
    s.push_str(&format!(
        "if not exist \"{}\" goto ok\r\n",
        staged.display()
    ));
    s.push_str("set /a tries+=1\r\n");
    s.push_str("if %tries% lss 15 goto retry\r\n");
    s.push_str(&format!("echo TIMEOUT> \"{}\"\r\n", marker.display()));
    s.push_str("exit /b 1\r\n");
    s.push_str(":ok\r\n");
    s.push_str(&format!("echo OK> \"{}\"\r\n", marker.display()));
    for p in cleanup {
        s.push_str(&format!("del /q \"{}\" 2>nul\r\n", p.display()));
    }
    s.push_str("del \"%~f0\"\r\n");
    s
}

/// 从备份恢复二进制文件
///
/// 仅 Unix 使用：Windows 回滚走分离交换脚本（运行中 exe 无法进程内覆盖），
/// 故 Windows 构建下该函数无调用点，需允许 dead_code（`-D warnings` 门禁）。
#[cfg_attr(windows, allow(dead_code))]
pub fn rollback_binary(backup: &BinaryBackup) -> Result<(), UpdateError> {
    let current_exe = std::env::current_exe()
        .map_err(|e| UpdateError::RollbackFailed(format!("Cannot get current exe: {}", e)))?;

    if !backup.backup_path.exists() {
        return Err(UpdateError::RollbackFailed(format!(
            "Backup not found: {}",
            backup.backup_path.display()
        )));
    }

    std::fs::copy(&backup.backup_path, &current_exe)
        .map_err(|e| UpdateError::RollbackFailed(format!("Cannot restore backup: {}", e)))?;

    #[cfg(unix)]
    set_executable(&current_exe)?;

    tracing::info!("Binary rolled back from {}", backup.backup_path.display());
    Ok(())
}

/// 创建当前二进制的备份
fn create_backup(exe_path: &Path, version: &str) -> Result<PathBuf, UpdateError> {
    let backup_name = format!(
        "{}.bak.{}",
        exe_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("easybot"),
        version
    );
    let backup_path = exe_path.with_file_name(&backup_name);

    std::fs::copy(exe_path, &backup_path)
        .map_err(|e| UpdateError::BackupFailed(format!("Cannot create binary backup: {}", e)))?;

    tracing::info!("Binary backup created: {}", backup_path.display());
    Ok(backup_path)
}

/// 验证新二进制能否正常启动
///
/// 通过运行 `{new_bin} --dir {home} check-update` 检测退出码。
/// `--dir` 透传保证校验时新二进制从正确目录解析配置（否则可能回落到默认目录，
/// 在 `--dir` 部署场景下得出错误结论）。
pub async fn verify_binary(bin_path: &Path, home: &Path) -> Result<(), UpdateError> {
    let bin = bin_path.to_path_buf();
    let home = home.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let output = std::process::Command::new(&bin)
            .arg("--dir")
            .arg(&home)
            .arg("check-update")
            .output()
            .map_err(|e| {
                UpdateError::VerificationFailed(format!("Cannot start new binary: {}", e))
            })?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(UpdateError::VerificationFailed(format!(
                "New binary exited with code {}: {}",
                output.status,
                stderr.trim()
            )))
        }
    })
    .await
    .map_err(|e| UpdateError::VerificationFailed(format!("Join error: {}", e)))?
}

/// 设置 Unix 可执行权限
#[cfg(unix)]
fn set_executable(path: &Path) -> Result<(), UpdateError> {
    use std::os::unix::fs::PermissionsExt;
    let metadata = std::fs::metadata(path)?;
    let mut perm = metadata.permissions();
    let current_mode = perm.mode();

    // 保持原有 owner/group，添加 owner 和 group 的可执行位
    let new_mode = current_mode | 0o111; // 不移除任何权限

    if current_mode != new_mode {
        perm.set_mode(new_mode);
        std::fs::set_permissions(path, perm)?;
    }

    Ok(())
}

#[cfg(not(unix))]
#[allow(dead_code)]
fn set_executable(_path: &Path) -> Result<(), UpdateError> {
    // Windows 没有 Unix 风格的可执行位
    Ok(())
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_backup_and_rollback() {
        let dir = std::env::temp_dir().join(format!("easybot_test_replace_{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        // 创建 "当前二进制"
        let exe_path = dir.join("easybot");
        fs::write(&exe_path, b"original content").unwrap();

        // 模拟 current_exe() 返回
        // 注意：不能实际修改 std::env::current_exe，我们直接测试备份逻辑
        let backup = create_backup(&exe_path, "0.0.16").unwrap();
        assert!(backup.exists());

        // 验证备份内容
        let content = fs::read(&backup).unwrap();
        assert_eq!(content, b"original content");

        // 修改原文件
        fs::write(&exe_path, b"new content").unwrap();
        assert_eq!(fs::read(&exe_path).unwrap(), b"new content");

        // 手动恢复
        fs::copy(&backup, &exe_path).unwrap();
        assert_eq!(fs::read(&exe_path).unwrap(), b"original content");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_verify_binary_nonexistent() {
        // 带 home 参数的透传：仍应启动失败（二进制不存在）
        let result = verify_binary(Path::new("/nonexistent/easybot"), Path::new("/tmp/home")).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_stage_path_unix_keeps_tmp_new() {
        let target = Path::new("/opt/easybot/easybot");
        let staged = stage_path(target, "0.0.36");
        // 非 Windows：暂存为 `{stem}.tmp.new`（rename 原子所需）
        assert_eq!(staged, Path::new("/opt/easybot/easybot.tmp.new"));
    }

    #[test]
    fn test_stage_path_windows_keeps_exe_suffix() {
        // 跨平台断言：Windows 分支用 `.exe` 后缀（保证 CreateProcess 可运行校验）。
        // 该测试在 Linux 上走 Unix 分支，这里仅验证 `.exe` 后缀路径不被误用，
        // Windows 专属分支由 CI 的 windows target 覆盖。
        let staged = stage_path(Path::new("C:\\easybot\\easybot.exe"), "0.0.36");
        #[cfg(windows)]
        assert_eq!(staged, PathBuf::from("C:\\easybot\\easybot.0.0.36.exe"));
        #[cfg(not(windows))]
        assert_eq!(staged, PathBuf::from("C:\\easybot\\easybot.tmp.new"));
    }

    #[test]
    fn test_build_swap_script_contains_move_and_retry() {
        let staged = Path::new("C:\\home\\data\\.update\\easybot.0.0.36.exe");
        let target = Path::new("C:\\home\\easybot.exe");
        let marker = Path::new("C:\\home\\data\\.update\\swap-result-0.0.36.txt");
        let cleanup = vec![PathBuf::from("C:\\home\\gateway.yaml.bak.0.0.35")];

        let script = build_swap_script(staged, target, marker, &cleanup);

        // 核心命令都在；delayed expansion 必须禁用，避免路径中的 `!` 被展开
        assert!(script.contains("setlocal DisableDelayedExpansion"));
        assert!(!script.contains("enabledelayedexpansion"));
        assert!(script.contains(
            r#"move /y "C:\home\data\.update\easybot.0.0.36.exe" "C:\home\easybot.exe" >nul 2>&1"#
        ));
        assert!(
            script.contains(r#"if not exist "C:\home\data\.update\easybot.0.0.36.exe" goto ok"#)
        );
        // 有界重试：最多 15 次（label 结构下 `%tries%` 逐行求值，无需 `!tries!`）
        assert!(script.contains("if %tries% lss 15 goto retry"));
        assert!(!script.contains("!tries!"));
        // 成功写 OK、超时写 TIMEOUT 到 marker
        assert!(script.contains(r#"echo OK> "C:\home\data\.update\swap-result-0.0.36.txt""#));
        assert!(script.contains(r#"echo TIMEOUT> "C:\home\data\.update\swap-result-0.0.36.txt""#));
        // 成功后清理陈旧文件 + 自删除
        assert!(script.contains(r#"del /q "C:\home\gateway.yaml.bak.0.0.35" 2>nul"#));
        assert!(script.contains("del \"%~f0\""));
    }

    #[test]
    fn test_build_swap_script_exclamation_path_stays_literal() {
        // 路径含 `!`（如用户名含感叹号的 home 目录）时，因 delayed expansion 被禁用，
        // 生成的批处理必须原样保留 `!`，不得被当作变量引用展开。
        let staged = Path::new("C:\\Users\\Wang!A\\.update\\easybot.0.0.36.exe");
        let target = Path::new("C:\\Users\\Wang!A\\easybot.exe");
        let marker = Path::new("C:\\Users\\Wang!A\\.update\\swap-result-0.0.36.txt");

        let script = build_swap_script(staged, target, marker, &[]);

        assert!(
            script.contains(r#"move /y "C:\Users\Wang!A\.update\easybot.0.0.36.exe" "C:\Users\Wang!A\easybot.exe" >nul 2>&1"#),
            "含 ! 的路径必须原样出现在 move 命令中"
        );
        assert!(
            script.contains(r#"echo OK> "C:\Users\Wang!A\.update\swap-result-0.0.36.txt""#),
            "含 ! 的 marker 路径必须原样出现"
        );
    }

    #[test]
    fn test_create_backup_nonexistent_source() {
        let result = create_backup(Path::new("/nonexistent/binary"), "0.0.16");
        assert!(result.is_err());
    }
}
