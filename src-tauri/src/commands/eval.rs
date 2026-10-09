//! Tauri commands for the agent evaluation suite.
//!
//! These run against the live app (real `Wry` `AppHandle`), drive the agent
//! through [`crate::agent::eval::runner`], and return scored results for the
//! Evals panel. The LLM is never contacted during a run — the agent's
//! `mock_llm_responses` queue short-circuits it.

use crate::agent::eval::{EvalResult, FixtureMeta, load_fixtures, runner};

/// Resolve the fixtures directory from a set of candidates, first hit wins.
fn fixtures_dir() -> std::path::PathBuf {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    // Dev: repo checkout ships fixtures next to the crate manifest.
    candidates.push(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/eval"));
    // Runtime: relative to the current working directory.
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("fixtures/eval"));
        candidates.push(cwd.join("src-tauri/fixtures/eval"));
    }
    for c in &candidates {
        if c.is_dir() {
            return c.clone();
        }
    }
    candidates.into_iter().next().unwrap_or_default()
}

/// List available fixtures (metadata only) without running them.
#[tauri::command]
pub async fn list_eval_fixtures() -> Result<Vec<FixtureMeta>, String> {
    let dir = fixtures_dir();
    Ok(load_fixtures(&dir).iter().map(|f| f.meta()).collect())
}

/// Run every fixture and return scored results.
#[tauri::command]
pub async fn run_eval_suite(app: tauri::AppHandle) -> Result<Vec<EvalResult>, String> {
    let dir = fixtures_dir();
    let fixtures = load_fixtures(&dir);
    if fixtures.is_empty() {
        return Err(format!("No eval fixtures found in {}", dir.display()));
    }
    log::info!(
        "[eval] running {} fixtures from {}",
        fixtures.len(),
        dir.display()
    );

    let mut results = Vec::with_capacity(fixtures.len());
    for fx in &fixtures {
        let r = runner::run_fixture(&app, fx).await;
        log::info!(
            "[eval] {} → {} ({} failures)",
            r.fixture_id,
            if r.passed { "PASS" } else { "FAIL" },
            r.failures.len()
        );
        results.push(r);
    }
    Ok(results)
}

/// Run a single fixture by id.
#[tauri::command]
pub async fn run_eval_fixture(
    app: tauri::AppHandle,
    fixture_id: String,
) -> Result<EvalResult, String> {
    let dir = fixtures_dir();
    let fixtures = load_fixtures(&dir);
    let fx = fixtures
        .iter()
        .find(|f| f.id == fixture_id)
        .ok_or_else(|| format!("fixture {fixture_id:?} not found in {}", dir.display()))?;
    Ok(runner::run_fixture(&app, fx).await)
}
