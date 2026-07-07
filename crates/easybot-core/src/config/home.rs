//! 配置目录路径管理
//!
//! 跨平台用户配置目录解析：
//! - macOS/Linux: ~/.easybot/
//! - Windows:     %APPDATA%\easybot\

use std::path::PathBuf;

/// EasyBot 配置根目录名称
const FOLDER_NAME: &str = "easybot";

/// 解析 EasyBot 配置根目录
///
/// 优先级（从高到低）:
/// 1. `--dir` CLI 参数（由调用方传入）
/// 2. `EASYBOT_HOME` 环境变量
/// 3. 平台默认目录:
///    - macOS/Linux: ~/.easybot/
///    - Windows:     %APPDATA%\easybot\
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

    // 3. 平台默认目录
    platform_default_data_dir()
}

/// 按平台返回默认配置目录
///
/// - macOS/Linux: ~/.easybot/          （类 Unix hidden dir）
/// - Windows:     %APPDATA%\easybot\   （平台标准数据目录）
fn platform_default_data_dir() -> PathBuf {
    // macOS / Linux → ~/.easybot/
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        if let Some(home) = dirs::home_dir() {
            return home.join(format!(".{}", FOLDER_NAME));
        }
    }

    // Windows（及其他平台）→ 平台标准数据目录
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
    pub env_path: PathBuf,
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
            env_path: home.join(".env"),
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
        if cfg!(target_os = "linux") {
            println!("├── easybot.service         # systemd 服务单元");
            println!("├── easybot.sh              # 服务管理脚本");
        } else if cfg!(target_os = "macos") {
            println!("├── com.easybot.gateway.plist  # launchd 服务配置");
            println!("├── easybot.sh              # 服务管理脚本");
        } else if cfg!(target_os = "windows") {
            println!("├── manage-service.ps1      # Windows 服务管理脚本");
        }
        println!("├── data/");
        println!("│   ├── gateway.db");
        println!("│   └── media_cache/");
        println!("├── logs/");
        println!("│   ├── easybot.YYYY-MM-DD.log  # 当 logging.output = \"file\" 时写入");
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
        // SAFETY: 单线程测试
        unsafe { env::set_var("EASYBOT_HOME", "/tmp/test-easybot") };
        let home = resolve_home(None);
        assert_eq!(home, PathBuf::from("/tmp/test-easybot"));
        // SAFETY: 单线程测试
        unsafe { env::remove_var("EASYBOT_HOME") };
    }

    #[test]
    fn test_cli_override_takes_precedence() {
        // SAFETY: 单线程测试
        unsafe { env::set_var("EASYBOT_HOME", "/tmp/should-not-use") };
        let home = resolve_home(Some(PathBuf::from("/opt/easybot")));
        assert_eq!(home, PathBuf::from("/opt/easybot"));
        // SAFETY: 单线程测试
        unsafe { env::remove_var("EASYBOT_HOME") };
    }

    #[test]
    fn test_env_path_in_paths() {
        let dir = env::temp_dir().join("easybot_paths_env_test");
        let paths = EasyBotPaths::new(dir.clone()).unwrap();
        assert_eq!(paths.env_path, dir.join(".env"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_legacy_path() {
        let tmp = env::temp_dir().join(".easybot_test_legacy");
        let _ = std::fs::create_dir_all(&tmp);
        // 模拟 ~/.easybot/ 存在的情况
        // 在测试环境中 dirs::home_dir() 可能返回 /，所以我们直接用
        let home = resolve_home(Some(tmp.clone()));
        assert_eq!(home, tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
