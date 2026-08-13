//! `easybot plugin new` 脚手架（T18 / DX-2）
//!
//! 生成一个**独立可构建**的插件工程（SDK 走 git tag 依赖，作者无需 clone 主仓）：
//!
//! ```text
//! <name>/
//! ├── Cargo.toml                    # cdylib + SDK git 依赖 + release(LTO/panic=abort)
//! ├── .cargo/config.toml            # 构建配置（说明 [patch] 放 Cargo.toml）
//! ├── src/lib.rs                    # 完整 PlatformAdapter 骨架（TODO 占位 + 中文注释）
//! ├── plugin.yaml                   # 插件清单（name / sdk_version / author 预填）
//! ├── tests/unit.rs                 # 单元测试（身份 / 能力 / 状态）
//! ├── tests/host_test.rs            # PluginTestHost 集成测试（离线模拟宿主）
//! ├── README.md                     # 黄金路径（构建 → 联调 → 测试 → 发布）
//! ├── .gitignore
//! ├── LICENSE                       # MIT（作者占位）
//! └── .github/workflows/
//!     └── plugin-publish.yml        # 发布者 CI 模板（copy，include_str! 保证同步）
//! ```
//!
//! 模板版本对齐：SDK git tag 由**编译时版本常量**生成（`v{CARGO_PKG_VERSION}`），
//! 保证脚手架生成的工程依赖与运行它的 EasyBot 版本一致（对齐
//! `generate_default_config` 的模式）。

use anyhow::Context;
use std::path::{Path, PathBuf};

use crate::plugin_scaffold_template as tpl;

/// 脚手架选项（来自 `easybot plugin new` 子命令）。
pub struct ScaffoldOptions {
    /// 插件名（kebab-case，同时是平台名与 Cargo 包名）。
    pub name: String,
    /// 目标父目录（默认当前目录；工程建在 `<target-dir>/<name>/`）。
    pub target_dir: Option<PathBuf>,
    /// 作者署名（写入 plugin.yaml / LICENSE）。
    pub author: Option<String>,
}

/// 执行脚手架生成，返回生成的工程根目录。
pub fn scaffold(opts: &ScaffoldOptions) -> anyhow::Result<PathBuf> {
    let name = opts.name.trim();
    validate_name(name)?;

    let vars = tpl::TemplateVars {
        name: name.to_string(),
        crate_name: crate_name(name),
        struct_name: pascal(name),
        display_name: humanize(name),
        description: format!("An EasyBot adapter plugin named {name}."),
        author: opts
            .author
            .clone()
            .unwrap_or_else(|| "Your Name <you@example.com>".into()),
        sdk_tag: format!("v{}", env!("CARGO_PKG_VERSION")),
        sdk_version: easybot_plugin_sdk::EASYBOT_PLUGIN_ABI_VERSION,
    };

    let root = opts
        .target_dir
        .as_deref()
        .unwrap_or_else(|| Path::new("."))
        .join(name);
    if root.exists()
        && std::fs::read_dir(&root)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false)
    {
        anyhow::bail!(
            "target directory already exists and is not empty: {}",
            root.display()
        );
    }

    let files: Vec<(&str, String)> = vec![
        ("Cargo.toml", tpl::render(tpl::CARGO_TOML, &vars)),
        (".cargo/config.toml", tpl::render(tpl::CARGO_CONFIG, &vars)),
        ("src/lib.rs", tpl::render(tpl::SRC_LIB_RS, &vars)),
        ("plugin.yaml", tpl::render(tpl::PLUGIN_YAML, &vars)),
        ("tests/unit.rs", tpl::render(tpl::TESTS_UNIT, &vars)),
        ("tests/host_test.rs", tpl::render(tpl::TESTS_HOST, &vars)),
        ("README.md", tpl::render(tpl::README, &vars)),
        (".gitignore", tpl::render(tpl::GITIGNORE, &vars)),
        ("LICENSE", tpl::render(tpl::LICENSE, &vars)),
        (
            ".github/workflows/plugin-publish.yml",
            tpl::render(tpl::PLUGIN_PUBLISH_YML, &vars),
        ),
    ];

    for (rel, content) in &files {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create parent dir for {}", path.display()))?;
        }
        std::fs::write(&path, content).with_context(|| format!("write {}", path.display()))?;
    }

    println!("✓ Scaffolded plugin project at {}", root.display());
    println!();
    println!("  Next steps:");
    println!("    cd {}", root.display());
    println!("    cargo build --release   # build self-contained cdylib");
    println!("    cargo test              # offline unit + PluginTestHost tests");
    let lib_ext = if cfg!(target_os = "macos") {
        "dylib"
    } else if cfg!(target_os = "windows") {
        "dll"
    } else {
        "so"
    };
    println!(
        "    cp target/release/lib{}.{} ./   # copy the cdylib next to plugin.yaml",
        crate_name(name),
        lib_ext
    );
    println!(
        "    easybot plugin install --file . {name}  # install into local host (offline path, full verify)"
    );
    println!();
    println!("  Publish: see README.md (gen-keypair → register public key → push tag)");
    Ok(root)
}

/// 校验插件名：`[a-z0-9][a-z0-9_-]*`（拒绝路径穿越 / 空白 / 大写）。
fn validate_name(name: &str) -> anyhow::Result<()> {
    if name.is_empty() {
        anyhow::bail!("plugin name must not be empty");
    }
    let valid = name.chars().enumerate().all(|(i, c)| {
        (c.is_ascii_lowercase() && c.is_ascii_alphabetic())
            || (c.is_ascii_digit() && i > 0)
            || ((c == '-' || c == '_') && i > 0)
    });
    if !valid {
        anyhow::bail!(
            "invalid plugin name '{name}' — must match [a-z0-9][a-z0-9_-]* (kebab-case recommended, e.g. my-adapter)"
        );
    }
    Ok(())
}

/// `-`/`_` 转 `_`（Rust lib 名与集成测试 import 路径）。
fn crate_name(name: &str) -> String {
    name.replace('-', "_")
}

/// 转 PascalCase（如 `hello-adapter` → `HelloAdapter`，用于结构体名）。
fn pascal(name: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;
    for c in name.chars() {
        if c == '-' || c == '_' {
            capitalize = true;
        } else if capitalize {
            out.push(c.to_ascii_uppercase());
            capitalize = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// 人类可读显示名（如 `hello-adapter` → `Hello Adapter`）。
fn humanize(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|s| !s.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    let mut s = first.to_ascii_uppercase().to_string();
                    s.push_str(chars.as_str());
                    s
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独立的临时目录（避免并行测试互相干扰）。
    fn temp_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "easybot-scaffold-test-{}-{}",
            std::process::id(),
            tag
        ))
    }

    #[test]
    fn naming_helpers() {
        assert_eq!(crate_name("hello-adapter"), "hello_adapter");
        assert_eq!(pascal("hello-adapter"), "HelloAdapter");
        assert_eq!(pascal("my_cool_plugin"), "MyCoolPlugin");
        assert_eq!(humanize("hello-adapter"), "Hello Adapter");
        assert_eq!(humanize("my_cool_plugin"), "My Cool Plugin");
        assert_eq!(humanize("a"), "A");
    }

    #[test]
    fn validate_name_accepts_kebab_and_rejects_traversal() {
        assert!(validate_name("my-adapter").is_ok());
        assert!(validate_name("hello").is_ok());
        assert!(validate_name("plugin_2").is_ok());
        assert!(validate_name("../evil").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("..").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("-bad").is_err());
        assert!(validate_name("Uppercase").is_err());
        assert!(validate_name("has space").is_err());
    }

    #[test]
    fn scaffold_writes_expected_files() {
        let root = temp_dir("writes");
        let _ = std::fs::remove_dir_all(&root);
        let result = scaffold(&ScaffoldOptions {
            name: "hello-adapter".into(),
            target_dir: Some(root.clone()),
            author: Some("Test Author <t@example.com>".into()),
        });
        assert!(result.is_ok(), "scaffold failed: {:?}", result.err());

        let generated = root.join("hello-adapter");
        for rel in tpl::FILES {
            assert!(
                generated.join(rel).exists(),
                "missing generated file: {rel}"
            );
        }

        // Cargo.toml：cdylib + SDK git tag = v{CARGO_PKG_VERSION}
        let cargo_toml = std::fs::read_to_string(generated.join("Cargo.toml")).unwrap();
        assert!(cargo_toml.contains("crate-type = [\"cdylib\", \"rlib\"]"));
        let expected_tag = format!("v{}", env!("CARGO_PKG_VERSION"));
        assert!(
            cargo_toml.contains(&format!("tag = \"{expected_tag}\"")),
            "SDK tag must be derived from compile-time version constant"
        );

        // src/lib.rs：结构体名 / 平台名 / declare_plugin
        let lib_rs = std::fs::read_to_string(generated.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("pub struct HelloAdapter"));
        assert!(lib_rs.contains("fn platform_name"));
        assert!(lib_rs.contains("declare_plugin!(HelloAdapter, HelloAdapter::new);"));

        // plugin.yaml：sdk_version 与 ABI 常量一致
        let plugin_yaml = std::fs::read_to_string(generated.join("plugin.yaml")).unwrap();
        assert!(plugin_yaml.contains("name: \"hello-adapter\""));
        assert!(plugin_yaml.contains(&format!(
            "sdk_version: {}",
            easybot_plugin_sdk::EASYBOT_PLUGIN_ABI_VERSION
        )));

        // tests/unit.rs：import 路径用下划线 crate 名
        let unit = std::fs::read_to_string(generated.join("tests/unit.rs")).unwrap();
        assert!(unit.contains("use hello_adapter::HelloAdapter;"));

        // plugin-publish.yml 与主仓模板一致（include_str! 保证）
        let ci = std::fs::read_to_string(generated.join(".github/workflows/plugin-publish.yml"))
            .unwrap();
        assert!(ci.contains("name: Publish Plugin"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scaffold_refuses_existing_nonempty_dir() {
        let root = temp_dir("exists");
        let _ = std::fs::remove_dir_all(&root);
        let target = root.join("existing");
        std::fs::create_dir_all(target.join("src")).unwrap();
        std::fs::write(target.join("src/main.rs"), "").unwrap();

        let result = scaffold(&ScaffoldOptions {
            name: "existing".into(),
            target_dir: Some(root.clone()),
            author: None,
        });
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
