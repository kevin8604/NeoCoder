//! debug_run tool: launch a program under a DAP adapter, hit breakpoints, and
//! return runtime state (call stack + locals + evaluated expressions) so the
//! LLM can inspect variables without needing `println!` scaffolding.
//!
//! Typical flow inside one tool call:
//! ```text
//! start_session → set_breakpoints → configuration_done → continue → snapshot
//! → evaluate ["x", "self.items.len()"] → stop_session
//! ```
//!
//! The adapter is resolved by language (`codelldb` for Rust, `lldb-dap` for
//! C/C++/Swift, `python -m debugpy.adapter` for Python); callers may override
//! with `adapter: "/path/to/custom-adapter"`.

use async_trait::async_trait;
use serde_json::Value;

use super::{Tool, ToolContext};
use crate::dap::render_runtime_snapshot;

pub struct DebugRun;

#[async_trait]
impl Tool for DebugRun {
    fn name(&self) -> &str {
        "debug_run"
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> String {
        let manager = match ctx.dap_manager.as_ref() {
            Some(m) => m.clone(),
            None => return "Error: DAP manager not available in this runtime".to_string(),
        };

        let Some(program) = args["program"].as_str() else {
            return "Error: debug_run requires 'program' (path to the executable to debug)"
                .to_string();
        };
        let work_dir = ctx.project_path.as_deref().unwrap_or(".");
        let resolved_program = crate::agent::utils::resolve_path(Some(work_dir), program);
        if !resolved_program.exists() {
            return format!(
                "Error: program '{}' does not exist. Build it first (run_build), then pass the \
                 produced binary path.",
                resolved_program.display()
            );
        }

        // Sandbox read check on the program path
        if let Err(e) = ctx.sandbox.check_path(
            std::path::Path::new(&resolved_program),
            ctx.project_path.as_deref(),
            false,
        ) {
            return format!("Error: sandbox blocked program path: {}", e);
        }

        let language = args["language"].as_str().unwrap_or("rust");
        let cwd = args["cwd"]
            .as_str()
            .map(|s| crate::agent::utils::resolve_path(Some(work_dir), s))
            .map(|p| p.to_string_lossy().to_string())
            .or_else(|| Some(work_dir.to_string()));
        let program_args: Vec<String> = args["args"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        let adapter_override = args["adapter"].as_str();
        let stop_timeout = args["stop_timeout_secs"].as_u64().unwrap_or(10);

        // Breakpoints: [{ path, lines:[int] }]
        let bps_input: Vec<(String, Vec<u32>)> = args["breakpoints"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|b| {
                        let p = b["path"].as_str()?;
                        let lines = b["lines"]
                            .as_array()
                            .map(|ls| {
                                ls.iter()
                                    .filter_map(|l| l.as_u64().map(|x| x as u32))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let resolved = crate::agent::utils::resolve_path(Some(work_dir), p)
                            .to_string_lossy()
                            .to_string();
                        Some((resolved, lines))
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Per-call ephemeral session id
        let session_id = format!("debug_run-{}", uuid::Uuid::new_v4());

        // 1. Start session (spawns the debug adapter and issues launch request)
        if let Err(e) = manager
            .start_session(
                &session_id,
                language,
                &resolved_program.to_string_lossy(),
                program_args,
                cwd,
                adapter_override,
                None,
            )
            .await
        {
            return format!("Error: failed to start debug session — {}", e);
        }

        // Best-effort cleanup on any early return below.
        let mut result = run_debug_flow(
            &manager,
            &session_id,
            &bps_input,
            &args["evaluate"],
            stop_timeout,
        )
        .await;
        // 6. Stop session
        let _ = manager.stop_session(&session_id).await;
        if let Some(trailer) = result.strip_suffix("<<<DONE>>>") {
            result = trailer.trim_end().to_string();
        }
        result
    }
}

async fn run_debug_flow(
    manager: &crate::dap::DapManager,
    session_id: &str,
    bps: &[(String, Vec<u32>)],
    evaluate: &Value,
    stop_timeout: u64,
) -> String {
    // 2. Set breakpoints (per file)
    let mut bp_notes = Vec::new();
    for (path, lines) in bps {
        match manager
            .set_breakpoints(session_id, path, lines.clone())
            .await
        {
            Ok(resolved) => {
                let verified = resolved.iter().filter(|b| b.verified).count();
                bp_notes.push(format!(
                    "{}: requested {}, verified {}",
                    path.rsplit(['/', '\\']).next().unwrap_or(path),
                    resolved.len(),
                    verified
                ));
            }
            Err(e) => return format!("Error: setBreakpoints on {} failed — {}", path, e),
        }
    }

    // 3. configurationDone
    if let Err(e) = manager.configuration_done(session_id).await {
        return format!("Error: configurationDone failed — {}", e);
    }

    // 4. configurationDone already started execution; wait for the first stop
    let snap = match manager.resume(session_id, "wait", stop_timeout).await {
        Ok(s) => s,
        Err(e) => {
            return format!(
                "Debug run: no stop event — {}\nBreakpoints: {}\nProgram ran to completion or \
                 timed out before any breakpoint was hit. Verify the target path and line \
                 numbers (1-based) match the compiled source.",
                e,
                if bp_notes.is_empty() {
                    "(none set)".to_string()
                } else {
                    bp_notes.join(", ")
                }
            );
        }
    };

    // 5. Optional evaluate expressions
    let mut eval_notes = String::new();
    if let Some(arr) = evaluate.as_array() {
        for expr in arr.iter().filter_map(|v| v.as_str()) {
            match manager.evaluate(session_id, expr).await {
                Ok(r) => eval_notes.push_str(&format!("  {} = {}\n", expr, r)),
                Err(e) => eval_notes.push_str(&format!("  {} <error: {}>\n", expr, e)),
            }
        }
    }

    let mut out = String::new();
    if !bp_notes.is_empty() {
        out.push_str(&format!("Breakpoints: {}\n\n", bp_notes.join(", ")));
    }
    out.push_str(&render_runtime_snapshot(&snap));
    if !eval_notes.is_empty() {
        out.push_str("\n## Evaluated expressions\n");
        out.push_str(&eval_notes);
    }
    out.push_str("<<<DONE>>>");
    out
}
