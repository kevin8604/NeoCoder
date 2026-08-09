//! 统一事件记录管道（EventBus）。
//!
//! 消除三套各自为政的 JSONL 写入实现：
//! - `memory::agent_log`（会话 JSONL 日志）
//! - `telemetry`（使用统计 JSONL）
//! - `logging`（文本日志，保留标准 log crate 语义）
//!
//! 职责划分：
//! - `JsonlAppender`：线程安全的 JSONL 文件追加核心（建目录、追加一行、flush）。
//!   所有结构化事件文件（agent log / telemetry）共用此实现。
//! - `EventBus`：应用级全局事件总线，按 `category` 注册/获取 appender，
//!   提供统一的 `emit` 入口，供任意模块记录结构化事件。
//! - 文本日志仍走标准 `log` crate（`logging` 模块），EventBus 只承载结构化 JSONL 事件。

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// 线程安全的 JSONL 文件追加器。
///
/// - 打开失败不致命：内部持有 `Option<File>`，不可用时 `append` 返回错误。
/// - 每行一条 JSON，写后立即 `flush`（兼顾崩溃恢复与实时可读）。
pub struct JsonlAppender {
    file: Mutex<Option<File>>,
    path: PathBuf,
}

impl JsonlAppender {
    /// 打开（或创建）`{dir}/{file_name}` 追加写入器，自动创建目录。
    pub fn open(dir: &Path, file_name: &str) -> Self {
        let path = dir.join(file_name);
        if let Err(e) = fs::create_dir_all(dir) {
            log::warn!("[EventBus] Failed to create dir '{}': {}", dir.display(), e);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| log::warn!("[EventBus] Failed to open '{}': {}", path.display(), e))
            .ok();
        Self {
            file: Mutex::new(file),
            path,
        }
    }

    /// 追加一条 JSON 记录并 flush 到磁盘。
    pub fn append<T: serde::Serialize>(&self, value: &T) -> Result<(), String> {
        let mut guard = self.file.lock().map_err(|e| e.to_string())?;
        let file = guard
            .as_mut()
            .ok_or_else(|| format!("JSONL appender not open: {}", self.path.display()))?;
        let line = serde_json::to_string(value)
            .map_err(|e| format!("Failed to serialize event: {}", e))?;
        writeln!(file, "{}", line)
            .map_err(|e| format!("Failed to write event line: {}", e))?;
        file.flush()
            .map_err(|e| format!("Failed to flush event file: {}", e))?;
        Ok(())
    }

    /// 当前日志文件路径。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// 应用级事件总线：按 category 管理多个 JSONL appender。
///
/// 初始化的服务（如 TelemetryCollector）通过 `register` 注册自己的文件，
/// 任意模块可通过 `emit(category, event)` 记录结构化事件。
pub struct EventBus {
    appenders: Mutex<HashMap<String, Arc<JsonlAppender>>>,
}

static GLOBAL: LazyLock<EventBus> = LazyLock::new(|| EventBus {
    appenders: Mutex::new(HashMap::new()),
});

impl EventBus {
    /// 获取全局事件总线单例。
    pub fn global() -> &'static EventBus {
        &GLOBAL
    }

    /// 注册（或替换）一个 category 的 appender，返回可共享的句柄。
    pub fn register(&self, category: &str, dir: &Path, file_name: &str) -> Arc<JsonlAppender> {
        let appender = Arc::new(JsonlAppender::open(dir, file_name));
        if let Ok(mut map) = self.appenders.lock() {
            map.insert(category.to_string(), appender.clone());
        }
        appender
    }

    /// 获取已注册 category 的 appender。
    pub fn appender(&self, category: &str) -> Option<Arc<JsonlAppender>> {
        self.appenders.lock().ok()?.get(category).cloned()
    }

    /// 向指定 category 发射一条结构化事件。
    pub fn emit<T: serde::Serialize>(&self, category: &str, value: &T) -> Result<(), String> {
        match self.appender(category) {
            Some(a) => a.append(value),
            None => Err(format!(
                "EventBus: no appender registered for category '{}'",
                category
            )),
        }
    }
}

/// 便捷全局函数：向指定 category 发射一条结构化事件。
pub fn emit<T: serde::Serialize>(category: &str, value: &T) -> Result<(), String> {
    EventBus::global().emit(category, value)
}
