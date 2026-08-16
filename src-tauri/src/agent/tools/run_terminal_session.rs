//! run_terminal_session: persistent PTY shell session tool for the Agent.
//!
//! Unlike run_terminal_command (one-shot process per call), this tool keeps a
//! shell alive across calls — `cd`, environment variables, and activated
//! virtualenvs persist between invocations. Each `session_id` maps to one
//! shell process; pass `reset: true` to kill and restart it.
//!
//! Implementation notes:
//! - Uses `portable-pty` (same backend as the frontend terminal panel) but a
//!   dedicated per-session instance, so it never interferes with the UI.
//! - Completion detection uses a unique echo marker with exit code:
//!   `echo __NEOCODER_DONE_<id>__<exitcode>` (syntax adapted per shell).
//! - Reads on a background thread and bridges to async via mpsc, so the
//!   tokio executor is never blocked by PTY I/O.

use super::{Tool, ToolContext};
use crate::terminal::{completion_marker_cmd, detect_shell, spawn_pty};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, LazyLock, Mutex};

/// A live agent shell session.
pub struct AgentPtySession {
    reader: Arc<Mutex<Box<dyn Read + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
    /// Shell flavor used to emit the completion marker (cmd / powershell / unix)
    marker_cmd: String,
    cwd: Arc<Mutex<String>>,
}

static AGENT_SESSIONS: LazyLock<Mutex<HashMap<String, Arc<AgentPtySession>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Number of agent PTY sessions kept alive at most (LRU-evict oldest on overflow).
const MAX_SESSIONS: usize = 4;

fn spawn_session() -> Result<Arc<AgentPtySession>, String> {
    let (shell, flavor) = detect_shell();
    let handles = spawn_pty(&shell, 24, 120)?;
    let marker_cmd = completion_marker_cmd(&flavor);

    Ok(Arc::new(AgentPtySession {
        reader: Arc::new(Mutex::new(handles.reader)),
        writer: Arc::new(Mutex::new(handles.writer)),
        child: Arc::new(Mutex::new(handles.child)),
        marker_cmd,
        cwd: Arc::new(Mutex::new(String::new())),
    }))
}

fn get_or_create_session(session_id: &str) -> Result<Arc<AgentPtySession>, String> {
    let mut sessions = AGENT_SESSIONS.lock().map_err(|e| e.to_string())?;
    if let Some(s) = sessions.get(session_id) {
        // Session still alive?
        if let Ok(mut child) = s.child.lock() {
            match child.try_wait() {
                Ok(None) => return Ok(s.clone()),
                _ => { /* exited — fall through to respawn */ }
            }
        }
        sessions.remove(session_id);
    }

    // Evict oldest when at capacity (FIFO)
    while sessions.len() >= MAX_SESSIONS {
        if let Some(oldest) = sessions.keys().next().cloned() {
            sessions.remove(&oldest);
        } else {
            break;
        }
    }

    let session = spawn_session()?;
    sessions.insert(session_id.to_string(), session.clone());
    Ok(session)
}

/// Read all PTY output until the completion marker appears (or timeout).
async fn read_until_marker(
    session: &AgentPtySession,
    mid: &str,
    timeout_secs: u64,
) -> (String, Option<i32>) {
    let marker = format!("__NEOCODER_DONE_{}__", mid);

    // Bridge blocking PTY reads to async via mpsc on a background thread
    let reader = session.reader.clone();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(128);
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.lock().map(|mut r| r.read(&mut buf)) {
                Ok(Ok(0)) | Ok(Err(_)) | Err(_) => break,
                Ok(Ok(n)) => {
                    if tx.blocking_send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut collected = String::new();
    let mut exit_code: Option<i32> = None;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let chunk = tokio::time::timeout(remaining, rx.recv()).await;
        match chunk {
            Ok(Some(bytes)) => {
                collected.push_str(&String::from_utf8_lossy(&bytes));
                // Look for the marker anywhere in the accumulated output
                if let Some(pos) = collected.find(&marker) {
                    // Extract exit code after the marker (digits until non-digit)
                    let after = &collected[pos + marker.len()..];
                    let code_str: String = after
                        .chars()
                        .take_while(|c| c.is_ascii_digit() || *c == '-')
                        .collect();
                    if let Ok(code) = code_str.parse::<i32>() {
                        exit_code = Some(code);
                    }
                    // Trim output after marker (keep a tiny tail for safety)
                    let end = (pos + marker.len() + code_str.len() + 1).min(collected.len());
                    collected.truncate(end);
                    break;
                }
            }
            Ok(None) => break, // reader thread ended (session exited)
            Err(_) => break,   // deadline
        }
    }

    (collected, exit_code)
}

/// Write a command to the PTY followed by the completion marker command.
fn send_command(session: &AgentPtySession, mid: &str, command: &str) -> Result<(), String> {
    let mut writer = session.writer.lock().map_err(|e| e.to_string())?;
    let payload = if session.marker_cmd.contains("%NEOCODER_MID%") {
        format!("set NEOCODER_MID={}&{}&{}", mid, command, session.marker_cmd)
    } else if session.marker_cmd.contains("$env:NEOCODER_MID") {
        format!("$env:NEOCODER_MID='{}'; {}; {}", mid, command, session.marker_cmd)
    } else {
        format!("export NEOCODER_MID={}; {}; {}", mid, command, session.marker_cmd)
    };
    writer
        .write_all(payload.as_bytes())
        .map_err(|e| e.to_string())?;
    writer.write_all(b"\r\n").map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

pub struct RunTerminalSession;

#[async_trait::async_trait]
impl Tool for RunTerminalSession {
    fn name(&self) -> &str {
        "run_terminal_session"
    }

    async fn execute(&self, args: serde_json::Value, ctx: &ToolContext) -> String {
        let command = args["command"].as_str().unwrap_or("");
        if command.trim().is_empty() {
            return "Error: command is required".to_string();
        }

        // Sandbox safety check (same policy as run_terminal_command)
        if let Err(reason) = ctx.sandbox.check_command(command) {
            log::warn!("Blocked dangerous command '{}': {}", command, reason);
            return format!("Error: Command blocked for safety: {}. If you believe this is a false positive, ask the user to run it manually.", reason);
        }

        let session_id = args["session_id"].as_str().unwrap_or("default");
        let reset = args["reset"].as_bool().unwrap_or(false);
        let timeout = args["timeout_seconds"].as_u64().unwrap_or(30).clamp(5, 300);

        if reset {
            if let Ok(mut sessions) = AGENT_SESSIONS.lock() {
                if let Some(s) = sessions.remove(session_id) {
                    if let Ok(mut child) = s.child.lock() {
                        let _ = child.kill();
                    }
                    log::info!("[TermSession] Reset session '{}'", session_id);
                }
            }
        }

        let session = match get_or_create_session(session_id) {
            Ok(s) => s,
            Err(e) => return format!("Error: failed to start terminal session: {}", e),
        };

        let mid = format!("{:x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0));

        if let Err(e) = send_command(&session, &mid, command) {
            return format!("Error: failed to write to terminal session: {}", e);
        }

        // Read output until marker / timeout
        let (raw_output, exit_code) = read_until_marker(&session, &mid, timeout).await;

        // Post-process: strip command echo (first line) and the marker line
        let mut lines: Vec<&str> = raw_output.split('\n').collect();
        // First line is usually the echoed command itself
        if !lines.is_empty() {
            lines.remove(0);
        }
        // Drop the marker line (contains __NEOCODER_DONE_)
        lines.retain(|l| !l.contains("__NEOCODER_DONE_") && !l.trim().is_empty());

        // Detect exit status via marker code or keywords
        let status = match exit_code {
            Some(0) => "SUCCESS".to_string(),
            Some(c) => format!("FAILED (exit {})", c),
            None => "UNKNOWN (timed out or session exited)".to_string(),
        };

        let mut result = String::new();
        result.push_str(&format!("$ {} [session '{}']\n", command, session_id));
        result.push_str(&format!("Status: {}\n", status));
        if !lines.is_empty() {
            result.push_str("\n");
            let joined = lines.join("\n");
            const MAX_OUTPUT: usize = 50 * 1024;
            if joined.len() > MAX_OUTPUT {
                result.push_str(&joined[..MAX_OUTPUT]);
                result.push_str("\n... (output truncated at 50KB)");
            } else {
                result.push_str(&joined);
            }
        } else if exit_code == Some(0) {
            result.push_str("(no output)");
        }

        // Remember the working directory hint for the next call (best effort)
        let cwd_hint = command.trim();
        if cwd_hint.starts_with("cd ") || cwd_hint.starts_with("Set-Location") {
            if let Ok(mut cwd) = session.cwd.lock() {
                *cwd = cwd_hint[3..].trim_matches(|c| c == '"' || c == '\'').to_string();
            }
        }

        result
    }
}
