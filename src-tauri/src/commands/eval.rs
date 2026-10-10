//! Tauri commands for the agent evaluation suite.
//!
//! These run against the live app (real `Wry` `AppHandle`), drive the agent
//! through [`crate::agent::eval::runner`], and return scored results for the
//! Evals panel. The LLM is never contacted during a run — the agent's
//! `mock_llm_responses` queue short-circuits it.

use crate::agent::eval::{EvalResult, FixtureMeta, load_fixtures, runner, tuner};

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

/// Auto-tune agent control knobs (loop thresholds, budgets, compaction budget).
///
/// Runs the whole fixture suite once per candidate config, merging each
/// candidate's overrides onto fixtures **without** clobbering knobs a fixture
/// intentionally pins. Returns a ranked leaderboard gated on zero regressions
/// vs the baseline and lower API-call cost. Like the suite, this drives the real
/// agent through the mocked-LLM runner, so it measures control-flow robustness
/// (not model answer quality) — which is exactly what these knobs affect.
#[tauri::command]
pub async fn run_agent_tuning(app: tauri::AppHandle) -> Result<tuner::TuningReport, String> {
    let dir = fixtures_dir();
    let fixtures = load_fixtures(&dir);
    if fixtures.is_empty() {
        return Err(format!("No eval fixtures found in {}", dir.display()));
    }

    let candidates = tuner::default_candidates();
    log::info!(
        "[tuning] running {} candidates over {} fixtures",
        candidates.len(),
        fixtures.len()
    );

    let mut reports: Vec<tuner::CandidateReport> = Vec::new();
    let mut baseline_metrics: Option<tuner::CandidateMetrics> = None;

    for cand in &candidates {
        let mut results = Vec::with_capacity(fixtures.len());
        for fx in &fixtures {
            let merged = tuner::merge_overrides(fx, &cand.overrides);
            results.push(runner::run_fixture(&app, &merged).await);
        }
        let metrics = tuner::summarize(&results);
        if cand.id == "baseline" {
            baseline_metrics = Some(metrics.clone());
        }
        reports.push(tuner::CandidateReport {
            candidate_id: cand.id.clone(),
            label: cand.label.clone(),
            is_baseline: cand.id == "baseline",
            metrics,
            regressions: Vec::new(),
        });
    }

    let baseline = baseline_metrics.ok_or("tuner produced no baseline candidate")?;
    for r in reports.iter_mut() {
        if !r.is_baseline {
            r.regressions = tuner::detect_regressions(&baseline, &r.metrics);
        }
    }

    tuner::assemble(reports).ok_or("tuner produced no baseline report".to_string())
}
