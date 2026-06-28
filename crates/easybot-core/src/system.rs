//! 系统信息采集
//!
//! 使用 `sysinfo` crate 获取服务器软硬件信息及性能负载。
//! 所有字段在 Windows 上可能不可用，调用方检查 `Option` 值。

use std::time::Duration;
use sysinfo::{Disks, ProcessesToUpdate, System};

/// 系统信息快照
#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemInfo {
    pub os: OsInfo,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disk: DiskInfo,
    pub process: ProcessInfo,
    pub binary: BinaryInfo,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OsInfo {
    pub name: String,
    pub version: String,
    pub hostname: String,
    pub kernel: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CpuInfo {
    pub brand: String,
    pub cores: usize,
    /// CPU 使用率百分比 (0.0–100.0)
    pub usage: f64,
    /// 1 分钟负载均值（Windows 不可用 = None）
    pub load_avg_1: Option<f64>,
    pub load_avg_5: Option<f64>,
    pub load_avg_15: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MemoryInfo {
    pub total_gb: f64,
    pub used_gb: f64,
    /// 内存使用百分比 (0.0–100.0)
    pub percent: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiskInfo {
    pub total_gb: f64,
    pub used_gb: f64,
    /// 磁盘使用百分比 (0.0–100.0)
    pub percent: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProcessInfo {
    /// 进程内存占用（MB）
    pub memory_mb: f64,
    /// 进程 CPU 占用百分比
    pub cpu_percent: f64,
    /// 进程运行时长（秒）
    pub uptime: u64,
    /// 进程启动时间戳（Unix 秒）
    pub start_time: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BinaryInfo {
    pub profile: String,
}

/// 采集系统信息（首次调用约耗时 100-200ms）
pub fn collect_system_info() -> SystemInfo {
    // sysinfo 0.33 API:
    //   - System::new() creates a System without refreshing anything
    //   - call refresh_*() methods to populate data
    let mut sys = System::new();

    // CPU
    sys.refresh_cpu_all();
    let cpu_cores = sys.physical_core_count().unwrap_or(sys.cpus().len());
    let cpu_brand = if !sys.cpus().is_empty() {
        sys.cpus()[0].brand().to_string()
    } else {
        "Unknown".to_string()
    };
    let cpu_usage = sys.global_cpu_usage() as f64;

    // 负载均值（Windows 不可用）
    let load_avg = System::load_average();
    let load_avg_1 = if load_avg.one > 0.0 {
        Some(load_avg.one)
    } else {
        None
    };
    let load_avg_5 = if load_avg.five > 0.0 {
        Some(load_avg.five)
    } else {
        None
    };
    let load_avg_15 = if load_avg.fifteen > 0.0 {
        Some(load_avg.fifteen)
    } else {
        None
    };

    // 内存
    sys.refresh_memory();
    let total_mem = sys.total_memory();
    let used_mem = sys.used_memory();
    let mem_percent = if total_mem > 0 {
        (used_mem as f64 / total_mem as f64) * 100.0
    } else {
        0.0
    };

    // 磁盘：取容量最大的分区（即主磁盘，自动跳过 /proc 等虚拟分区且避免 APFS 重复计数）
    let disks = Disks::new_with_refreshed_list();
    let (disk_total, disk_used) = disks
        .iter()
        .max_by_key(|d| d.total_space())
        .map(|d| {
            let used = d.total_space().saturating_sub(d.available_space());
            (d.total_space(), used)
        })
        .unwrap_or((0, 0));
    let disk_percent = if disk_total > 0 {
        (disk_used as f64 / disk_total as f64) * 100.0
    } else {
        0.0
    };

    // 进程信息
    let pid = sysinfo::get_current_pid().ok();
    let (proc_mem, proc_cpu, proc_uptime, proc_start) = if let Some(pid) = pid {
        sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);
        if let Some(process) = sys.process(pid) {
            let mem = process.memory() as f64 / (1024.0 * 1024.0);
            let cpu = process.cpu_usage() as f64;
            let run_time = process.run_time();
            let start_time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                - run_time;
            (mem, cpu, run_time, start_time)
        } else {
            (0.0, 0.0, 0, 0)
        }
    } else {
        (0.0, 0.0, 0, 0)
    };

    SystemInfo {
        os: OsInfo {
            name: System::name().unwrap_or_else(|| "Unknown".into()),
            version: System::long_os_version().unwrap_or_else(|| "Unknown".into()),
            hostname: System::host_name().unwrap_or_else(|| "Unknown".into()),
            kernel: System::kernel_version(),
        },
        cpu: CpuInfo {
            brand: cpu_brand,
            cores: cpu_cores,
            usage: cpu_usage,
            load_avg_1,
            load_avg_5,
            load_avg_15,
        },
        memory: MemoryInfo {
            total_gb: (total_mem as f64) / (1024.0 * 1024.0 * 1024.0),
            used_gb: (used_mem as f64) / (1024.0 * 1024.0 * 1024.0),
            percent: mem_percent,
        },
        disk: DiskInfo {
            total_gb: (disk_total as f64) / (1024.0 * 1024.0 * 1024.0),
            used_gb: (disk_used as f64) / (1024.0 * 1024.0 * 1024.0),
            percent: disk_percent,
        },
        process: ProcessInfo {
            memory_mb: proc_mem,
            cpu_percent: proc_cpu,
            uptime: Duration::from_secs(proc_uptime).as_secs(),
            start_time: proc_start,
        },
        binary: BinaryInfo {
            profile: if cfg!(debug_assertions) {
                "debug".into()
            } else {
                "release".into()
            },
        },
    }
}
