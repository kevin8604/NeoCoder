//! 统一文件操作服务层。
//!
//! 供 Tauri 命令层（`commands/project.rs`）与 Agent 工具（`agent/tools/*`）共用，
//! 消除两套独立实现。沙箱检查在 `sandbox` 参数为 `Some` 时执行
//! （Agent 工具传入沙箱）；前端命令无沙箱上下文时传 `None`（跳过检查）。
//! 所有函数返回 `Result<T, String>`，上层（命令/工具）自行决定格式化方式。

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::sandbox::SandboxChecker;

pub struct FileService;

impl FileService {
    /// 解析目标路径：相对路径相对于 project_path，绝对路径原样返回
    pub fn resolve(base: Option<&str>, raw: &str) -> PathBuf {
        crate::agent::utils::resolve_path(base, raw)
    }

    /// 读取文本文件（带沙箱读权限 + 文件大小检查）
    pub fn read_text(
        path: &Path,
        project: Option<&str>,
        sandbox: Option<&SandboxChecker>,
    ) -> Result<String, String> {
        if let Some(sb) = sandbox {
            sb.check_path(path, project, false)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
            sb.check_file_size(path).map_err(|e| e.to_string())?;
        }
        if !path.exists() {
            return Err(format!("File not found: {}", path.display()));
        }
        std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))
    }

    /// 写入文本文件（自动创建父目录；create_only 时拒绝覆盖）
    pub fn write_text(
        path: &Path,
        content: &str,
        project: Option<&str>,
        sandbox: Option<&SandboxChecker>,
        create_only: bool,
    ) -> Result<(), String> {
        if let Some(sb) = sandbox {
            sb.check_path(path, project, true)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
        }
        if path.exists() && create_only {
            return Err(format!(
                "File {} already exists and create_only is true",
                path.display()
            ));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dirs: {}", e))?;
        }
        std::fs::write(path, content)
            .map_err(|e| format!("Failed to write {}: {}", path.display(), e))
    }

    /// 追加文本到文件末尾（文件不存在时创建）
    pub fn append_text(
        path: &Path,
        content: &str,
        project: Option<&str>,
        sandbox: Option<&SandboxChecker>,
    ) -> Result<(), String> {
        if let Some(sb) = sandbox {
            sb.check_path(path, project, true)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dirs: {}", e))?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| format!("Failed to open {}: {}", path.display(), e))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("Failed to append to {}: {}", path.display(), e))
    }

    /// 删除文件或目录（目录递归删除），与 commands::delete_file 语义一致
    pub fn remove(
        path: &Path,
        project: Option<&str>,
        sandbox: Option<&SandboxChecker>,
    ) -> Result<(), String> {
        if let Some(sb) = sandbox {
            sb.check_path(path, project, true)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
        }
        if !path.exists() {
            return Err(format!("Path not found: {}", path.display()));
        }
        if path.is_dir() {
            std::fs::remove_dir_all(path)
                .map_err(|e| format!("Failed to delete directory {}: {}", path.display(), e))
        } else {
            std::fs::remove_file(path)
                .map_err(|e| format!("Failed to delete file {}: {}", path.display(), e))
        }
    }

    /// 创建目录（含父目录）
    pub fn create_dir_all(
        path: &Path,
        project: Option<&str>,
        sandbox: Option<&SandboxChecker>,
    ) -> Result<(), String> {
        if let Some(sb) = sandbox {
            sb.check_path(path, project, true)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
        }
        std::fs::create_dir_all(path)
            .map_err(|e| format!("Failed to create directory {}: {}", path.display(), e))
    }

    /// 重命名/移动文件或目录（自动创建目标父目录）
    pub fn rename(
        source: &Path,
        destination: &Path,
        project: Option<&str>,
        sandbox: Option<&SandboxChecker>,
    ) -> Result<(), String> {
        if let Some(sb) = sandbox {
            sb.check_path(source, project, true)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
            sb.check_path(destination, project, true)
                .map_err(|e| format!("Sandbox blocked: {}", e))?;
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create parent dirs: {}", e))?;
        }
        std::fs::rename(source, destination).map_err(|e| {
            format!(
                "Failed to rename {} to {}: {}",
                source.display(),
                destination.display(),
                e
            )
        })
    }
}
