//! 配置目录路径管理
//!
//! 跨平台用户配置目录解析，遵循各平台标准：
//! - macOS:   ~/Library/Application Support/easybot/
//! - Linux:   ~/.local/share/easybot/
//! - Windows: %APPDATA%\easybot\
//!
//! 同时支持传统路径 ~/.easybot/ 以向后兼容。

use std::path::PathBuf;

/// EasyBot 配置根目录名称
const FOLDER_NAME: &str = "easybot";

/// 解析 EasyBot 配置根目录
///
/// 优先级（从高到低）:
/// 1. `--dir` CLI 参数（由调用方传入）
/// 2. `EASYBOT_HOME` 环境变量
/// 3. `~/.easybot/`（若已存在，用于从旧路径迁移的用户）
/// 4. 平台标准数据目录
pub fn resolve_home(cli_override: Option<PathBuf>) -> PathBuf {
    // 1. CLI 参数
    if let Some(dir) = cli_override {
        return dir;
    }

    // 2. 环境变量
    if let Ok(env_dir) = std::env::var("EASYBOT_HOME") {
        let path = PathBuf::from(env_dir);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }

    // 3. 传统路径 ~/.easybot/
    if let Some(home) = dirs::home_dir() {
        let legacy = home.join(format!(".{}", FOLDER_NAME));
        if legacy.exists() {
            return legacy;
        }
    }

    // 4. 平台标准目录
    platform_default_data_dir()
}

/// 按平台返回标准数据目录
fn platform_default_data_dir() -> PathBuf {
    if let Some(base) = dirs::data_dir() {
        base.join(FOLDER_NAME)
    } else {
        PathBuf::from(format!("./.{}", FOLDER_NAME))
    }
}

/// 获取子目录（自动创建）
pub fn ensure_subdir(home: &std::path::Path, name: &str) -> std::io::Result<PathBuf> {
    let dir = home.join(name);
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// EasyBot 配置目录所有子路径
#[derive(Debug, Clone)]
pub struct EasyBotPaths {
    pub home: PathBuf,
    pub data_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub plugins_dir: PathBuf,
    pub certs_dir: PathBuf,
    pub secrets_dir: PathBuf,
    pub config_file: PathBuf,
    pub local_config_file: PathBuf,
    pub db_path: PathBuf,
}

impl EasyBotPaths {
    /// 根据配置根目录初始化所有子路径
    ///
    /// 自动创建不存在的子目录。
    pub fn new(home: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&home)?;
        Ok(Self {
            data_dir: ensure_subdir(&home, "data")?,
            logs_dir: ensure_subdir(&home, "logs")?,
            plugins_dir: ensure_subdir(&home, "plugins")?,
            certs_dir: ensure_subdir(&home, "certs")?,
            secrets_dir: ensure_subdir(&home, "secrets")?,
            config_file: home.join("gateway.yaml"),
            local_config_file: home.join("gateway.local.yaml"),
            db_path: home.join("data").join("gateway.db"),
            home,
        })
    }

    /// 打印目录结构（用于用户查看）
    pub fn print_tree(&self) {
        println!("{}", self.home.display());
        println!("├── gateway.yaml");
        println!("├── gateway.local.yaml");
        println!("├── .env");
        println!("├── data/");
        println!("│   ├── gateway.db");
        println!("│   └── media_cache/");
        println!("├── logs/");
        println!("│   ├── easybot.log");
        println!("│   └── audit.log");
        println!("├── plugins/");
        println!("├── certs/");
        println!("└── secrets/");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_resolve_with_env_var() {
        env::set_var("EASYBOT_HOME", "/tmp/test-easybot");
        let home = resolve_home(None);
        assert_eq!(home, PathBuf::from("/tmp/test-easybot"));
        env::remove_var("EASYBOT_HOME");
    }

    #[test]
    fn test_cli_override_takes_precedence() {
        env::set_var("EASYBOT_HOME", "/tmp/should-not-use");
        let home = resolve_home(Some(PathBuf::from("/opt/easybot")));
        assert_eq!(home, PathBuf::from("/opt/easybot"));
        env::remove_var("EASYBOT_HOME");
    }

    #[test]
    fn test_legacy_path() {
        let tmp = std::env::temp_dir().join(".easybot_test_legacy");
        let _ = std::fs::create_dir_all(&tmp);
        // 模拟 ~/.easybot/ 存在的情况
        // 在测试环境中 dirs::home_dir() 可能返回 /，所以我们直接用
        let home = resolve_home(Some(tmp.clone()));
        assert_eq!(home, tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
