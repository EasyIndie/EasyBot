//! CLI 集成测试
//!
//! 验证 easybot 二进制的基本 CLI 行为。
//! 使用 std::process::Command 直接调用二进制。

use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

/// 获取 easybot 二进制路径
fn easybot_bin() -> PathBuf {
    // CARGO_MANIFEST_DIR 指向 tests/integration/
    // 需要上三层到 workspace root: tests/integration → tests → . → target
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .parent() // tests/
        .unwrap()
        .parent() // workspace root
        .unwrap();
    workspace_root.join("target").join("debug").join("easybot")
}

/// 找到一个空闲端口
fn find_free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind for port discovery");
    listener.local_addr().unwrap().port()
}

/// 在测试目录中写入 gateway.local.yaml，将 server.port 设为指定值
/// 确保并行测试不会因端口冲突而失败
fn write_port_override(dir: &std::path::Path, port: u16) {
    let content = format!("server:\n  port: {}\n", port);
    let mut file = std::fs::File::create(dir.join("gateway.local.yaml"))
        .expect("failed to create gateway.local.yaml");
    file.write_all(content.as_bytes())
        .expect("failed to write port override");
}

/// 从 --version 输出中提取版本号
fn parse_version_from_output(output: &str) -> Option<&str> {
    // 输出格式: "easybot X.Y.Z"
    // 或: "easybot 0.1.0"
    let prefix = "easybot ";
    if let Some(pos) = output.find(prefix) {
        let rest = &output[pos + prefix.len()..];
        let version = rest.split_whitespace().next()?;
        // 验证是 semver 格式
        let parts: Vec<&str> = version.split('.').collect();
        if parts.len() == 3 && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())) {
            Some(version)
        } else {
            None
        }
    } else {
        None
    }
}

#[test]
fn test_cli_version() {
    let output = Command::new(easybot_bin())
        .arg("--version")
        .output()
        .expect("failed to run easybot --version");
    assert!(output.status.success(), "--version should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("easybot"),
        "output should contain 'easybot'"
    );
    assert!(
        parse_version_from_output(&stdout).is_some(),
        "output should contain a valid semver version (X.Y.Z), got: {}",
        stdout.trim()
    );
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
    assert!(
        !output.status.success(),
        "unknown flag should exit non-zero"
    );
}

/// 以下 3 个插件门禁/CLI 测试依赖 `easybot` 二进制启用 plugin-system
/// （`easybot plugin ...` 子命令、生产模式签名扫描）。pre-push 的
/// `cargo test --all` 不带该 feature → 二进制无插件能力，跳过；
/// CI 的 `--workspace --features "default,plugin-system"` 会启用 → 仍运行。
#[cfg(feature = "plugin-system")]
#[test]
fn test_production_rejects_unverified_plugins() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let init = Command::new(easybot_bin())
        .args(["--init", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success());

    // 未签名插件目录（plugin.yaml + 库文件，无 plugin.sig.json）——生产门禁应拒绝
    let plugin_dir = dir.path().join("plugins").join("my-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.yaml"),
        "name: my-plugin\nsdk_version: 1\nlibrary: libmy_plugin.so\n",
    )
    .unwrap();
    std::fs::write(plugin_dir.join("libmy_plugin.so"), b"not-a-plugin").unwrap();

    let output = Command::new(easybot_bin())
        .args(["--production", "--dir", dir.path().to_str().unwrap()])
        .env("EASYBOT_ADMIN_PASSWORD", "a-production-password")
        .env("EASYBOT_ALLOW_PLAINTEXT", "true")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("refuses"), "{stderr}");
    assert!(stderr.contains("my-plugin"), "{stderr}");
    assert!(stderr.contains("unverified"), "{stderr}");
}

#[cfg(feature = "plugin-system")]
#[test]
fn test_production_allows_verified_trusted_plugins() {
    use easybot_core::plugin::signing::{
        PluginSignature, SIGNATURE_SCHEMA_VERSION, encode_public_key, generate_keypair,
        sign_artifact,
    };

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let init = Command::new(easybot_bin())
        .args(["--init", "--dir", dir.path().to_str().unwrap()])
        .output()
        .unwrap();
    assert!(init.status.success());

    // 生成发布者密钥对，并把公钥登记进配置 trustedPublishers（配置侧信任）
    let (signing, verifying) = generate_keypair();
    let pk = encode_public_key(&verifying);
    let port = find_free_port();
    let local_yaml =
        format!("server:\n  port: {port}\nplugins:\n  trustedPublishers:\n    testpub: \"{pk}\"\n");
    std::fs::write(dir.path().join("gateway.local.yaml"), local_yaml).unwrap();

    // 签名插件目录：plugin.yaml + 库文件 + 合法 plugin.sig.json
    let plugin_dir = dir.path().join("plugins").join("signed-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.yaml"),
        "name: signed-plugin\nsdk_version: 1\nlibrary: libsigned.so\n",
    )
    .unwrap();
    let lib = b"fake-dylib-bytes";
    std::fs::write(plugin_dir.join("libsigned.so"), lib).unwrap();
    let sig = PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: "signed-plugin".into(),
        version: "1.0.0".into(),
        publisher: "testpub".into(),
        artifact: "libsigned.so".into(),
        signature: sign_artifact(lib, &signing),
        public_key: pk,
    };
    sig.write_to(&plugin_dir.join("plugin.sig.json"))
        .expect("failed to write plugin.sig.json");

    // 生产启动应放行（服务器阻塞，用 spawn + kill 验证不因插件门禁退出）
    let mut child = Command::new(easybot_bin())
        .args(["--production", "--dir", dir.path().to_str().unwrap()])
        .env("EASYBOT_ADMIN_PASSWORD", "a-production-password")
        .env("EASYBOT_ALLOW_PLAINTEXT", "true")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to start easybot --production");

    // Let it run briefly; a spurious gate refusal would exit non-zero
    std::thread::sleep(std::time::Duration::from_millis(1200));
    match child.try_wait() {
        Ok(Some(status)) => {
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            panic!("production gate refused a verified+trusted plugin (status {status}): {stderr}");
        }
        Ok(None) => {
            let _ = child.kill();
            let _ = child.wait();
        }
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("failed to check easybot status: {e}");
        }
    }
}

#[cfg(feature = "plugin-system")]
#[test]
fn test_plugin_cli_offline_install_trust_inspect() {
    use easybot_core::plugin::signing::{
        PluginSignature, SIGNATURE_SCHEMA_VERSION, encode_public_key, generate_keypair,
        sign_artifact,
    };

    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let dir_arg = dir.path().to_str().unwrap();
    let init = Command::new(easybot_bin())
        .args(["--init", "--dir", dir_arg])
        .output()
        .unwrap();
    assert!(init.status.success());

    // 空目录 list
    let out = Command::new(easybot_bin())
        .args(["--dir", dir_arg, "plugin", "list"])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("No plugins installed"));

    // 非法公钥 → 拒绝信任
    let bad = Command::new(easybot_bin())
        .args([
            "--dir",
            dir_arg,
            "plugin",
            "trust",
            "pub-a",
            "--public-key",
            "not-a-key",
        ])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "invalid key should be rejected");

    // 生成发布者密钥对并信任
    let (signing, verifying) = generate_keypair();
    let pk = encode_public_key(&verifying);
    let ok = Command::new(easybot_bin())
        .args([
            "--dir",
            dir_arg,
            "plugin",
            "trust",
            "pub-a",
            "--public-key",
            &pk,
        ])
        .output()
        .unwrap();
    assert!(ok.status.success(), "valid key should be trusted");
    let trust_path = dir.path().join("plugins").join(".trust");
    assert!(trust_path.exists());
    assert!(
        std::fs::read_to_string(&trust_path)
            .unwrap()
            .contains("pub-a")
    );

    // 离线安装：签名插件源目录 → install --file
    //
    // 故意不写 `library` 字段——走 `install_from_file` 的缺省库名推导分支
    // （`default_library_name` 按宿主 triple 落位；曾把 triple 写死为 "host"
    // 恒落入 `.so` 分支，macOS/Windows 落位扩展名错误、加载期验签失效）。
    // 库文件名按宿主平台推导，确保本测试在 mac/linux/windows 都能通过。
    let src = dir.path().join("plugin-src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("plugin.yaml"), "name: myplugin\nsdk_version: 1\n").unwrap();
    let lib = b"fake-dylib-bytes";
    let lib_name = easybot_core::plugin::install::default_library_name(
        "myplugin",
        easybot_core::updater::types::current_target_triple().unwrap_or("unknown"),
    );
    std::fs::write(src.join(&lib_name), lib).unwrap();
    let sig = PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: "myplugin".into(),
        version: "1.0.0".into(),
        publisher: "pub-a".into(),
        artifact: lib_name.clone(),
        signature: sign_artifact(lib, &signing),
        public_key: pk,
    };
    sig.write_to(&src.join("plugin.sig.json")).unwrap();

    let inst = Command::new(easybot_bin())
        .args([
            "--dir",
            dir_arg,
            "plugin",
            "install",
            "myplugin",
            "--file",
            src.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(inst.status.success(), "offline install should succeed");
    assert!(String::from_utf8_lossy(&inst.stdout).contains("Installed"));

    // list / inspect 反映已装插件
    let list = Command::new(easybot_bin())
        .args(["--dir", dir_arg, "plugin", "list"])
        .output()
        .unwrap();
    let list_out = String::from_utf8_lossy(&list.stdout);
    assert!(list_out.contains("myplugin"), "{list_out}");
    assert!(list_out.contains("valid"), "{list_out}");

    let insp = Command::new(easybot_bin())
        .args(["--dir", dir_arg, "plugin", "inspect", "myplugin"])
        .output()
        .unwrap();
    let insp_out = String::from_utf8_lossy(&insp.stdout);
    assert!(insp_out.contains("signature: valid"), "{insp_out}");
    assert!(insp_out.contains("pub-a"), "{insp_out}");
}

#[test]
fn test_cli_short_flags() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let dir_path = dir.path().to_str().unwrap();
    let port = find_free_port();
    write_port_override(dir.path(), port);

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
            child
                .stderr
                .take()
                .unwrap()
                .read_to_string(&mut stderr)
                .unwrap();
            panic!(
                "easybot exited prematurely with status {}: {}",
                status, stderr
            );
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
    let port = find_free_port();

    // First init the dir, then write port override, then start the server
    let init = Command::new(easybot_bin())
        .arg("--init")
        .arg("--dir")
        .arg(dir_path)
        .output()
        .expect("init failed");
    assert!(init.status.success());

    // 写入 port override（必须在 --init 之后，否则会被 init 覆盖）
    write_port_override(dir.path(), port);

    let mut child = Command::new(easybot_bin())
        .arg("--debug")
        .arg("--dir")
        .arg(dir_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("failed to start easybot");

    // Wait for server to start with retry (handles parallel test runner slowdown)
    let url = format!("http://localhost:{}/openapi.json", port);
    let resp = loop {
        match ureq::get(&url).call() {
            Ok(r) => break r,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(1000)),
        }
    };

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_eq!(resp.status(), 200, "openapi.json should return 200");

        let body = resp.into_body().read_to_string().unwrap();
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
            security.as_array().is_some_and(|arr| !arr.is_empty()),
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
