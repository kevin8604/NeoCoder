//! NeeCoder 日志系统
//!
//! 提供双输出日志（控制台 + 文件），支持自动轮转和分级过滤。
//!
//! ## 日志级别
//! - `RUST_LOG` 环境变量控制全局级别（默认 `info`）
//! - 文件日志始终记录 `debug` 及以上级别
//! - 控制台输出受 `RUST_LOG` 控制
//!
//! ## 日志文件
//! - 路径: `{app_data}/logs/neecoder.log`
//! - 轮转: 启动时归档当前日志为 `neecoder.log.{timestamp}`
//! - 保留: 最多保留 5 个历史日志文件

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MAX_LOG_FILES: usize = 5;
const LOG_FILE_NAME: &str = "neecoder.log";

/// 双输出日志器：同时写入控制台（stderr）和文件
pub struct DualLogger {
    file: Mutex<Option<File>>,
    console_level: LevelFilter,
    file_level: LevelFilter,
    log_dir: PathBuf,
}

impl DualLogger {
    /// 创建新的日志器
    ///
    /// - `log_dir`: 日志文件目录（通常为 `{app_data}/logs`）
    /// - `console_level`: 控制台最低日志级别
    fn new(log_dir: PathBuf, console_level: LevelFilter) -> Self {
        // 确保日志目录存在
        let _ = fs::create_dir_all(&log_dir);

        let log_path = log_dir.join(LOG_FILE_NAME);

        // 轮转：归档旧日志
        Self::rotate_logs(&log_dir);

        // 打开（或创建）日志文件
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();

        Self {
            file: Mutex::new(file),
            console_level,
            file_level: LevelFilter::Debug,
            log_dir,
        }
    }

    /// 归档旧日志文件，保留最近 MAX_LOG_FILES 个
    fn rotate_logs(log_dir: &Path) {
        let current = log_dir.join(LOG_FILE_NAME);
        if !current.exists() {
            return;
        }

        // 检查文件大小，太小的直接删除
        if let Ok(meta) = fs::metadata(&current) {
            if meta.len() == 0 {
                let _ = fs::remove_file(&current);
                return;
            }
        }

        // 用时间戳命名归档
        let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let archive_name = format!("neecoder.{}.log", timestamp);
        let archive_path = log_dir.join(&archive_name);
        let _ = fs::rename(&current, &archive_path);

        // 清理过多历史文件
        Self::cleanup_old_logs(log_dir);
    }

    /// 删除超出保留数量的旧日志
    fn cleanup_old_logs(log_dir: &Path) {
        let mut log_files: Vec<_> = fs::read_dir(log_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("neecoder.") && name.ends_with(".log") && name != LOG_FILE_NAME
            })
            .collect();

        // 按修改时间排序（最新在前）
        log_files.sort_by(|a, b| {
            let ta = a.metadata().and_then(|m| m.modified()).ok();
            let tb = b.metadata().and_then(|m| m.modified()).ok();
            tb.cmp(&ta)
        });

        // 删除超出的旧文件
        for old in log_files.into_iter().skip(MAX_LOG_FILES) {
            let _ = fs::remove_file(old.path());
        }
    }

    /// 获取日志目录路径
    pub fn log_dir(&self) -> &Path {
        &self.log_dir
    }
}

impl Log for DualLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.console_level || metadata.level() <= self.file_level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        let level = record.level();
        let target = record.target();
        let message = record.args();

        // 文件日志（始终写入，级别更低）
        if level <= self.file_level {
            if let Ok(mut guard) = self.file.lock() {
                if let Some(ref mut file) = *guard {
                    let _ = writeln!(
                        file,
                        "[{}] {:<5} [{}] {}",
                        timestamp, level, target, message
                    );
                }
            }
        }

        // 控制台日志
        if level <= self.console_level {
            let color = match level {
                Level::Error => "\x1b[31m", // 红
                Level::Warn => "\x1b[33m",  // 黄
                Level::Info => "\x1b[36m",  // 青
                Level::Debug => "\x1b[37m", // 白
                Level::Trace => "\x1b[90m", // 灰
            };
            let reset = "\x1b[0m";
            eprintln!(
                "{}[{}] {:<5} [{}]{} {}",
                color, timestamp, level, target, reset, message
            );
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(ref mut file) = *guard {
                let _ = file.flush();
            }
        }
    }
}

/// 初始化全局日志系统
///
/// 应在 `lib.rs` 的 `run()` 中、`tauri::Builder` 之前调用。
///
/// - `app_data_dir`: 应用数据目录（如 `~/.config/neecoder`）
pub fn init(app_data_dir: &Path) {
    let log_dir = app_data_dir.join("logs");

    // 解析 RUST_LOG 环境变量
    let console_level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|v| match v.to_lowercase().as_str() {
            "trace" => Some(LevelFilter::Trace),
            "debug" => Some(LevelFilter::Debug),
            "info" => Some(LevelFilter::Info),
            "warn" => Some(LevelFilter::Warn),
            "error" => Some(LevelFilter::Error),
            "off" => Some(LevelFilter::Off),
            _ => None,
        })
        .unwrap_or(LevelFilter::Info);

    let logger = DualLogger::new(log_dir.clone(), console_level);

    // 取两个级别中更宽松的作为全局过滤器
    let global_level = std::cmp::max(
        console_level.to_level().map(|l| l.to_level_filter()).unwrap_or(LevelFilter::Off),
        logger.file_level,
    );

    let log_path = log_dir.join(LOG_FILE_NAME);
    eprintln!(
        "[NeeCoder] Logging initialized: console={}, file={} -> {}",
        console_level,
        logger.file_level,
        log_path.display()
    );

    // 设置全局 logger（只调用一次）
    log::set_boxed_logger(Box::new(logger)).expect("Failed to set global logger");
    log::set_max_level(global_level);
}

/// 获取日志文件路径（供外部查询）
pub fn log_file_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join("logs").join(LOG_FILE_NAME)
}

/// 读取当前日志文件最后 N 行（供前端查看）
pub fn read_recent_logs(app_data_dir: &Path, lines: usize) -> String {
    let path = log_file_path(app_data_dir);
    match fs::read_to_string(&path) {
        Ok(content) => {
            let all_lines: Vec<&str> = content.lines().collect();
            let start = all_lines.len().saturating_sub(lines);
            all_lines[start..].join("\n")
        }
        Err(_) => String::new(),
    }
}
