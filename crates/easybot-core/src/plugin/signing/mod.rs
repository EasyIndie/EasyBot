//! 插件签名与校验（ed25519）
//!
//! # 签名对象
//!
//! 签名对象是**产物字节本身**（动态库文件内容）。元数据字段（`sha256` / `library` /
//! `version` / `publisher`）由产物字节的 sha256 间接锚定：篡改元数据里的 sha256 但保留
//! 签名 → 下载的产物与篡改后的 sha256 不匹配 → 拒；同时篡改 sha256 并换产物 → 需伪造
//! 发布者私钥 → 不可行。加上 HTTPS 拉取元数据，无需再签"摘要清单"。
//!
//! # 安全边界
//!
//! 签名只证明**作者与完整性**，**不证明代码安全**。插件以宿主权限无沙箱运行，
//! 真正的隔离边界是容器化兜底（见 `docs/18 plugin-security.md`）。
//!
//! # 密钥生命周期
//!
//! 私钥仅由发布者 CI 持有（`PUBLISHER_PRIVATE_KEY` secret），公钥随
//! `plugin.sig.json` / `easybot-plugin.json` 发布；信任登记见 [`trust`]。

pub mod trust;

use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core_06::OsRng;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// 签名文件 schema 版本
pub const SIGNATURE_SCHEMA_VERSION: u32 = 1;

// ══════════════════════════════════════════════════════════════════
// 错误类型
// ══════════════════════════════════════════════════════════════════

/// 签名操作过程中的所有可能错误
#[derive(Debug, thiserror::Error)]
pub enum SigningError {
    #[error("Failed to parse signing key: {0}")]
    KeyParse(String),

    #[error("Failed to parse signature: {0}")]
    SignatureParse(String),

    #[error("Signature verification failed: {0}")]
    VerificationFailed(String),

    #[error("Base64 decode error: {0}")]
    Base64Error(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

// ══════════════════════════════════════════════════════════════════
// 密钥操作
// ══════════════════════════════════════════════════════════════════

/// 生成新的 ed25519 密钥对
///
/// 私钥仅用于 `easybot-plugin-sign gen-keypair`（发布者本机），
/// 应保存到 CI secret；公钥登记进官方 `trusted_publishers`。
pub fn generate_keypair() -> (SigningKey, VerifyingKey) {
    let signing = SigningKey::generate(&mut OsRng);
    let verifying = signing.verifying_key();
    (signing, verifying)
}

/// 编码签名私钥为 base64
pub fn encode_signing_key(key: &SigningKey) -> String {
    b64_encode(key.to_bytes())
}

/// 编码验证公钥为 base64
pub fn encode_public_key(key: &VerifyingKey) -> String {
    b64_encode(key.to_bytes())
}

/// 解析 base64 编码的签名私钥
pub fn parse_signing_key(b64: &str) -> Result<SigningKey, SigningError> {
    let bytes: [u8; 32] = b64_decode(b64)?
        .try_into()
        .map_err(|_| SigningError::KeyParse("expected 32-byte signing key".into()))?;
    Ok(SigningKey::from_bytes(&bytes))
}

/// 解析 base64 编码的验证公钥
pub fn parse_public_key(b64: &str) -> Result<VerifyingKey, SigningError> {
    let bytes: [u8; 32] = b64_decode(b64)?
        .try_into()
        .map_err(|_| SigningError::KeyParse("expected 32-byte verifying key".into()))?;
    VerifyingKey::from_bytes(&bytes).map_err(|e| SigningError::KeyParse(e.to_string()))
}

// ══════════════════════════════════════════════════════════════════
// 签名与校验
// ══════════════════════════════════════════════════════════════════

/// 对产物字节签名，返回 base64 编码的 ed25519 签名
pub fn sign_artifact(data: &[u8], key: &SigningKey) -> String {
    let signature: Signature = key.sign(data);
    b64_encode(signature.to_bytes())
}

/// 验证产物字节的签名（纯密码学完整性校验）
///
/// 只验证"数据 + 签名 + 公钥"三者一致；该公钥是否为可信发布者密钥，
/// 由调用方结合 [`trust::TrustStore`] / 配置的 `trusted_publishers` 决定。
pub fn verify_artifact(
    data: &[u8],
    signature_b64: &str,
    public_key_b64: &str,
) -> Result<(), SigningError> {
    let public_key = parse_public_key(public_key_b64)?;
    let sig_bytes: [u8; 64] = b64_decode(signature_b64)?
        .try_into()
        .map_err(|_| SigningError::SignatureParse("expected 64-byte signature".into()))?;
    let signature = Signature::from_bytes(&sig_bytes);

    public_key
        .verify(data, &signature)
        .map_err(|e| SigningError::VerificationFailed(e.to_string()))
}

/// 计算公钥指纹（base64 公钥的前 16 字符，用于 `.trust` 匹配与 UI 展示）
pub fn public_key_fingerprint(public_key_b64: &str) -> String {
    public_key_b64.chars().take(16).collect()
}

// ══════════════════════════════════════════════════════════════════
// 签名文件
// ══════════════════════════════════════════════════════════════════

/// 插件签名文件（`plugin.sig.json`）
///
/// 存放在插件目录，`signature` 覆盖动态库文件字节。加载器（strict 模式）在
/// 启动时对磁盘上的库重新验签，防"安装后被替换"。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSignature {
    #[serde(rename = "schemaVersion")]
    pub schema_version: u32,
    /// 插件名
    pub name: String,
    /// 插件版本
    pub version: String,
    /// 发布者标识
    pub publisher: String,
    /// 被签名的动态库文件名
    pub artifact: String,
    /// base64 编码的 ed25519 签名
    pub signature: String,
    /// base64 编码的验证公钥
    pub public_key: String,
}

impl PluginSignature {
    /// 从磁盘读取 `plugin.sig.json`
    pub fn from_file(path: &Path) -> Result<Self, SigningError> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// 原子写入磁盘（temp + rename，防半写损坏）
    pub fn write_to(&self, path: &Path) -> Result<(), SigningError> {
        let text = serde_json::to_string_pretty(self)?;
        let tmp = PathBuf::from(format!("{}.tmp", path.display()));
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// 校验签名是否匹配磁盘上的动态库文件
    pub fn verify_library(&self, library_path: &Path) -> Result<(), SigningError> {
        let data = std::fs::read(library_path)?;
        verify_artifact(&data, &self.signature, &self.public_key)
    }
}

// ══════════════════════════════════════════════════════════════════
// 内部工具
// ══════════════════════════════════════════════════════════════════

use std::path::PathBuf;

fn b64_encode(data: impl AsRef<[u8]>) -> String {
    base64::engine::general_purpose::STANDARD.encode(data)
}

fn b64_decode(s: &str) -> Result<Vec<u8>, SigningError> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| SigningError::Base64Error(e.to_string()))
}

// ══════════════════════════════════════════════════════════════════
// 测试
// ══════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_roundtrip() {
        let (signing, verifying) = generate_keypair();
        let sk = encode_signing_key(&signing);
        let pk = encode_public_key(&verifying);

        let parsed_sk = parse_signing_key(&sk).unwrap();
        let parsed_pk = parse_public_key(&pk).unwrap();
        assert_eq!(parsed_pk, verifying);
        assert_eq!(parsed_sk.verifying_key(), verifying);
    }

    #[test]
    fn test_sign_verify_ok() {
        let (signing, verifying) = generate_keypair();
        let data = b"plugin artifact bytes";
        let sig = sign_artifact(data, &signing);
        let pk = encode_public_key(&verifying);

        verify_artifact(data, &sig, &pk).expect("valid signature should verify");
    }

    #[test]
    fn test_verify_tampered_data_fails() {
        let (signing, verifying) = generate_keypair();
        let sig = sign_artifact(b"original", &signing);
        let pk = encode_public_key(&verifying);

        let err = verify_artifact(b"tampered", &sig, &pk).unwrap_err();
        assert!(err.to_string().contains("failed"), "got: {err}");
    }

    #[test]
    fn test_verify_wrong_key_fails() {
        let (signing, _) = generate_keypair();
        let (_, other_pk) = generate_keypair();
        let sig = sign_artifact(b"data", &signing);
        let pk = encode_public_key(&other_pk);

        assert!(verify_artifact(b"data", &sig, &pk).is_err());
    }

    #[test]
    fn test_parse_invalid_key() {
        assert!(parse_signing_key("not-base64!!").is_err());
        // 长度错误的 base64（非 32 字节）
        let short = b64_encode([0u8; 16]);
        assert!(parse_signing_key(&short).is_err());
    }

    #[test]
    fn test_fingerprint_stable_and_short() {
        let (_, verifying) = generate_keypair();
        let pk = encode_public_key(&verifying);
        let fp = public_key_fingerprint(&pk);
        assert_eq!(fp.len(), 16);
        assert_eq!(public_key_fingerprint(&pk), fp);
    }

    #[test]
    fn test_plugin_signature_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("plugin-sig-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // 造一个假的动态库并签名
        let (signing, verifying) = generate_keypair();
        let lib_path = dir.join("libdemo.so");
        std::fs::write(&lib_path, b"fake-dylib-bytes").unwrap();
        let data = std::fs::read(&lib_path).unwrap();

        let sig = PluginSignature {
            schema_version: SIGNATURE_SCHEMA_VERSION,
            name: "demo".into(),
            version: "1.0.0".into(),
            publisher: "demo-pub".into(),
            artifact: "libdemo.so".into(),
            signature: sign_artifact(&data, &signing),
            public_key: encode_public_key(&verifying),
        };

        let sig_path = dir.join("plugin.sig.json");
        sig.write_to(&sig_path).unwrap();
        let loaded = PluginSignature::from_file(&sig_path).unwrap();
        loaded.verify_library(&lib_path).expect("should verify");

        // 篡改库文件后验证失败
        std::fs::write(&lib_path, b"tampered").unwrap();
        assert!(loaded.verify_library(&lib_path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
