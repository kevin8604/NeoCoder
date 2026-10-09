//! Tauri commands exposing the DAP debug session to the frontend DebugPanel.
//!
//! All commands are thin wrappers over [`crate::dap::DapManager`]. A debug
//! session is identified by a client-supplied `session_id` (the frontend reuses
//! the active workspace id). Streaming state (breakpoint hits, stops) is pushed
//! back over the `dap-event` channel.

use crate::dap::{
    DapBreakpoint, DapManager, DapStackFrame, DapVariable, DebugSessionInfo, RuntimeSnapshot,
};
use serde_json::Value;
use std::sync::Arc;
use tauri::{Emitter, State};

fn emit(app: &tauri::AppHandle, payload: Value) {
    let _ = app.emit("dap-event", payload);
}

#[tauri::command]
pub async fn start_debug_session(
    app: tauri::AppHandle,
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
    language: String,
    program: String,
    args: Option<Vec<String>>,
    cwd: Option<String>,
    adapter: Option<String>,
    extra_launch: Option<Value>,
) -> Result<DebugSessionInfo, String> {
    let info = manager
        .start_session(
            &session_id,
            &language,
            &program,
            args.unwrap_or_default(),
            cwd,
            adapter.as_deref(),
            extra_launch,
        )
        .await?;
    emit(
        &app,
        serde_json::json!({ "type": "started", "sessionId": session_id, "info": info }),
    );
    Ok(info)
}

#[tauri::command]
pub async fn set_debug_breakpoints(
    app: tauri::AppHandle,
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
    path: String,
    lines: Vec<u32>,
) -> Result<Vec<DapBreakpoint>, String> {
    let bps = manager.set_breakpoints(&session_id, &path, lines).await?;
    emit(
        &app,
        serde_json::json!({ "type": "breakpoints", "sessionId": session_id, "breakpoints": bps }),
    );
    Ok(bps)
}

#[tauri::command]
pub async fn configuration_done(
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
) -> Result<(), String> {
    manager.configuration_done(&session_id).await
}

/// Resume execution (`continue` / `next` / `stepIn` / `stepOut`) and return the
/// runtime snapshot captured at the resulting stop.
#[tauri::command]
pub async fn resume_debug(
    app: tauri::AppHandle,
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
    kind: String,
    timeout_secs: Option<u64>,
) -> Result<RuntimeSnapshot, String> {
    let snap = manager
        .resume(&session_id, &kind, timeout_secs.unwrap_or(15))
        .await?;
    emit(
        &app,
        serde_json::json!({ "type": "stopped", "sessionId": session_id, "snapshot": snap }),
    );
    Ok(snap)
}

#[tauri::command]
pub async fn get_stack_trace(
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
) -> Result<Vec<DapStackFrame>, String> {
    manager.snapshot(&session_id).await
}

#[tauri::command]
pub async fn get_locals(
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
) -> Result<Vec<DapVariable>, String> {
    manager.locals(&session_id).await
}

#[tauri::command]
pub async fn evaluate_expression(
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
    expression: String,
) -> Result<String, String> {
    manager.evaluate(&session_id, &expression).await
}

#[tauri::command]
pub async fn get_debug_session_info(
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
) -> Result<DebugSessionInfo, String> {
    manager.session_info(&session_id).await
}

#[tauri::command]
pub async fn stop_debug_session(
    app: tauri::AppHandle,
    manager: State<'_, Arc<DapManager>>,
    session_id: String,
) -> Result<(), String> {
    manager.stop_session(&session_id).await?;
    emit(
        &app,
        serde_json::json!({ "type": "terminated", "sessionId": session_id }),
    );
    Ok(())
}
