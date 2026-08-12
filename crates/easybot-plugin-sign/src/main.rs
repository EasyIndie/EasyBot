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
//! ```

use clap::{Parser, Subcommand};
use easybot_core::plugin::signing::{
    encode_public_key, encode_signing_key, generate_keypair, parse_signing_key, sign_artifact,
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
    Sign(SignArgs),
}

/// 从 library 文件名推导插件名（`lib{name}.so` / `{name}.dll` → `name`）
fn library_name_for(library: &str) -> String {
    let stem = PathBuf::from(library)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| library.to_string());
    stem.strip_prefix("lib").unwrap_or(&stem).to_string()
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
            } = args;

            let key_b64 = match key {
                Some(k) => k,
                None => std::env::var("PUBLISHER_PRIVATE_KEY").map_err(|_| {
                    anyhow::anyhow!("--key 未提供且 PUBLISHER_PRIVATE_KEY 环境变量未设置")
                })?,
            };
            let key = parse_signing_key(&key_b64)?;
            let data = std::fs::read(&artifact)?;

            let signature = sign_artifact(&data, &key);
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

            // 输出 artifact 条目，CI 模板用 jq 组装进 easybot-plugin.json 的 artifacts map。
            // publisher/version/triple 一并输出，供脚本直接复用（避免重复传参）。
            let entry = serde_json::json!({
                "name": library_name_for(&library),
                "publisher": publisher,
                "version": version,
                "triple": triple,
                "library": library,
                "size": data.len(),
                "sha256": sha256,
                "signature": signature,
            });
            println!("{}", serde_json::to_string_pretty(&entry)?);
        }
    }
    Ok(())
}
