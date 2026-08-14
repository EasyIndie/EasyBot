//! EasyBot 插件签名工具
//!
//! 供插件发布者 CI 使用（见 `.github/workflows/plugin-publish.yml` 模板）。
//!
//! 私钥仅应存在于发布者的 GitHub Actions secret（`PUBLISHER_PRIVATE_KEY`）；
//! 公钥登记进官方 `trusted_publishers` 后，客户端才能验签安装。
//!
//! # 用法
//!
//! ```text
//! # 首次：生成本机密钥对（私钥仅显示一次，保存到 CI secret）
//! easybot-plugin-sign gen-keypair
//!
//! # CI：对每个产物签名，输出 artifact 元数据（供组装 easybot-plugin.json）
//! easybot-plugin-sign sign --key "$PUBLISHER_PRIVATE_KEY" \
//!   --publisher my-org --version 1.0.0 --triple x86_64-unknown-linux-musl \
//!   --artifact ./target/x86_64-unknown-linux-musl/release/libmy-plugin.so
//!
//! # 离线分发（`plugin install --file`）：额外写出 plugin.sig.json
//! # --name 传 plugin.yaml 的 name（kebab-case 插件必须，库名推导是下划线 crate 名）
//! easybot-plugin-sign sign --key "$PRIVATE_KEY" \
//!   --publisher my-org --version 1.0.0 --triple x86_64-apple-darwin \
//!   --artifact ./target/release/libmy_adapter.dylib --name my-adapter \
//!   --sig-json ./plugin.sig.json
//! ```

use clap::{Parser, Subcommand};
use easybot_core::plugin::signing::{
    PluginSignature, SIGNATURE_SCHEMA_VERSION, encode_public_key, encode_signing_key,
    generate_keypair, parse_signing_key, sign_artifact,
};
use sha2::Digest;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "easybot-plugin-sign",
    about = "EasyBot 插件签名工具（发布者 CI）",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 生成 ed25519 密钥对（私钥仅输出一次，请保存到 CI secret）
    GenKeypair,
    /// 对产物动态库签名并输出 artifact 元数据 JSON
    /// （Box 化避免 large_enum_variant：Windows 布局下 SignArgs 相对 GenKeypair
    ///  超 clippy 200B 阈值；CLI 一次性解析，Box 零成本）
    Sign(Box<SignArgs>),
}

/// 从 library 文件名推导插件名（`lib{name}.so` / `{name}.dll` → `name`）
fn library_name_for(library: &str) -> String {
    let stem = PathBuf::from(library)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| library.to_string());
    stem.strip_prefix("lib").unwrap_or(&stem).to_string()
}

/// 组装离线分发用的 `plugin.sig.json` 内容（PluginSignature 格式）
///
/// 与 market 格式 artifact 条目（`{triple, library, sha256, ...}`）同一次签名、
/// 同一份字节，只是 JSON 外壳不同——`install --file` 读取此文件验签+信任。
fn build_plugin_signature(
    name: &str,
    version: &str,
    publisher: &str,
    library: &str,
    signature: &str,
    public_key: &str,
) -> PluginSignature {
    PluginSignature {
        schema_version: SIGNATURE_SCHEMA_VERSION,
        name: name.to_string(),
        version: version.to_string(),
        publisher: publisher.to_string(),
        artifact: library.to_string(),
        signature: signature.to_string(),
        public_key: public_key.to_string(),
    }
}

#[derive(clap::Args)]
struct SignArgs {
    /// base64 编码的 ed25519 私钥（未传时读取 PUBLISHER_PRIVATE_KEY 环境变量）
    #[arg(long)]
    key: Option<String>,
    /// 发布者标识
    #[arg(long)]
    publisher: String,
    /// 插件语义化版本（如 1.0.0）
    #[arg(long)]
    version: String,
    /// 目标平台 triple（如 x86_64-unknown-linux-musl）
    #[arg(long)]
    triple: String,
    /// 产物动态库文件路径
    #[arg(long)]
    artifact: PathBuf,
    /// 动态库文件名（默认取 artifact 文件名）
    #[arg(long)]
    library: Option<String>,
    /// 插件名（默认从库文件名推导；kebab-case 插件须显式传 plugin.yaml 的 name）
    #[arg(long)]
    name: Option<String>,
    /// 额外写出离线分发用的 plugin.sig.json 到该路径
    /// （`install --file` 读取此格式；stdout 仍输出 market 格式 artifact 条目）
    #[arg(long)]
    sig_json: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::GenKeypair => {
            let (signing, verifying) = generate_keypair();
            // 公钥到 stdout（可重定向保存登记），私钥同样输出但提示仅此一次
            println!("PUBLIC_KEY={}", encode_public_key(&verifying));
            println!("PRIVATE_KEY={}", encode_signing_key(&signing));
            eprintln!(
                "⚠ 私钥仅显示这一次。请保存到 GitHub Actions secret `PUBLISHER_PRIVATE_KEY`，\
                 公钥（PUBLIC_KEY）提交给 EasyBot 官方登记 trusted_publishers。"
            );
        }
        Command::Sign(args) => {
            let SignArgs {
                key,
                publisher,
                version,
                triple,
                artifact,
                library,
                name,
                sig_json,
            } = *args;

            let key_b64 = match key {
                Some(k) => k,
                None => std::env::var("PUBLISHER_PRIVATE_KEY").map_err(|_| {
                    anyhow::anyhow!("--key 未提供且 PUBLISHER_PRIVATE_KEY 环境变量未设置")
                })?,
            };
            let key = parse_signing_key(&key_b64)?;
            let data = std::fs::read(&artifact)?;

            let signature = sign_artifact(&data, &key);
            let public_key = encode_public_key(&key.verifying_key());
            let sha256: String = {
                let mut hasher = sha2::Sha256::new();
                hasher.update(&data);
                hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect()
            };

            let library = match library {
                Some(lib) => lib,
                None => artifact
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| format!("lib{}.so", triple)),
            };
            // 插件名：显式 --name 优先（kebab-case 插件须传 plugin.yaml 的 name，
            // 因为库名推导会得到下划线 crate 名）；否则从库文件名推导。
            let name = name.unwrap_or_else(|| library_name_for(&library));

            // 输出 artifact 条目，CI 模板用 jq 组装进 easybot-plugin.json 的 artifacts map。
            // publisher/version/triple 一并输出，供脚本直接复用（避免重复传参）。
            let entry = serde_json::json!({
                "name": name,
                "publisher": publisher,
                "version": version,
                "triple": triple,
                "library": library,
                "size": data.len(),
                "sha256": sha256,
                "signature": signature,
                "public_key": public_key,
            });
            println!("{}", serde_json::to_string_pretty(&entry)?);

            // 离线分发（install --file）：额外写出 plugin.sig.json（PluginSignature 格式）。
            // 与市场条目同一次签名、同一份字节——只换 JSON 外壳；stdout 保持 market 格式不变，
            // 发布 CI 模板不受影响。
            if let Some(out) = sig_json {
                let sig = build_plugin_signature(
                    &name,
                    &version,
                    &publisher,
                    &library,
                    &signature,
                    &public_key,
                );
                sig.write_to(&out)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_core::plugin::signing::{
        encode_public_key, generate_keypair, parse_signing_key, sign_artifact,
    };

    /// `sign --sig-json` 写出的 plugin.sig.json 必须满足离线 `install --file` 的契约：
    /// 能被 `PluginSignature::from_file` 读回，且签名对磁盘库文件验证通过。
    #[test]
    fn sig_json_round_trip_verifies_artifact() {
        let (signing, _verifying) = generate_keypair();
        let signing_b64 = encode_signing_key(&signing);
        let key = parse_signing_key(&signing_b64).unwrap();
        let data = b"fake-dylib-bytes-for-offline-install";

        // 与 `sign` 命令相同的推导路径：name=显式 --name，artifact=library 文件名
        let library = "libhello_adapter.dylib";
        let sig = build_plugin_signature(
            "hello-adapter",
            "0.1.0",
            "accept",
            library,
            &sign_artifact(data, &key),
            &encode_public_key(&key.verifying_key()),
        );

        let dir = std::env::temp_dir().join(format!("sign-tool-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lib_path = dir.join(library);
        std::fs::write(&lib_path, data).unwrap();

        let sig_path = dir.join("plugin.sig.json");
        sig.write_to(&sig_path).unwrap();

        // 读回 + 验签——install --file 的完整契约
        let read_back = PluginSignature::from_file(&sig_path).unwrap();
        assert_eq!(read_back.name, "hello-adapter");
        assert_eq!(read_back.publisher, "accept");
        assert_eq!(read_back.artifact, "libhello_adapter.dylib");
        read_back.verify_library(&lib_path).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// kebab-case 插件名经 `--name` 显式传入时，market 条目与 plugin.sig.json
    /// 的 name 都用显式值（库名推导会得到下划线 crate 名，不能用作插件名）。
    #[test]
    fn explicit_name_beats_library_derivation() {
        // libhello_adapter.dylib → library_name_for → "hello_adapter"（下划线）
        assert_eq!(library_name_for("libhello_adapter.dylib"), "hello_adapter");
        // 显式 --name 覆盖推导
        let sig = build_plugin_signature(
            "hello-adapter",
            "0.1.0",
            "accept",
            "libhello_adapter.dylib",
            "sig",
            "pk",
        );
        assert_eq!(sig.name, "hello-adapter");
    }
}
