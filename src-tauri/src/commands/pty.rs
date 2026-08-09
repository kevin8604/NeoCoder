//! Terminal PTY backend — manages a shell process with true PTY support
//!
//! Architecture:
//! - Uses `portable-pty` crate for cross-platform PTY support
//! - Frontend → Backend: "pty-input" event → write to PTY master
//! - Backend → Frontend: read PTY slave → "pty-output" event
//! - Backend → Frontend: process exit → "pty-exit" event
//!
//! Benefits over piped stdio:
//! - Can read prompts (no line buffering)
//! - Supports terminal resize (SIGWINCH)
//! - Proper terminal emulation

use crate::terminal::{detect_shell, spawn_pty};
use portable_pty::PtySize;
use std::sync::Arc;
use std::sync::Mutex;
use std::io::{Read, Write};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::Mutex as TokioMutex;

/// Holds the PTY handles for bidirectional I/O
pub struct PtyState {
    /// The PTY master (for resize)
    pub pty_master: Arc<TokioMutex<Option<Box<dyn portable_pty::MasterPty + Send>>>>,
    /// The child process
    pub child: Arc<TokioMutex<Option<Box<dyn portable_pty::Child + Send>>>>,
    /// Shell command
    pub shell_cmd: Arc<Mutex<String>>,
    /// PTY size
    pub size: Arc<TokioMutex<PtySize>>,
}

impl PtyState {
    pub fn new() -> Self {
        Self {
            pty_master: Arc::new(TokioMutex::new(None)),
            child: Arc::new(TokioMutex::new(None)),
            shell_cmd: Arc::new(Mutex::new(detect_shell().0)),
            size: Arc::new(TokioMutex::new(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })),
        }
    }
}

/// Spawn a new shell process with true PTY support.
/// Emits "pty-output" for stdout/stderr and "pty-exit" on termination.
#[tauri::command]
pub async fn start_terminal(
    app: AppHandle,
    state: State<'_, PtyState>,
) -> Result<(), String> {
    // Kill previous terminal if running
    stop_terminal_inner(&state).await;

    let shell = {
        let lock = state.shell_cmd.lock().map_err(|e| e.to_string())?;
        lock.clone()
    };

    let size = {
        let lock = state.size.lock().await;
        *lock
    };

    // Create a new PTY pair (shared spawn logic with agent sessions)
    let handles = spawn_pty(&shell, size.rows, size.cols)?;

    // Store the PTY master and child
    {
        let mut master_lock = state.pty_master.lock().await;
        *master_lock = Some(handles.master);
    }
    {
        let mut child_lock = state.child.lock().await;
        *child_lock = Some(handles.child);
    }

    // Store the reader in app state
    app.manage(PtyReader {
        reader: Arc::new(TokioMutex::new(handles.reader)),
    });

    // Store the writer in app state
    app.manage(PtyWriter {
        writer: Arc::new(TokioMutex::new(handles.writer)),
    });

    let app_clone = app.clone();

    // Read from PTY in background task
    tauri::async_runtime::spawn(async move {
        let reader_state = app_clone.state::<PtyReader>();
        let mut reader_lock = reader_state.reader.lock().await;
        let mut buf = [0u8; 4096];
        
        loop {
            match reader_lock.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    let _ = app_clone.emit("pty-exit", "Process terminated");
                    break;
                }
                Ok(n) => {
                    // Convert bytes to string (lossy for non-UTF8)
                    let output = String::from_utf8_lossy(&buf[..n]);
                    let _ = app_clone.emit("pty-output", output.as_ref());
                }
                Err(e) => {
                    let _ = app_clone.emit("pty-error", format!("PTY read error: {}", e));
                    break;
                }
            }
        }
    });

    log::info!("Terminal started with true PTY: shell={}", shell);
    Ok(())
}

/// Shared PTY reader
pub struct PtyReader {
    pub reader: Arc<TokioMutex<Box<dyn Read + Send>>>,
}

/// Shared PTY writer
pub struct PtyWriter {
    pub writer: Arc<TokioMutex<Box<dyn Write + Send>>>,
}

/// Write input to the PTY
#[tauri::command]
pub async fn write_stdin(
    app: AppHandle,
    state: State<'_, PtyState>,
    data: String,
) -> Result<(), String> {
    // Check if child process is alive
    {
        let mut child_lock = state.child.lock().await;
        if let Some(ref mut child) = *child_lock {
            match child.try_wait() {
                Ok(Some(_)) => {
                    return Err("Terminal process already exited".to_string());
                }
                Ok(None) => {} // Still running
                Err(e) => {
                    return Err(format!("Failed to check process status: {}", e));
                }
            }
        } else {
            return Err("No terminal process running".to_string());
        }
    }

    // Get writer from app state
    match app.try_state::<PtyWriter>() {
        Some(writer_state) => {
            let mut writer = writer_state.writer.lock().await;
            writer.write_all(data.as_bytes()).map_err(|e| {
                format!("Failed to write to PTY: {}", e)
            })?;
            writer.flush().map_err(|e| {
                format!("Failed to flush PTY: {}", e)
            })?;
            Ok(())
        }
        None => Err("No PTY writer available. Start terminal first.".to_string()),
    }
}

/// Kill the shell process
#[tauri::command]
pub async fn stop_terminal(app: AppHandle, state: State<'_, PtyState>) -> Result<(), String> {
    stop_terminal_inner(&state).await;
    let _ = app.emit("pty-exit", "Terminal stopped by user");
    Ok(())
}

async fn stop_terminal_inner(state: &State<'_, PtyState>) {
    let mut child_lock = state.child.lock().await;
    if let Some(mut child) = child_lock.take() {
        let _ = child.kill();
        log::info!("Terminal process killed");
    }
    
    let mut master_lock = state.pty_master.lock().await;
    *master_lock = None;
}

/// Resize the terminal (cols, rows) — sends SIGWINCH on Unix
#[tauri::command]
pub async fn resize_terminal(
    _app: AppHandle,
    state: State<'_, PtyState>,
    cols: u32,
    rows: u32,
) -> Result<(), String> {
    let new_size = PtySize {
        rows: rows as u16,
        cols: cols as u16,
        pixel_width: 0,
        pixel_height: 0,
    };

    // Update stored size
    {
        let mut size_lock = state.size.lock().await;
        *size_lock = new_size;
    }

    // Resize the PTY
    let master_lock = state.pty_master.lock().await;
    if let Some(ref master) = *master_lock {
        master
            .resize(new_size)
            .map_err(|e| format!("Failed to resize PTY: {}", e))?;
        log::debug!("Terminal resized to {}x{}", cols, rows);
    }

    Ok(())
}
