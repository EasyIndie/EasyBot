use super::*;
use crate::plugin::registry::types::{
    PluginArtifact, PluginCatalog, PluginRegistryError, PluginRequirements,
};
use crate::plugin::signing::{
    SIGNATURE_SCHEMA_VERSION, encode_public_key, generate_keypair, sign_artifact,
};
use async_trait::async_trait;

/// 内存注册表（无网络）：返回固定目录/版本/产物字节
struct TestRegistry {
    catalog: PluginCatalog,
    versions: Vec<PluginVersionMeta>,
    artifact_bytes: Vec<u8>,
}

#[async_trait]
impl PluginRegistry for TestRegistry {
    async fn catalog(&self) -> Result<PluginCatalog, PluginRegistryError> {
        Ok(self.catalog.clone())
    }
    async fn versions_for(
        &self,
        _source: &PluginSource,
        _limit: usize,
    ) -> Result<Vec<PluginVersionMeta>, PluginRegistryError> {
        Ok(self.versions.clone())
    }
    async fn download(
        &self,
        _artifact: &PluginArtifact,
        dest: &Path,
    ) -> Result<(), PluginRegistryError> {
        std::fs::write(dest, &self.artifact_bytes)?;
        Ok(())
    }
}

fn temp_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("plugin-manager-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

async fn manager_with(plugins_dir: PathBuf, registry: TestRegistry) -> Arc<PluginManager> {
    let config = Arc::new(RwLock::new(PluginConfig::default()));
    let manager = Arc::new(
        PluginManager::new(
            plugins_dir,
            config,
            Arc::new(AdapterManager::new()),
            Arc::new(EventBus::new()),
            false,
        )
        .await,
    );
    manager.set_registry_sources(vec![Arc::new(registry)]).await;
    manager
}

/// 构造一个已签名的插件版本元数据（产物字节 = b"plugin-bytes"）
fn signed_meta(
    name: &str,
    version: &str,
    publisher: &str,
    trusted: bool,
) -> (PluginVersionMeta, TestRegistry, String) {
    let (signing, verifying) = generate_keypair();
    let pk_b64 = encode_public_key(&verifying);
    let data = b"plugin-bytes".to_vec();
    let sig = sign_artifact(&data, &signing);
    let sha = crate::updater::github::sha256_hex_bytes(&data);
    let triple = current_target_triple().unwrap().to_string();

    let source = PluginSource {
        name: name.into(),
        publisher: publisher.into(),
        owner: "EasyIndie".into(),
        repo: format!("easybot-plugin-{name}"),
        display_name: Some(name.to_string()),
        description: Some("test".into()),
        tags: vec![],
        verified: trusted,
    };
    let artifact = PluginArtifact {
        url: format!(
            "https://github.com/EasyIndie/easybot-plugin-{name}/releases/download/v{version}/lib{name}.so"
        ),
        size: data.len() as u64,
        sha256: sha.clone(),
        signature: Some(sig),
        public_key: Some(pk_b64.clone()),
        library: Some(format!("lib{name}.so")),
    };
    let mut artifacts = HashMap::new();
    artifacts.insert(triple, artifact);

    let meta = PluginVersionMeta {
        schema_version: 1,
        name: name.into(),
        version: version.into(),
        sdk_version: 1,
        publisher: publisher.into(),
        tag: format!("v{version}"),
        channel: PluginChannel::Stable,
        requires: None,
        deprecated: false,
        artifacts,
    };
    let catalog = PluginCatalog {
        schema_version: 1,
        plugins: vec![source],
    };
    let registry = TestRegistry {
        catalog,
        versions: vec![meta.clone()],
        artifact_bytes: data,
    };
    (meta, registry, pk_b64)
}

#[tokio::test]
async fn test_install_requires_trust_then_succeeds() {
    let home = temp_home("install");
    let plugins = home.join("plugins");
    let (_, registry, pk_b64) = signed_meta("slack", "1.0.0", "easybot", false);
    let manager = manager_with(plugins.clone(), registry).await;

    // 未受信任发布者 → needs_trust（非错误）
    let outcome = manager
        .install(InstallRequest {
            qualified: "easybot/slack".into(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(outcome.needs_trust);
    assert!(
        !plugins.join("slack").exists(),
        "trust not granted → no install"
    );

    // 用户显式 trust 后安装成功
    let outcome = manager
        .install(InstallRequest {
            qualified: "easybot/slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!outcome.needs_trust);
    assert!(!outcome.upgraded);

    let dir = plugins.join("slack");
    assert!(dir.join("plugin.yaml").exists());
    assert!(dir.join("plugin.sig.json").exists());
    assert!(dir.join("libslack.so").exists());
    assert_eq!(
        std::fs::read(dir.join("libslack.so")).unwrap(),
        b"plugin-bytes"
    );

    // trust: true 是**一次性确认**，不写入 `.trust`（显式 plugin trust 才写）
    assert!(
        !manager
            .trust_store()
            .read()
            .unwrap()
            .is_trusted("easybot", &pk_b64),
        "install --yes must NOT auto-write .trust (explicit plugin trust required)"
    );

    // list_installed 展示签名有效
    let listed = manager.list_installed().await;
    assert_eq!(listed.len(), 1);
    let p = &listed[0];
    assert_eq!(p.name, "slack");
    assert!(p.signed);
    assert_eq!(p.signature_valid, Some(true));
    assert_eq!(p.publisher.as_deref(), Some("easybot"));

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn test_install_downgrade_rejected_and_uninstall() {
    let home = temp_home("uninstall");
    let plugins = home.join("plugins");
    let (_, registry, _pk) = signed_meta("slack", "1.0.0", "easybot", true);
    let manager = manager_with(plugins.clone(), registry).await;
    manager
        .install(InstallRequest {
            qualified: "easybot/slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(plugins.join("slack").exists());

    // 再次安装同版本 → AlreadyInstalled
    let err = manager
        .install(InstallRequest {
            qualified: "slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(err, PluginManagerError::AlreadyInstalled(_)));

    // 降级到 0.9.0 → 拒绝
    let (_, reg_old, _) = signed_meta("slack", "0.9.0", "easybot", true);
    let manager_old = manager_with(plugins.clone(), reg_old).await;
    let err = manager_old
        .install(InstallRequest {
            qualified: "slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        PluginManagerError::DowngradeNotAllowed { .. }
    ));

    // 卸载
    manager_old.uninstall("slack").await.unwrap();
    assert!(!plugins.join("slack").exists());

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn test_update_pins_current_version_by_default() {
    let home = temp_home("update");
    let plugins = home.join("plugins");
    let (_, registry, pk_b64) = signed_meta("slack", "1.0.0", "easybot", true);
    let manager = manager_with(plugins.clone(), registry).await;
    manager
        .install(InstallRequest {
            qualified: "slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap();
    // 显式信任发布者（更新不自动写 .trust，且同密钥才通过）
    manager.trust_publisher("easybot", &pk_b64).await.unwrap();

    // 默认 pin 当前版本：同版本重拉（重建刷新），不跨版本
    let outcome = manager
        .update("slack", UpdateOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome.version, "1.0.0");
    assert!(!outcome.needs_trust);

    // 注册表只有 1.0.0 时 --latest 视为已最新
    let err = manager
        .update(
            "slack",
            UpdateOptions {
                latest: true,
                channel: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PluginManagerError::AlreadyInstalled(_)));

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn test_update_key_rotation_requires_re_trust() {
    let home = temp_home("update-key-rotation");
    let plugins = home.join("plugins");
    let (_, registry, pk_a) = signed_meta("slack", "1.0.0", "easybot", true);
    let manager = manager_with(plugins.clone(), registry).await;
    manager
        .install(InstallRequest {
            qualified: "slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap();
    // 显式信任密钥 A
    manager.trust_publisher("easybot", &pk_a).await.unwrap();

    // 发布者换了签名密钥（密钥轮换/泄露）：signed_meta 每次生成独立密钥对，
    // v2.0.0 用的是另一把密钥 B → 同发布者信任不再成立 → 更新需重新信任
    let (_, reg_v2, _pk_b) = signed_meta("slack", "2.0.0", "easybot", true);
    manager.set_registry_sources(vec![Arc::new(reg_v2)]).await;
    let outcome = manager
        .update(
            "slack",
            UpdateOptions {
                latest: true,
                channel: None,
            },
        )
        .await
        .unwrap();
    assert!(
        outcome.needs_trust,
        "changed signing key must require explicit re-trust"
    );
    // needs_trust 在落位前返回 → 已装的 v1.0.0 目录保持原样（未被 v2 覆盖）
    assert_eq!(
        manager
            .read_installed_manifest("slack")
            .await
            .unwrap()
            .unwrap()
            .version,
        "1.0.0"
    );

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn test_set_enabled_and_requires_easybot_gate() {
    let home = temp_home("enabled");
    let plugins = home.join("plugins");
    let (mut meta, registry, _pk) = signed_meta("slack", "1.0.0", "easybot", true);
    // requires 不满足 → 拒绝安装
    meta.requires = Some(PluginRequirements {
        easybot: Some(">=99.0.0".into()),
    });
    let manager = manager_with(
        plugins.clone(),
        TestRegistry {
            catalog: registry.catalog,
            versions: vec![meta],
            artifact_bytes: registry.artifact_bytes,
        },
    )
    .await;
    let err = manager
        .install(InstallRequest {
            qualified: "slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        PluginManagerError::EasyBotVersionRequirement { .. }
    ));

    // 满足 requires 后安装，然后禁用/启用
    let (_, registry, _) = signed_meta("slack", "1.0.0", "easybot", true);
    let manager = manager_with(plugins.clone(), registry).await;
    manager
        .install(InstallRequest {
            qualified: "slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap();

    manager.set_enabled("slack", false).await.unwrap();
    let listed = manager.list_installed().await;
    assert!(!listed[0].enabled);

    manager.set_enabled("slack", true).await.unwrap();
    let listed = manager.list_installed().await;
    assert!(listed[0].enabled);

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_scan_unverified_detects_unsigned_tampered_and_untrusted() {
    let home = temp_home("scan");
    let plugins = home.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();

    // 空目录 + 松散文件 + 点目录 → 不报告
    std::fs::write(plugins.join("stray.so"), b"junk").unwrap();
    std::fs::create_dir_all(plugins.join(".marketplace")).unwrap();
    let empty = scan_unverified(&plugins, &PluginConfig::default(), &TrustStore::default());
    assert!(empty.is_empty(), "{empty:?}");

    // 已签名 + 配置 trusted_publishers 受信任 → 通过
    let (signing, verifying) = generate_keypair();
    let pk = encode_public_key(&verifying);
    let mut cfg = PluginConfig::default();
    cfg.trusted_publishers.insert("pub-a".into(), pk.clone());
    let trusted_dir = plugins.join("trusted-plugin");
    std::fs::create_dir_all(&trusted_dir).unwrap();
    std::fs::write(
        trusted_dir.join("plugin.yaml"),
        "name: trusted-plugin\nsdk_version: 1\nlibrary: libtrusted.so\n",
    )
    .unwrap();
    std::fs::write(trusted_dir.join("libtrusted.so"), b"plugin-bytes").unwrap();
    let sig = PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: "trusted-plugin".into(),
        version: "1.0.0".into(),
        publisher: "pub-a".into(),
        artifact: "libtrusted.so".into(),
        signature: sign_artifact(b"plugin-bytes", &signing),
        public_key: pk.clone(),
    };
    sig.write_to(&trusted_dir.join("plugin.sig.json")).unwrap();
    assert!(scan_unverified(&plugins, &cfg, &TrustStore::default()).is_empty());

    // 未签名插件 → 报告缺签名
    let unsigned_dir = plugins.join("unsigned-plugin");
    std::fs::create_dir_all(&unsigned_dir).unwrap();
    std::fs::write(
        unsigned_dir.join("plugin.yaml"),
        "name: unsigned-plugin\nsdk_version: 1\n",
    )
    .unwrap();
    let out = scan_unverified(&plugins, &cfg, &TrustStore::default());
    let (name, reason) = out.iter().find(|(n, _)| n == "unsigned-plugin").unwrap();
    assert_eq!(name, "unsigned-plugin");
    assert!(reason.contains("sig"), "{reason}");

    // 已签名但发布者未受信任 → 报告（用独立密钥对，签名本身有效）
    let (other_signing, other_verifying) = generate_keypair();
    let untrusted_dir = plugins.join("untrusted-plugin");
    std::fs::create_dir_all(&untrusted_dir).unwrap();
    std::fs::write(
        untrusted_dir.join("plugin.yaml"),
        "name: untrusted-plugin\nsdk_version: 1\nlibrary: libu.so\n",
    )
    .unwrap();
    std::fs::write(untrusted_dir.join("libu.so"), b"plugin-bytes").unwrap();
    let sig = PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: "untrusted-plugin".into(),
        version: "1.0.0".into(),
        publisher: "pub-b".into(),
        artifact: "libu.so".into(),
        signature: sign_artifact(b"plugin-bytes", &other_signing),
        public_key: encode_public_key(&other_verifying),
    };
    sig.write_to(&untrusted_dir.join("plugin.sig.json"))
        .unwrap();
    let out = scan_unverified(&plugins, &cfg, &TrustStore::default());
    let (_, reason) = out.iter().find(|(n, _)| n == "untrusted-plugin").unwrap();
    assert!(reason.contains("not trusted"), "{reason}");

    // 篡改入口文件 → 验签失败
    std::fs::write(untrusted_dir.join("libu.so"), b"tampered-bytes").unwrap();
    let out = scan_unverified(&plugins, &cfg, &TrustStore::default());
    let (_, reason) = out.iter().find(|(n, _)| n == "untrusted-plugin").unwrap();
    assert!(reason.contains("verification failed"), "{reason}");

    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn test_scan_unverified_skips_disabled_plugins() {
    let home = temp_home("scan-disabled");
    let plugins = home.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();

    // 故意禁用的未签名插件 → 不报告（不加载 → 无需验签）
    let disabled_dir = plugins.join("disabled-plugin");
    std::fs::create_dir_all(&disabled_dir).unwrap();
    std::fs::write(
        disabled_dir.join("plugin.yaml"),
        "name: disabled-plugin\nsdk_version: 1\nenabled: false\n",
    )
    .unwrap();

    let out = scan_unverified(&plugins, &PluginConfig::default(), &TrustStore::default());
    assert!(out.is_empty(), "{out:?}");

    // 同目录启用 → 缺签名被报告
    std::fs::write(
        disabled_dir.join("plugin.yaml"),
        "name: disabled-plugin\nsdk_version: 1\n",
    )
    .unwrap();
    let out = scan_unverified(&plugins, &PluginConfig::default(), &TrustStore::default());
    assert!(out.iter().any(|(n, _)| n == "disabled-plugin"), "{out:?}");

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn test_explicit_trust_writes_trust_store() {
    let home = temp_home("trust-explicit");
    let plugins = home.join("plugins");
    let (_, registry, pk_b64) = signed_meta("slack", "1.0.0", "easybot", false);
    let manager = manager_with(plugins.clone(), registry).await;

    // 安装不写 .trust
    manager
        .install(InstallRequest {
            qualified: "easybot/slack".into(),
            trust: true,
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(
        !manager
            .trust_store()
            .read()
            .unwrap()
            .is_trusted("easybot", &pk_b64)
    );

    // 显式 plugin trust → 写入 .trust 且落盘
    manager.trust_publisher("easybot", &pk_b64).await.unwrap();
    assert!(
        manager
            .trust_store()
            .read()
            .unwrap()
            .is_trusted("easybot", &pk_b64)
    );
    let on_disk = TrustStore::load(&plugins.join(".trust"));
    assert!(on_disk.is_trusted("easybot", &pk_b64));

    let _ = std::fs::remove_dir_all(&home);
}

#[tokio::test]
async fn test_install_from_file_validates_library_and_requires() {
    let home = temp_home("install-file");
    let plugins = home.join("plugins");
    let (_, registry, _) = signed_meta("slack", "1.0.0", "easybot", true);
    let manager = manager_with(plugins.clone(), registry).await;

    let src = home.join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("libslack.so"), b"plugin-bytes").unwrap();
    std::fs::write(
        src.join("plugin.yaml"),
        "name: slack\nsdk_version: 1\nlibrary: libslack.so\nrequires:\n  easybot: \">=99.0.0\"\n",
    )
    .unwrap();

    // requires 不满足 → 拒绝
    let err = manager
        .install(InstallRequest {
            qualified: "slack".into(),
            file: Some(src.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        PluginManagerError::EasyBotVersionRequirement { .. }
    ));

    // library 路径穿越 → 拒绝
    std::fs::write(
        src.join("plugin.yaml"),
        "name: slack\nsdk_version: 1\nlibrary: ../../etc/passwd\n",
    )
    .unwrap();
    let err = manager
        .install(InstallRequest {
            qualified: "slack".into(),
            file: Some(src.clone()),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(err, PluginManagerError::InvalidLibrary(_)),
        "{err:?}"
    );

    // 合法输入（签名 + 已信任发布者）→ 安装成功
    let (signing, verifying) = generate_keypair();
    let pk_b64 = encode_public_key(&verifying);
    let sig = PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: "slack".into(),
        version: "0.1.0".into(),
        publisher: "easybot".into(),
        artifact: "libslack.so".into(),
        signature: sign_artifact(b"plugin-bytes", &signing),
        public_key: pk_b64.clone(),
    };
    sig.write_to(&src.join("plugin.sig.json")).unwrap();
    manager.trust_publisher("easybot", &pk_b64).await.unwrap();
    std::fs::write(
        src.join("plugin.yaml"),
        "name: slack\nsdk_version: 1\nlibrary: libslack.so\n",
    )
    .unwrap();
    let outcome = manager
        .install(InstallRequest {
            qualified: "slack".into(),
            file: Some(src),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(!outcome.needs_trust);
    assert!(plugins.join("slack").join("libslack.so").exists());

    let _ = std::fs::remove_dir_all(&home);
}
