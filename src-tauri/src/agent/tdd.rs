//! TDD orchestration: a Red → Green → Refactor state machine enforced at the
//! workflow layer (not by prompt alone).
//!
//! The `tdd` tool starts/stops the state machine for a session; `TddGateHook`
//! (agent/hooks.rs) advances it based on `run_tests` outcomes and injects
//! phase-specific guidance into the LLM context. The agent never has to
//! remember which phase it is in — the hook tells it.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde::{Deserialize, Serialize};

/// TDD phases (Red → Green → Refactor → Done).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum TddPhase {
    /// Write a failing test first.
    Red,
    /// Make the test pass with minimal implementation.
    Green,
    /// Clean up while keeping tests green.
    Refactor,
    /// All done.
    Done,
}

/// Per-session TDD state.
#[derive(Debug, Clone)]
pub struct TddState {
    pub phase: TddPhase,
    pub test_command: Option<String>,
    pub started_at: String,
    /// Number of times the suite went green (for final reporting).
    pub green_count: u32,
}

/// Global per-session TDD state (tools are stateless singletons).
pub static TDD_STATES: LazyLock<Mutex<HashMap<String, TddState>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Start (or restart) TDD mode for a session. Returns a human-readable status.
pub fn start(session_id: &str, test_command: Option<String>) -> String {
    let mut states = TDD_STATES.lock().unwrap_or_else(|e| e.into_inner());
    states.insert(
        session_id.to_string(),
        TddState {
            phase: TddPhase::Red,
            test_command,
            started_at: chrono::Utc::now().to_rfc3339(),
            green_count: 0,
        },
    );
    format!(
        "TDD mode started (phase: RED).\n{}\n\
         Follow this phase strictly. The system will auto-advance the phase when you run tests \
         via the run_tests tool.",
        phase_guidance(TddPhase::Red)
    )
}

/// Stop TDD mode for a session. Returns the session summary.
pub fn stop(session_id: &str) -> String {
    let mut states = TDD_STATES.lock().unwrap_or_else(|e| e.into_inner());
    match states.remove(session_id) {
        Some(s) => format!(
            "TDD mode stopped after {} green run(s). Final phase: {:?}.",
            s.green_count, s.phase
        ),
        None => "TDD mode was not active for this session.".to_string(),
    }
}

/// Current status of the session's TDD state.
pub fn status(session_id: &str) -> String {
    let states = TDD_STATES.lock().unwrap_or_else(|e| e.into_inner());
    match states.get(session_id) {
        Some(s) => format!(
            "TDD active — phase: {:?} (green runs: {}, started: {})\n{}",
            s.phase,
            s.green_count,
            s.started_at,
            phase_guidance(s.phase)
        ),
        None => "TDD mode is not active for this session. Use tdd { action: 'start' } to begin."
            .to_string(),
    }
}

/// Query the current phase (hook/agent use).
pub fn get(session_id: &str) -> Option<TddState> {
    TDD_STATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(session_id)
        .cloned()
}

/// Overwrite the phase for a session (used by the gate hook on transitions).
pub fn set_phase(session_id: &str, phase: TddPhase) {
    if let Ok(mut states) = TDD_STATES.lock()
        && let Some(s) = states.get_mut(session_id)
    {
        s.phase = phase;
    }
}

/// Bump the green-run counter (used by the gate hook).
pub fn record_green(session_id: &str) {
    if let Ok(mut states) = TDD_STATES.lock()
        && let Some(s) = states.get_mut(session_id)
    {
        s.green_count = s.green_count.saturating_add(1);
    }
}

/// Phase-specific guidance text injected into the LLM context.
pub fn phase_guidance(phase: TddPhase) -> &'static str {
    match phase {
        TddPhase::Red => {
            "TDD[RED] Write a FAILING test first that captures the desired behavior. \
                          Run it with the run_tests tool and confirm it FAILS (red) before writing \
                          any implementation code."
        }
        TddPhase::Green => {
            "TDD[GREEN] The failing test is confirmed. Now write the MINIMAL \
                            implementation to make it pass. Run run_tests — the phase advances \
                            automatically once the suite goes green."
        }
        TddPhase::Refactor => {
            "TDD[REFACTOR] The suite is green. Clean up the implementation \
                               (naming, duplication, structure) while keeping run_tests green. \
                               Re-run run_tests after refactoring."
        }
        TddPhase::Done => {
            "TDD[DONE] All phases completed with a green suite. Summarize what was \
                           implemented and the final test results."
        }
    }
}

/// True when a file path looks like a test file.
pub fn is_test_file(path: &str) -> bool {
    let p = std::path::Path::new(path);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let lower = name.to_lowercase();

    if lower.ends_with("_test.rs")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.tsx")
        || lower.ends_with(".test.js")
        || lower.ends_with(".test.jsx")
        || lower.ends_with(".spec.ts")
        || lower.ends_with(".spec.tsx")
        || lower.ends_with(".spec.js")
        || lower.ends_with(".spec.jsx")
        || lower.starts_with("test_")
        || lower.ends_with("_test.py")
        || lower.ends_with("_test.go")
    {
        return true;
    }

    // Directory conventions: tests/ (Rust), __tests__/ (JS), test/ (python-ish)

    p.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == "tests" || s == "__tests__" || s == "test"
    })
}

/// True when a file path looks like a source (non-test) file.
pub fn is_source_file(path: &str) -> bool {
    let ext = std::path::Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    matches!(
        ext.as_str(),
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "java"
            | "c"
            | "cpp"
            | "h"
            | "hpp"
            | "css"
            | "scss"
            | "vue"
            | "svelte"
            | "rb"
            | "php"
            | "kt"
            | "swift"
    ) && !is_test_file(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifies_test_files() {
        assert!(is_test_file("src/lib_test.rs"));
        assert!(is_test_file("src/tests/mod.rs"));
        assert!(is_test_file("src/App.test.tsx"));
        assert!(is_test_file("tests/helpers/mod.rs"));
        assert!(is_test_file("test_utils.py"));
        assert!(is_test_file("__tests__/App.js"));
        assert!(!is_test_file("src/lib.rs"));
        assert!(!is_test_file("src/App.tsx"));
    }

    #[test]
    fn state_machine_start_stop() {
        let sid = "tdd-test-1";
        let msg = start(sid, Some("cargo test".into()));
        assert!(msg.contains("RED"));
        assert_eq!(get(sid).unwrap().phase, TddPhase::Red);
        set_phase(sid, TddPhase::Green);
        record_green(sid);
        assert_eq!(get(sid).unwrap().phase, TddPhase::Green);
        assert_eq!(get(sid).unwrap().green_count, 1);
        stop(sid);
        assert!(get(sid).is_none());
    }
}
