//! tracing 日志系统初始化辅助。
//!
//! 控制台/文件输出（`fmt` 层）与管理后台 `/api/v1/logs` 内存环形缓冲
//! （`LogCollector` 层）**必须共用同一过滤规则**。此前 `LogCollector` 层没有挂
//! 过滤器，因而接收并保存所有 target 的 `TRACE`/`DEBUG` 事件（`hyper` /
//! `sqlx` / `tower_http` / `axum` / `rustls` 等），造成两个后果：
//!
//! 1. 环形缓冲（上限 5000 条）被依赖库内部噪音占满，业务与插件日志被挤掉，
//!    管理后台「日志」页几乎不可用；
//! 2. `tracing` 的全局最大级别被抬到 `TRACE`——未过滤的层不会对最大级别提供
//!    任何约束，于是所有 `trace!`/`debug!` 宏（包括依赖里的）都会被真实构造与
//!    记录，带来不必要的 CPU 与内存开销。
//!
//! 过滤优先级：`RUST_LOG`（存在且非空）> `easybot=<配置级别>`。
//! `docs/16`、`docs/17` 承诺 `RUST_LOG=<target>=<level>` 可按 target 细分，
//! 因此这里读取 `RUST_LOG`，而不是只认配置里的级别。

use tracing_subscriber::EnvFilter;

/// 构建控制台/文件与内存收集器共用的过滤规则。
///
/// - `log_level`：来自配置 `logging.level` 或 CLI `--debug`；
/// - `rust_log`：调用方传入的 `RUST_LOG`（显式传参便于单测注入，避免直接读环境变量）。
pub fn build_filter(log_level: &str, rust_log: Option<&str>) -> EnvFilter {
    match rust_log.map(str::trim).filter(|value| !value.is_empty()) {
        Some(directives) => EnvFilter::new(directives),
        None => EnvFilter::new(format!("easybot={log_level}")),
    }
}

// ── 按大小轮转的文件日志 ─────────────────────────────────────────────
//
// `tracing_appender::rolling` 只支持按时间（分/时/天）轮转，**没有大小上限**；
// 长期 24×7 运行时 `output: file` 仍可能把磁盘写满（#204）。这里实现 logrotate
// 风格的按大小轮转：写满就把当前文件重命名为 `.1`，旧的依次后移，超出保留数的删除，
// 可选 gzip 压缩。对外仍是一个 `Write`，可直接交给 `tracing_appender::non_blocking`。

/// 文件日志的落盘与轮转参数（由 `logging.file.*` 配置换算而来）
#[derive(Debug, Clone)]
pub struct RotationConfig {
    /// 当前写入的文件路径
    pub path: std::path::PathBuf,
    /// 单文件大小上限（字节）。0 = 不按大小轮转。
    pub max_bytes: u64,
    /// 保留的轮转文件个数（不含当前文件）
    pub max_files: usize,
    /// 轮转后是否 gzip
    pub compress: bool,
}

/// 轮转文件路径：`easybot.log` → `easybot.log.3` / `easybot.log.3.gz`
fn rotated_path(base: &std::path::Path, index: usize, gz: bool) -> std::path::PathBuf {
    let mut name = base.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{index}"));
    if gz {
        name.push(".gz");
    }
    base.with_file_name(name)
}

/// 删除编号大于 `keep` 的轮转文件（`name.N` / `name.N.gz`）
fn prune_rotation_surplus(base: &std::path::Path, keep: usize) {
    let Some(dir) = base.parent() else {
        return;
    };
    let Some(base_name) = base.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    let prefix = format!("{base_name}.");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let digits = rest.strip_suffix(".gz").unwrap_or(rest);
        // 只处理 `<数字>` 或 `<数字>.gz`，避免误删用户自己的 easybot.log.bak 之类
        let Ok(index) = digits.parse::<usize>() else {
            continue;
        };
        if index > keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

struct RotationState {
    cfg: RotationConfig,
    file: Option<std::fs::File>,
    written: u64,
}

impl RotationState {
    fn open(&mut self) -> std::io::Result<()> {
        if let Some(parent) = self.cfg.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.cfg.path)?;
        self.written = file.metadata().map(|m| m.len()).unwrap_or(0);
        self.file = Some(file);
        Ok(())
    }

    /// 把当前文件移到 `.1`（可压缩），其余旧文件依次后移，超出 `max_files` 的删除。
    fn rotate(&mut self) -> std::io::Result<()> {
        // 先关闭句柄，否则 Windows 上无法重命名
        if let Some(file) = self.file.take() {
            drop(file);
        }
        let gz = self.cfg.compress;
        let base = self.cfg.path.clone();

        // 删除超出保留数的轮转文件：扫目录而不是只清固定的几个编号，
        // 这样把 maxFiles 调小之后，遗留的 `.7`、`.8` 也会在下次轮转时被清掉。
        prune_rotation_surplus(&base, self.cfg.max_files);
        if self.cfg.max_files == 0 {
            let _ = std::fs::remove_file(&base);
            return self.open();
        }

        // 依次后移：max_files-1 → max_files，…，1 → 2
        for index in (1..self.cfg.max_files).rev() {
            for suffix_gz in [true, false] {
                let from = rotated_path(&base, index, suffix_gz);
                if from.exists() {
                    let to = rotated_path(&base, index + 1, suffix_gz);
                    let _ = std::fs::remove_file(&to);
                    if let Err(e) = std::fs::rename(&from, &to) {
                        tracing::warn!(
                            "Failed to rotate log file {} -> {}: {e}",
                            from.display(),
                            to.display()
                        );
                    }
                }
            }
        }

        // 当前文件 → .1
        if gz {
            let target = rotated_path(&base, 1, true);
            if let Err(e) = gzip_file(&base, &target) {
                tracing::warn!("Failed to compress log file {}: {e}", base.display());
                let _ = std::fs::rename(&base, rotated_path(&base, 1, false));
            }
        } else if let Err(e) = std::fs::rename(&base, rotated_path(&base, 1, false)) {
            tracing::warn!("Failed to rotate log file {}: {e}", base.display());
        }
        self.open()
    }
}

fn gzip_file(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(from)?;
    let out = std::fs::File::create(to)?;
    let mut encoder = flate2::write::GzEncoder::new(out, flate2::Compression::fast());
    std::io::copy(&mut input, &mut encoder)?;
    encoder.finish()?;
    std::fs::remove_file(from)?;
    Ok(())
}

/// 按大小轮转的文件写入器（实现 `Write` + `MakeWriter`，可直接交给 tracing）
#[derive(Clone)]
pub struct SizeRotatingWriter {
    state: std::sync::Arc<std::sync::Mutex<RotationState>>,
}

impl SizeRotatingWriter {
    /// 打开（或续写）`cfg.path`；父目录不存在时自动创建。
    pub fn new(cfg: RotationConfig) -> std::io::Result<Self> {
        let mut state = RotationState {
            cfg,
            file: None,
            written: 0,
        };
        state.open()?;
        Ok(Self {
            state: std::sync::Arc::new(std::sync::Mutex::new(state)),
        })
    }
}

impl std::io::Write for SizeRotatingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        // 单条日志不应拆到两个文件里：写满即轮转
        if state.cfg.max_bytes > 0
            && state.written > 0
            && state.written + buf.len() as u64 > state.cfg.max_bytes
        {
            state.rotate()?;
        }
        if state.file.is_none() {
            state.open()?;
        }
        let file = state
            .file
            .as_mut()
            .ok_or_else(|| std::io::Error::other("log file handle is not open"))?;
        let written = file.write(buf)?;
        state.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let mut state = match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        match state.file.as_mut() {
            Some(file) => file.flush(),
            None => Ok(()),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for SizeRotatingWriter {
    type Writer = SizeRotatingWriter;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// 把日志时间戳格式化成“本地时区（RFC3339 带偏移）”还是“UTC（`Z` 结尾）”。
///
/// `local` 依赖系统时区数据：Docker 镜像已装 tzdata，裸机用 OS 的；
/// 解析不出时区时 chrono 会退回 UTC，不会 panic。
pub fn log_timer(timezone: &str) -> LogTimer {
    match timezone.trim().to_ascii_lowercase().as_str() {
        "utc" | "z" => LogTimer::Utc,
        _ => LogTimer::Local,
    }
}

/// 日志时间戳时区选择（对应 `logging.timezone` / `EASYBOT_LOG_TIMEZONE`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogTimer {
    Utc,
    Local,
}

#[cfg(test)]
mod tests {
    use super::*;
    use easybot_api::log_collector::LogCollector;
    use std::io::Write as _;
    use tracing_subscriber::Layer;
    use tracing_subscriber::layer::SubscriberExt;

    /// 用给定过滤器把事件送进内存收集器，返回实际被保留的 message。
    fn captured(filter: EnvFilter, emit: impl FnOnce()) -> Vec<String> {
        let collector = LogCollector::new(64);
        let subscriber = tracing_subscriber::registry().with(collector.clone().with_filter(filter));
        tracing::subscriber::with_default(subscriber, emit);
        collector
            .query(None, None, 64, None)
            .into_iter()
            .map(|entry| entry.message)
            .collect()
    }

    #[test]
    fn collector_only_keeps_matching_target_and_level() {
        // 配置 info：保留 easybot* 的 INFO，丢弃 easybot 的 DEBUG 与依赖库噪音。
        let kept = captured(build_filter("info", None), || {
            tracing::info!(target: "easybot_core::test", "kept-app-log");
            tracing::debug!(target: "easybot_core::test", "dropped-debug");
            tracing::info!(target: "sqlx::query", "dropped-dependency-noise");
        });
        assert_eq!(kept, vec!["kept-app-log".to_string()]);
    }

    #[test]
    fn explicit_rust_log_takes_over() {
        // RUST_LOG=my_adapter=trace：只采集该 target，配置里的 easybot 级别被覆盖。
        let kept = captured(build_filter("info", Some("my_adapter=trace")), || {
            tracing::trace!(target: "my_adapter::ws", "plugin-detail");
            tracing::info!(target: "easybot_core::test", "hidden-by-rust-log");
        });
        assert_eq!(kept, vec!["plugin-detail".to_string()]);
    }

    // ── 文件日志：按大小轮转 ────────────────────────────────────

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "easybot-log-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_lines(writer: &mut SizeRotatingWriter, lines: &[&str]) {
        for line in lines {
            writer.write_all(line.as_bytes()).expect("write log line");
        }
        writer.flush().expect("flush");
    }

    #[test]
    fn rotates_by_size_and_keeps_max_files() {
        let dir = tmp_dir("rotate");
        let path = dir.join("easybot.log");
        let mut writer = SizeRotatingWriter::new(RotationConfig {
            path: path.clone(),
            max_bytes: 32,
            max_files: 2,
            compress: false,
        })
        .expect("open writer");

        // 每条 16 字节 → 每两条触发一次轮转
        let line = "0123456789abcde\n";
        write_lines(&mut writer, &[line, line, line, line, line, line, line]);

        assert!(path.exists(), "current file must exist");
        assert!(dir.join("easybot.log.1").exists(), ".1 must exist");
        assert!(dir.join("easybot.log.2").exists(), ".2 must exist");
        // max_files = 2：不允许出现 .3
        assert!(
            !dir.join("easybot.log.3").exists(),
            ".3 must be dropped (max_files = 2)"
        );
        // 当前文件不可能超过上限（单条 16 字节、上限 32 字节）
        let size = std::fs::metadata(&path).expect("stat current").len();
        assert!(size <= 32, "current file {size} exceeds max_bytes");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rotation_compresses_when_enabled() {
        let dir = tmp_dir("gzip");
        let path = dir.join("easybot.log");
        let mut writer = SizeRotatingWriter::new(RotationConfig {
            path: path.clone(),
            max_bytes: 32,
            max_files: 3,
            compress: true,
        })
        .expect("open writer");

        let line = "0123456789abcde\n";
        write_lines(&mut writer, &[line, line, line, line]);

        let gz = dir.join("easybot.log.1.gz");
        assert!(gz.exists(), ".1.gz must exist when compress = true");
        let mut magic = [0u8; 2];
        use std::io::Read as _;
        std::fs::File::open(&gz)
            .expect("open gz")
            .read_exact(&mut magic)
            .expect("read gzip magic");
        assert_eq!(magic, [0x1f, 0x8b], "compressed file must be gzip");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_max_bytes_never_rotates() {
        let dir = tmp_dir("norotate");
        let path = dir.join("easybot.log");
        let mut writer = SizeRotatingWriter::new(RotationConfig {
            path: path.clone(),
            max_bytes: 0,
            max_files: 3,
            compress: false,
        })
        .expect("open writer");
        let line = "0123456789abcde\n";
        write_lines(&mut writer, &[line; 8]);
        assert!(!dir.join("easybot.log.1").exists());
        assert_eq!(std::fs::metadata(&path).expect("stat").len(), 16 * 8);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn timezone_config_maps_to_timer() {
        assert_eq!(log_timer("utc"), LogTimer::Utc);
        assert_eq!(log_timer(" UTC "), LogTimer::Utc);
        assert_eq!(log_timer("local"), LogTimer::Local);
        // 未知值按 local 处理（跟随 TZ），而不是让进程失败
        assert_eq!(log_timer("Asia/Shanghai"), LogTimer::Local);
    }

    #[test]
    fn blank_rust_log_falls_back_to_config_level() {
        let kept = captured(build_filter("info", Some("   ")), || {
            tracing::info!(target: "easybot_core::test", "kept-app-log");
        });
        assert_eq!(kept, vec!["kept-app-log".to_string()]);
    }
}
