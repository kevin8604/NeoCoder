//! Tauri commands for the agent evaluation suite.
//!
//! These run against the live app (real `Wry` `AppHandle`), drive the agent
//! through [`crate::agent::eval::runner`], and return scored results for the
//! Evals panel. The LLM is never contacted during a run — the agent's
//! `mock_llm_responses` queue short-circuits it.

use crate::agent::eval::pack;
use crate::agent::eval::{EvalFixture, EvalResult, FixtureMeta, load_fixtures, runner, tuner};
use crate::agent::failure_lessons::FailureLessonsStore;
use crate::commands::config::ConfigState;
use std::sync::Arc;
use tauri::Manager;
use tokio::sync::Semaphore;

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

/// Max fixtures run concurrently. Fixtures are independent and mocked (no API
/// calls, no side effects, per-`session_id` event filtering), so running them in
/// parallel is safe; the cap bounds simultaneous AgentInstances / memory use.
const EVAL_CONCURRENCY: usize = 4;

/// Run a set of fixtures concurrently and return results in input order.
/// Each fixture gets its own `AgentInstance` + `chat-event` listener (filtered by
/// its own session id), so there is no cross-fixture interference. A panicking
/// task is dropped rather than aborting the batch.
async fn run_fixtures_parallel(
    app: &tauri::AppHandle,
    fixtures: &[EvalFixture],
) -> Vec<EvalResult> {
    use tokio::task::JoinSet;

    let sem = Arc::new(Semaphore::new(EVAL_CONCURRENCY));
    let mut js: JoinSet<(usize, EvalResult)> = JoinSet::new();

    for (idx, fx) in fixtures.iter().enumerate() {
        let app = app.clone();
        let fx = fx.clone();
        let sem = sem.clone();
        js.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore never closed");
            let r = runner::run_fixture(&app, &fx).await;
            (idx, r)
        });
    }

    let mut ordered: Vec<(usize, EvalResult)> = Vec::with_capacity(fixtures.len());
    while let Some(joined) = js.join_next().await {
        match joined {
            Ok(pair) => {
                let r = &pair.1;
                log::info!(
                    "[eval] {} → {} ({} failures)",
                    r.fixture_id,
                    if r.passed { "PASS" } else { "FAIL" },
                    r.failures.len()
                );
                ordered.push(pair);
            }
            Err(e) => log::warn!("[eval] a fixture task failed to complete: {e}"),
        }
    }
    ordered.sort_by_key(|(i, _)| *i);
    ordered.into_iter().map(|(_, r)| r).collect()
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
        "[eval] running {} fixtures (concurrency {}) from {}",
        fixtures.len(),
        EVAL_CONCURRENCY,
        dir.display()
    );
    Ok(run_fixtures_parallel(&app, &fixtures).await)
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
    run_tuning_report(&app).await
}

/// Run every tuning candidate over the fixture suite and return the ranked
/// report. Shared by the report-only command and the apply (write-back) one.
async fn run_tuning_report(app: &tauri::AppHandle) -> Result<tuner::TuningReport, String> {
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
        // Candidate configs stay sequential (so the leaderboard order is stable);
        // the independent, mocked fixtures within each candidate run in parallel.
        let merged: Vec<EvalFixture> = fixtures
            .iter()
            .map(|fx| tuner::merge_overrides(fx, &cand.overrides))
            .collect();
        let results = run_fixtures_parallel(app, &merged).await;
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

/// Run the tuning suite and, **only if a candidate passes the regression gate
/// (all fixtures pass, no regression vs baseline, strictly fewer API calls)**,
/// write its loop knobs back into the persisted `AppSettings.tuning`. The gate
/// IS the rollback guard: a config that could make anything worse is never
/// applied, and the previous knobs are returned so the caller can see/restore
/// them. The mocked-LLM suite uses no API calls, so this is safe and free.
#[tauri::command]
pub async fn apply_agent_tuning(app: tauri::AppHandle) -> Result<tuner::TuningOutcome, String> {
    run_apply_tuning(&app).await
}

/// Shared apply logic for the manual command and the background tuner. Runs the
/// gated write-back and appends a dated line to `tuning_reports.jsonl`.
pub(crate) async fn run_apply_tuning(
    app: &tauri::AppHandle,
) -> Result<tuner::TuningOutcome, String> {
    let report = run_tuning_report(app).await?;

    let state = app
        .try_state::<ConfigState>()
        .ok_or("ConfigState not available")?;
    let current = state.manager.read().await.get_settings().await;
    let previous = current.tuning;

    let outcome = match report.recommended_id.clone() {
        Some(id) => {
            let cands = tuner::default_candidates();
            let cand = cands
                .iter()
                .find(|c| c.id == id)
                .ok_or_else(|| format!("recommended candidate {id} not found"))?;
            let next = tuner::tuning_from_candidate(cand, previous);
            let mut new_settings = current.clone();
            new_settings.tuning = next;
            state
                .manager
                .write()
                .await
                .update_settings(new_settings)
                .await?;
            log::info!("[tuning] applied winning config: {id}");
            tuner::TuningOutcome {
                applied: true,
                candidate_id: Some(id),
                label: Some(cand.label.clone()),
                report,
                previous,
                applied_to: Some(next),
                reason: "regression-free and cheaper than baseline".to_string(),
            }
        }
        None => tuner::TuningOutcome {
            applied: false,
            candidate_id: None,
            label: None,
            report,
            previous,
            applied_to: None,
            reason: "baseline already best — no regression-free cheaper config found".to_string(),
        },
    };

    append_tuning_report(app, &outcome);
    Ok(outcome)
}

/// Append a compact one-line record of a tuning run to `tuning_reports.jsonl`
/// (the "report" side of the nightly job). Best-effort; never fails the run.
fn append_tuning_report(app: &tauri::AppHandle, out: &tuner::TuningOutcome) {
    let dir = match app.path().app_config_dir() {
        Ok(d) => d,
        Err(_) => return,
    };
    let _ = std::fs::create_dir_all(&dir);
    let record = serde_json::json!({
        "ts": chrono::Utc::now().to_rfc3339(),
        "applied": out.applied,
        "candidate_id": out.candidate_id,
        "label": out.label,
        "reason": out.reason,
        "baseline_pass": out.report.baseline.metrics.passed_all,
        "recommended_id": out.report.recommended_id,
        "total_api_calls": out.report.baseline.metrics.total_api_calls,
    });
    let line = match serde_json::to_string(&record) {
        Ok(s) => s,
        Err(_) => return,
    };
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("tuning_reports.jsonl"))
    {
        use std::io::Write;
        let _ = writeln!(f, "{line}");
    }
}

/// Read the most recent tuning reports (newest first), for the Evals panel.
#[tauri::command]
pub async fn get_tuning_reports(
    app: tauri::AppHandle,
    limit: Option<usize>,
) -> Result<Vec<serde_json::Value>, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    let path = dir.join("tuning_reports.jsonl");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Ok(Vec::new()),
    };
    let mut lines: Vec<serde_json::Value> = text
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    lines.reverse();
    lines.truncate(limit.unwrap_or(20));
    Ok(lines)
}

/// Background self-tuning loop. Inert unless `tuning.auto_tune_enabled` is set;
/// each tick re-reads the flag and interval, so toggling it in Settings takes
/// effect without a restart. Runs the same gated apply (mock LLM, no API cost).
pub fn spawn_auto_tuner(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        // Poll cheaply; the heavy suite only runs when enabled and due.
        let mut last_run = std::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            let state = match app.try_state::<ConfigState>() {
                Some(s) => s,
                None => continue,
            };
            let tuning = state.manager.read().await.get_settings().await.tuning;
            if !tuning.auto_tune_enabled {
                continue;
            }
            let interval = std::time::Duration::from_secs(
                (tuning.auto_tune_interval_hours.max(1) as u64) * 3600,
            );
            if last_run.elapsed() < interval {
                continue;
            }
            last_run = std::time::Instant::now();
            log::info!("[tuning] background auto-tune tick");
            match run_apply_tuning(&app).await {
                Ok(o) if o.applied => {
                    log::info!("[tuning] background applied {:?}", o.candidate_id)
                }
                Ok(_) => log::info!("[tuning] background kept baseline"),
                Err(e) => log::warn!("[tuning] background run failed: {e}"),
            }
        }
    });
}

// ── Knowledge pack (portable tuning + lessons) ──────────────────────────────

fn pack_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("knowledge_pack.json"))
}

/// Build a knowledge pack from the current tuning config + all failure lessons
/// and write it to `app_config_dir/knowledge_pack.json`. Returns the file path
/// so the user can share that file. Fixtures are NOT embedded (they live in the
/// repo and are shared via git); only their count is recorded as metadata.
#[tauri::command]
pub async fn export_knowledge_pack(app: tauri::AppHandle) -> Result<String, String> {
    let state = app
        .try_state::<ConfigState>()
        .ok_or("ConfigState not available")?;
    let tuning = state.manager.read().await.get_settings().await.tuning;

    let lessons_path = {
        let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
        dir.join("failure_lessons.json")
    };
    let store = FailureLessonsStore::load(lessons_path);
    let fixtures = load_fixtures(&fixtures_dir());

    let pack_obj = pack::KnowledgePack::new(tuning, store.all().to_vec(), fixtures.len());
    let json = pack::to_json(&pack_obj)?;
    let out = pack_path(&app)?;
    std::fs::write(&out, json).map_err(|e| e.to_string())?;
    log::info!("[pack] exported knowledge pack to {}", out.display());
    Ok(out.to_string_lossy().to_string())
}

/// Import a previously exported `knowledge_pack.json`: merge its lessons into the
/// local store (union, no clobbering) and apply its tuning config. Additive on
/// lessons; the pack's knobs are adopted wholesale.
#[tauri::command]
pub async fn import_knowledge_pack(app: tauri::AppHandle) -> Result<pack::ImportResult, String> {
    let path = pack_path(&app)?;
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("no knowledge pack at {}: {e}", path.display()))?;
    let pack_obj = pack::parse_pack(&text)?;

    let lessons_path = {
        let dir = app.path().app_config_dir().map_err(|e| e.to_string())?;
        dir.join("failure_lessons.json")
    };
    let mut store = FailureLessonsStore::load(lessons_path);
    let lessons_added = store.merge_lessons(&pack_obj.lessons);

    let state = app
        .try_state::<ConfigState>()
        .ok_or("ConfigState not available")?;
    let current = state.manager.read().await.get_settings().await;
    let mut new_settings = current.clone();
    new_settings.tuning = pack_obj.tuning;
    state
        .manager
        .write()
        .await
        .update_settings(new_settings)
        .await?;

    log::info!(
        "[pack] imported: +{lessons_added} lessons ({} in pack), tuning adopted",
        pack_obj.lessons.len()
    );
    Ok(pack::ImportResult {
        lessons_added,
        lessons_in_pack: pack_obj.lessons.len(),
        tuning_applied: true,
        exported_at: pack_obj.exported_at,
    })
}
