//! CLI 集成测试
//!
//! 验证 easybot 二进制的基本 CLI 行为。
//! 使用 std::process::Command 直接调用二进制。

use std::path::PathBuf;
use std::process::Command;

/// 获取 easybot 二进制路径
fn easybot_bin() -> PathBuf {
    // CARGO_MANIFEST_DIR 指向 tests/integration/
    // 需要上三层到 workspace root: tests/integration → tests → . → target
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent()   // tests/
        .unwrap()
        .parent()   // workspace root
        .unwrap();
    workspace_root.join("target").join("debug").join("easybot")
}

#[test]
fn test_cli_version() {
    let output = Command::new(easybot_bin())
        .arg("--version")
        .output()
        .expect("failed to run easybot --version");
    assert!(output.status.success(), "--version should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("easybot"), "output should contain 'easybot'");
    assert!(stdout.contains("0.1.0"), "output should contain version");
}

#[test]
fn test_cli_help() {
    let output = Command::new(easybot_bin())
        .arg("--help")
        .output()
        .expect("failed to run easybot --help");
    assert!(output.status.success(), "--help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage:"), "should show usage");
    assert!(stdout.contains("--config"), "should show --config flag");
    assert!(stdout.contains("--init"), "should show --init flag");
    assert!(stdout.contains("--debug"), "should show --debug flag");
    assert!(stdout.contains("--version"), "should show --version flag");
}

#[test]
fn test_cli_init_creates_config() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let dir_path = dir.path().to_str().unwrap();

    let output = Command::new(easybot_bin())
        .arg("--init")
        .arg("--dir")
        .arg(dir_path)
        .output()
        .expect("failed to run easybot --init");

    assert!(output.status.success(), "--init should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("initialized"),
        "first init should print 'initialized', got: {}",
        stdout
    );

    // Verify gateway.yaml was created
    assert!(
        dir.path().join("gateway.yaml").exists(),
        "gateway.yaml should exist after --init"
    );

    // Verify data/ and plugins/ dirs were created
    assert!(dir.path().join("data").exists(), "data/ should exist");
    assert!(dir.path().join("plugins").exists(), "plugins/ should exist");
}

#[test]
fn test_cli_init_idempotent() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let dir_path = dir.path().to_str().unwrap();

    // First run
    let first = Command::new(easybot_bin())
        .arg("--init")
        .arg("--dir")
        .arg(dir_path)
        .output()
        .expect("first --init failed");
    assert!(first.status.success());

    // Second run — should print "already initialized"
    let second = Command::new(easybot_bin())
        .arg("--init")
        .arg("--dir")
        .arg(dir_path)
        .output()
        .expect("second --init failed");
    assert!(second.status.success(), "second --init should exit 0");

    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("already"),
        "second init should say 'already', got: {}",
        stdout
    );

    // gateway.yaml should still exist exactly once
    assert!(
        dir.path().join("gateway.yaml").exists(),
        "gateway.yaml should still exist"
    );
}

#[test]
fn test_cli_unknown_flag() {
    let output = Command::new(easybot_bin())
        .arg("--nonexistent-flag")
        .output()
        .expect("failed to run easybot with unknown flag");
    assert!(!output.status.success(), "unknown flag should exit non-zero");
}

#[test]
fn test_cli_short_flags() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let dir_path = dir.path().to_str().unwrap();

    let output = Command::new(easybot_bin())
        .arg("-d")
        .arg("--dir")
        .arg(dir_path)
        .output()
        .expect("failed to run easybot -d");
    // With --dir pointing to empty dir, it should start with default config
    // and the --debug flag should enable debug logging.
    // It may exit with error or start successfully depending on config.
    // However it should not crash with a usage error.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("error:"),
        "short flag should not cause parse error: {}",
        stderr
    );
}
