//! CLI 集成测试
//!
//! 验证 easybot 二进制的基本 CLI 行为。
//! 使用 std::process::Command 直接调用二进制。

use std::path::PathBuf;
use std::process::Command;
use std::io::Read;

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

    // Server will block, so use spawn + kill pattern
    let mut child = Command::new(easybot_bin())
        .arg("-d")
        .arg("--dir")
        .arg(dir_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start easybot -d");

    // Let it run briefly to see if it crashes
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Check if still alive (if it crashed, we'd see an error)
    match child.try_wait() {
        Ok(Some(status)) => {
            // Process already exited — read stderr to see why
            let mut stderr = String::new();
            child.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
            panic!("easybot exited prematurely with status {}: {}", status, stderr);
        }
        Ok(None) => {
            // Still running — expected
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("failed to check easybot status: {}", e);
        }
    }
}

#[test]
fn test_openapi_has_security_scheme() {
    // Start the server
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let dir_path = dir.path().to_str().unwrap();

    // First init the dir, then start the server
    let init = Command::new(easybot_bin())
        .arg("--init")
        .arg("--dir")
        .arg(dir_path)
        .output()
        .expect("init failed");
    assert!(init.status.success());

    let mut child = Command::new(easybot_bin())
        .arg("--debug")
        .arg("--dir")
        .arg(dir_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to start easybot");

    // Wait for server to start, then fetch openapi.json
    std::thread::sleep(std::time::Duration::from_secs(2));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let resp = ureq::get("http://localhost:8080/openapi.json")
            .call()
            .expect("failed to fetch openapi.json");
        assert_eq!(resp.status(), 200, "openapi.json should return 200");

        let mut body = String::new();
        resp.into_reader().read_to_string(&mut body).unwrap();
        let spec: serde_json::Value =
            serde_json::from_str(&body).expect("openapi.json should be valid JSON");

        // Check security scheme exists
        let schemes = &spec["components"]["securitySchemes"];
        assert!(
            schemes.get("ApiKeyAuth").is_some(),
            "openapi.json should have ApiKeyAuth security scheme"
        );

        let scheme = &schemes["ApiKeyAuth"];
        assert_eq!(
            scheme["type"], "http",
            "security scheme type should be http"
        );
        assert_eq!(
            scheme["scheme"], "bearer",
            "security scheme should be bearer"
        );

        // Check global security requirement
        let security = &spec["security"];
        assert!(
            security.as_array().map_or(false, |arr| !arr.is_empty()),
            "openapi.json should have global security requirement"
        );

        // Check at least one non-health path exists
        let paths = &spec["paths"];
        assert!(
            paths.get("/api/v1/adapters").is_some(),
            "should have /api/v1/adapters endpoint"
        );
    }));

    // Cleanup
    let _ = child.kill();
    let _ = child.wait();

    if let Err(e) = result {
        panic!("OpenAPI test failed: {:?}", e);
    }
}
