//! 统一终端执行层。
//!
//! 供三处共用，消除重复实现：
//! - `run_terminal_command`（Agent 一次性命令）
//! - `run_terminal_session`（Agent 持久 PTY 会话）
//! - `commands::pty`（前端终端面板）
//!
//! 职责：shell 检测、PTY 创建、一次性命令执行、终端历史记录、错误定位解析。

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::sync::Mutex;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

// ── 一次性命令执行 ─────────────────────────────────────────────────────────

/// 一次性命令的执行结果（原始输出，格式化由调用方决定）
pub struct OneShotOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// 以 `cmd /C`（Windows）或 `sh -c`（Unix）执行一次性命令，带超时保护。
/// 供 run_terminal_command / run_build / run_tests 共用。
pub async fn run_one_shot(
    command: &str,
    cwd: &str,
    timeout_secs: u64,
) -> Result<OneShotOutput, String> {
    let shell = if cfg!(target_os = "windows") {
        "cmd"
    } else {
        "sh"
    };
    let flag = if cfg!(target_os = "windows") {
        "/C"
    } else {
        "-c"
    };

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(shell)
            .arg(flag)
            .arg(command)
            .current_dir(cwd)
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "Command '{}' timed out after {} seconds",
            command, timeout_secs
        )
    })?;
    let output = output.map_err(|e| format!("Failed to execute command '{}': {}", command, e))?;

    Ok(OneShotOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

// ── 终端历史记录 ────────────────────────────────────────────────────────────

struct TerminalEntry {
    command: String,
    output: String,
    exit_code: i32,
}

static TERMINAL_HISTORY: std::sync::LazyLock<Mutex<VecDeque<TerminalEntry>>> =
    std::sync::LazyLock::new(|| Mutex::new(VecDeque::new()));

/// 记录一条命令执行历史（最多 10 条，供 @error/@terminal 特性使用）
pub fn push_terminal_entry(command: &str, output: &str, exit_code: i32) {
    if let Ok(mut history) = TERMINAL_HISTORY.lock() {
        if history.len() >= 10 {
            history.pop_front();
        }
        history.push_back(TerminalEntry {
            command: command.to_string(),
            output: output.to_string(),
            exit_code,
        });
    }
}

pub fn get_recent_terminal(n: usize) -> Vec<(String, String, i32)> {
    let history = TERMINAL_HISTORY.lock().ok();
    match history {
        Some(h) => h
            .iter()
            .rev()
            .take(n)
            .map(|e| (e.command.clone(), e.output.clone(), e.exit_code))
            .collect(),
        None => Vec::new(),
    }
}

pub fn get_error_summary() -> String {
    let history = TERMINAL_HISTORY.lock().ok();
    match history {
        Some(h) => {
            let mut summary = String::new();
            for entry in h.iter().rev().take(5) {
                if entry.exit_code != 0
                    || entry.output.to_lowercase().contains("error")
                    || entry.output.to_lowercase().contains("fail")
                {
                    summary.push_str(&format!(
                        "$ {}\nExit: {}\n{}\n\n",
                        entry.command,
                        entry.exit_code,
                        if entry.output.len() > 2000 {
                            crate::agent::utils::safe_truncate(&entry.output, 2000)
                        } else {
                            &entry.output
                        }
                    ));
                }
            }
            if summary.is_empty() {
                "No recent errors found.".to_string()
            } else {
                summary
            }
        }
        None => "Terminal history unavailable.".to_string(),
    }
}

// ── 错误定位解析 ────────────────────────────────────────────────────────────

/// Parse common compiler/linter error patterns and extract file:line references
pub fn parse_error_locations(stderr: &str, stdout: &str) -> String {
    let combined = format!("{}\n{}", stdout, stderr);
    let mut errors: Vec<String> = Vec::new();

    for line in combined.lines() {
        let lower = line.to_lowercase();

        // Rust: error[E0308]: src/main.rs:42:5 or error: src/main.rs:42
        if lower.contains("error[") || (lower.starts_with("error") && lower.contains(".rs:")) {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // TypeScript: src/foo.ts(12,5): error TS2322
        if lower.contains(".ts(") && lower.contains("error ts") {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // JavaScript: src/foo.js:12:5
        if (lower.contains(".js:") || lower.contains(".jsx:"))
            && (lower.contains("error") || lower.contains("syntaxerror"))
        {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // Python: File "foo.py", line 23
        if lower.contains("file \"") && lower.contains("line ") {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // Go: ./main.go:15:2:
        if lower.contains(".go:")
            && (lower.contains("undefined") || lower.contains("cannot") || lower.contains("syntax"))
        {
            errors.push(format!("  {}", line.trim()));
            continue;
        }
        // Generic: error in file.ext:LINE
        if lower.contains("error") && (line.contains(": ") || line.contains(" at ")) {
            // Only include if it looks like it has a file path
            if line.contains('/') || line.contains('\\') || line.contains('.') {
                errors.push(format!("  {}", line.trim()));
            }
        }
    }

    if errors.is_empty() {
        return String::new();
    }

    // Deduplicate
    errors.sort();
    errors.dedup();
    let count = errors.len();
    let max_show = if errors.len() > 10 { 10 } else { errors.len() };

    let mut result = format!("\n--- Error Summary ({} found) ---\n", count);
    for e in errors.iter().take(max_show) {
        result.push_str(e);
        result.push('\n');
    }
    if count > max_show {
        result.push_str(&format!("  ... and {} more\n", count - max_show));
    }
    result
}

// ── PTY 创建 ────────────────────────────────────────────────────────────────

/// 检测系统默认 shell，返回 (shell 路径, shell 风味)
/// 风味用于完成标记语法：cmd / powershell / unix
pub fn detect_shell() -> (String, String) {
    #[cfg(target_os = "windows")]
    {
        let has_pwsh = std::process::Command::new("pwsh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if has_pwsh {
            ("pwsh".to_string(), "powershell".to_string())
        } else {
            let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd".to_string());
            let flavor = if shell.to_lowercase().ends_with("cmd.exe") || shell == "cmd" {
                "cmd".to_string()
            } else {
                "powershell".to_string()
            };
            (shell, flavor)
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "bash".to_string());
        let flavor = if shell.ends_with("zsh") {
            "zsh"
        } else {
            "unix"
        }
        .to_string();
        (shell, flavor)
    }
}

/// PTY 句柄集合：master（resize）、reader/writer（I/O）、child（进程控制）
pub struct PtyHandles {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send>,
}

/// 创建交互式 PTY shell 会话。
/// 供 run_terminal_session（Agent）与 commands::pty（前端面板）共用。
pub fn spawn_pty(shell: &str, rows: u16, cols: u16) -> Result<PtyHandles, String> {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| format!("Failed to open PTY: {}", e))?;

    let mut cmd = CommandBuilder::new(shell);
    if cfg!(target_os = "windows") {
        if shell.to_lowercase().ends_with("cmd.exe") || shell == "cmd" {
            cmd.arg("/Q");
        }
    } else {
        cmd.arg("-i");
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("Failed to spawn shell '{}': {}", shell, e))?;

    let reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
    let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

    Ok(PtyHandles {
        master: pair.master,
        reader,
        writer,
        child,
    })
}

/// 根据 shell 风味生成命令完成标记（echo 返回码），供持久会话检测命令结束
pub fn completion_marker_cmd(flavor: &str) -> String {
    match flavor {
        "cmd" => "echo __NEOCODER_DONE_%NEOCODER_MID%__%errorlevel%",
        "powershell" => "echo __NEOCODER_DONE_$env:NEOCODER_MID__$LASTEXITCODE",
        _ => "echo __NEOCODER_DONE_$NEOCODER_MID__$?",
    }
    .to_string()
}
