//! Pure decision core for agent control-knob auto-tuning.
//!
//! Rationale: the eval harness drives the agent through a *mocked* LLM, so
//! prompt text and temperature have **no observable effect** on fixture results
//! (the canned turns are returned regardless of what the system prompt says).
//! What *does* change outcomes under the mock are the knobs that steer control
//! flow: the iteration budget, the context-compaction budget, plan mode, and the
//! loop-detection thresholds. This module turns a run of fixture results — one
//! per candidate config — into a regression-gated, efficiency-ranked leaderboard.
//!
//! Everything here is pure and headless-testable; the app command that actually
//! runs each candidate through [`super::runner`] lives in `commands::eval`.

use super::{AgentOverrides, EvalFixture, EvalResult};
use serde::{Deserialize, Serialize};

/// A named configuration to try: the deltas merged onto fixtures that don't
/// already pin a given knob.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuneCandidate {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub overrides: AgentOverrides,
}

/// A curated, bounded candidate set. Kept small on purpose — a full cartesian
/// grid over six knobs would be slow (each candidate re-runs every fixture) and
/// most combinations are uninteresting. The interesting axis is loop sensitivity:
/// aggressive detection saves turns but risks cutting valid exploration short;
/// lenient detection does the opposite.
pub fn default_candidates() -> Vec<TuneCandidate> {
    let c = |id: &str, label: &str, o: AgentOverrides| TuneCandidate {
        id: id.into(),
        label: label.into(),
        overrides: o,
    };
    vec![
        c(
            "baseline",
            "Baseline (built-in defaults)",
            AgentOverrides::default(),
        ),
        c(
            "loop-aggressive",
            "Loop detection: aggressive (fail fast)",
            AgentOverrides {
                loop_no_progress: Some(3),
                loop_failure_streak: Some(3),
                loop_repeated_read: Some(2),
                loop_ping_pong_cycles: Some(2),
                loop_read_only_streak: Some(8),
                ..Default::default()
            },
        ),
        c(
            "loop-lenient",
            "Loop detection: lenient (allow more retries)",
            AgentOverrides {
                loop_no_progress: Some(8),
                loop_failure_streak: Some(8),
                loop_repeated_read: Some(5),
                loop_ping_pong_cycles: Some(4),
                loop_read_only_streak: Some(25),
                ..Default::default()
            },
        ),
        c(
            "compaction-eager",
            "Context: compact earlier (smaller budget)",
            AgentOverrides {
                max_context_tokens: Some(600),
                ..Default::default()
            },
        ),
    ]
}

/// Merge a candidate's overrides onto a fixture, **respecting any knob the
/// fixture already pins**. This is the correctness-critical rule: a fixture like
/// `budget-exhaust` deliberately sets `max_iterations: 3`, so a tuning candidate
/// must not silently overwrite it and change what the fixture is testing.
pub fn merge_overrides(fixture: &EvalFixture, cand: &AgentOverrides) -> EvalFixture {
    let mut f = fixture.clone();
    let base = &mut f.agent;
    if base.max_iterations.is_none() {
        base.max_iterations = cand.max_iterations;
    }
    if base.max_context_tokens.is_none() {
        base.max_context_tokens = cand.max_context_tokens;
    }
    if base.plan_mode.is_none() {
        base.plan_mode = cand.plan_mode;
    }
    if base.temperature.is_none() {
        base.temperature = cand.temperature;
    }
    if base.max_tokens.is_none() {
        base.max_tokens = cand.max_tokens;
    }
    if base.system_prompt.is_none() {
        base.system_prompt = cand.system_prompt.clone();
    }
    if base.tool_names.is_empty() {
        base.tool_names = cand.tool_names.clone();
    }
    if base.loop_no_progress.is_none() {
        base.loop_no_progress = cand.loop_no_progress;
    }
    if base.loop_ping_pong_cycles.is_none() {
        base.loop_ping_pong_cycles = cand.loop_ping_pong_cycles;
    }
    if base.loop_failure_streak.is_none() {
        base.loop_failure_streak = cand.loop_failure_streak;
    }
    if base.loop_read_only_streak.is_none() {
        base.loop_read_only_streak = cand.loop_read_only_streak;
    }
    if base.loop_repeated_read.is_none() {
        base.loop_repeated_read = cand.loop_repeated_read;
    }
    f
}

/// Aggregate one candidate's fixture results.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CandidateMetrics {
    pub total: usize,
    pub passed: usize,
    pub passed_all: bool,
    pub total_api_calls: u32,
    pub total_iterations: u32,
    pub passed_ids: Vec<String>,
    pub failed_ids: Vec<String>,
}

pub fn summarize(results: &[EvalResult]) -> CandidateMetrics {
    let mut m = CandidateMetrics {
        total: results.len(),
        ..Default::default()
    };
    for r in results {
        m.total_api_calls += r.metrics.api_calls;
        m.total_iterations += r.metrics.iterations;
        if r.passed {
            m.passed += 1;
            m.passed_ids.push(r.fixture_id.clone());
        } else {
            m.failed_ids.push(r.fixture_id.clone());
        }
    }
    m.passed_all = m.total > 0 && m.passed == m.total;
    m
}

/// Fixtures the baseline passed but this candidate failed.
pub fn detect_regressions(baseline: &CandidateMetrics, cand: &CandidateMetrics) -> Vec<String> {
    cand.failed_ids
        .iter()
        .filter(|id| baseline.passed_ids.iter().any(|b| b == *id))
        .cloned()
        .collect()
}

/// One candidate's outcome after the suite has run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CandidateReport {
    pub candidate_id: String,
    pub label: String,
    pub is_baseline: bool,
    pub metrics: CandidateMetrics,
    pub regressions: Vec<String>,
}

/// Order non-baseline candidates best-first: regression-free, then all-pass,
/// then lowest API calls, then lowest iterations, then id (deterministic tiebreak).
pub fn rank(candidates: Vec<CandidateReport>) -> Vec<CandidateReport> {
    let mut v = candidates;
    v.sort_by(|a, b| {
        let reg_a = a.regressions.is_empty();
        let reg_b = b.regressions.is_empty();
        reg_b
            .cmp(&reg_a)
            .then(b.metrics.passed_all.cmp(&a.metrics.passed_all))
            .then(a.metrics.total_api_calls.cmp(&b.metrics.total_api_calls))
            .then(a.metrics.total_iterations.cmp(&b.metrics.total_iterations))
            .then(a.candidate_id.cmp(&b.candidate_id))
    });
    v
}

/// The recommended config: best-ranked candidate that passes everything, has no
/// regressions, and uses strictly fewer API calls than the baseline (a real
/// efficiency win). `None` means no candidate beats the baseline — keep it.
pub fn recommended(ranked: &[CandidateReport], baseline: &CandidateReport) -> Option<String> {
    ranked.iter().find_map(|c| {
        if !c.is_baseline
            && c.metrics.passed_all
            && c.regressions.is_empty()
            && c.metrics.total_api_calls < baseline.metrics.total_api_calls
        {
            Some(c.candidate_id.clone())
        } else {
            None
        }
    })
}

/// The full tuning outcome: the baseline run plus every other candidate ranked
/// best-first, and the recommended candidate id (if one beats the baseline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningReport {
    pub baseline: CandidateReport,
    pub ranked: Vec<CandidateReport>,
    pub recommended_id: Option<String>,
}

/// Assemble candidate reports (exactly one must be `is_baseline`) into a
/// [`TuningReport`]: split out the baseline, rank the rest, and compute the
/// recommendation. Returns `None` if no baseline is present.
pub fn assemble(reports: Vec<CandidateReport>) -> Option<TuningReport> {
    let baseline = reports.iter().find(|r| r.is_baseline).cloned()?;
    let non_baseline: Vec<CandidateReport> =
        reports.into_iter().filter(|r| !r.is_baseline).collect();
    let ranked = rank(non_baseline);
    let recommended_id = recommended(&ranked, &baseline);
    Some(TuningReport {
        baseline,
        ranked,
        recommended_id,
    })
}

/// Fold a winning candidate's loop-knob overrides onto the current persisted
/// [`TuningConfig`]. Only the knobs the candidate actually sets are changed;
/// everything else is preserved. Used by the apply step after the regression
/// gate has already accepted the candidate.
pub fn tuning_from_candidate(
    cand: &TuneCandidate,
    current: crate::config::TuningConfig,
) -> crate::config::TuningConfig {
    let mut t = current;
    let o = &cand.overrides;
    if let Some(v) = o.loop_no_progress {
        t.loop_no_progress = v;
    }
    if let Some(v) = o.loop_ping_pong_cycles {
        t.loop_ping_pong = v;
    }
    if let Some(v) = o.loop_failure_streak {
        t.loop_failure_streak = v;
    }
    if let Some(v) = o.loop_read_only_streak {
        t.loop_read_only_streak = v;
    }
    if let Some(v) = o.loop_repeated_read {
        t.loop_repeated_read = v;
    }
    t
}

/// Outcome of an apply-tuning attempt: whether the winning config was written
/// back, and the before/after knob snapshot plus the full report for the UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningOutcome {
    pub applied: bool,
    pub candidate_id: Option<String>,
    pub label: Option<String>,
    pub report: TuningReport,
    pub previous: crate::config::TuningConfig,
    pub applied_to: Option<crate::config::TuningConfig>,
    pub reason: String,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::eval::EvalMetrics;
    use crate::config::TuningConfig;

    fn fx_with(agent: AgentOverrides) -> EvalFixture {
        EvalFixture {
            id: "f".into(),
            name: "F".into(),
            description: String::new(),
            prompt: "p".into(),
            turns: vec![],
            agent,
            plan_response: None,
            expect: Default::default(),
        }
    }

    fn result(id: &str, passed: bool, api: u32, iters: u32) -> EvalResult {
        EvalResult {
            fixture_id: id.into(),
            name: id.into(),
            passed,
            failures: vec![],
            metrics: EvalMetrics {
                api_calls: api,
                iterations: iters,
                ..Default::default()
            },
        }
    }

    #[test]
    fn merge_respects_fixture_pinned_knob() {
        // fixture pins max_iterations=3; candidate wants 10 → keep 3.
        let fixture = fx_with(AgentOverrides {
            max_iterations: Some(3),
            ..Default::default()
        });
        let cand = AgentOverrides {
            max_iterations: Some(10),
            max_context_tokens: Some(600),
            ..Default::default()
        };
        let merged = merge_overrides(&fixture, &cand);
        assert_eq!(merged.agent.max_iterations, Some(3), "fixture value wins");
        // fixture leaves max_context_tokens unset → candidate applies.
        assert_eq!(merged.agent.max_context_tokens, Some(600));
    }

    #[test]
    fn merge_fills_unset_loop_and_tool_knobs() {
        let fixture = fx_with(AgentOverrides::default());
        let cand = AgentOverrides {
            loop_no_progress: Some(3),
            tool_names: vec!["read_file".into()],
            ..Default::default()
        };
        let merged = merge_overrides(&fixture, &cand);
        assert_eq!(merged.agent.loop_no_progress, Some(3));
        assert_eq!(merged.agent.tool_names, vec!["read_file".to_string()]);
    }

    #[test]
    fn merge_does_not_clobber_nonempty_tool_list() {
        let fixture = fx_with(AgentOverrides {
            tool_names: vec!["grep".into()],
            ..Default::default()
        });
        let cand = AgentOverrides {
            tool_names: vec!["read_file".into()],
            ..Default::default()
        };
        let merged = merge_overrides(&fixture, &cand);
        assert_eq!(merged.agent.tool_names, vec!["grep".to_string()]);
    }

    #[test]
    fn summarize_counts_and_flags_all_pass() {
        let rs = vec![result("a", true, 2, 3), result("b", false, 1, 1)];
        let m = summarize(&rs);
        assert_eq!(m.total, 2);
        assert_eq!(m.passed, 1);
        assert!(!m.passed_all);
        assert_eq!(m.total_api_calls, 3);
        assert_eq!(m.total_iterations, 4);
        assert_eq!(m.passed_ids, vec!["a".to_string()]);
        assert_eq!(m.failed_ids, vec!["b".to_string()]);
    }

    #[test]
    fn summarize_empty_is_not_all_pass() {
        let m = summarize(&[]);
        assert_eq!(m.total, 0);
        assert!(!m.passed_all, "empty suite must not be treated as passing");
    }

    #[test]
    fn regressions_are_baseline_passes_that_candidate_failed() {
        let base = summarize(&[result("a", true, 1, 1), result("b", true, 1, 1)]);
        let cand = summarize(&[result("a", false, 1, 1), result("b", true, 1, 1)]);
        let reg = detect_regressions(&base, &cand);
        assert_eq!(reg, vec!["a".to_string()]);
    }

    #[test]
    fn rank_prefers_regression_free_then_efficient() {
        let mk = |id: &str, passed_all: bool, api: u32, reg: Vec<&str>| CandidateReport {
            candidate_id: id.into(),
            label: id.into(),
            is_baseline: false,
            metrics: CandidateMetrics {
                passed_all,
                total_api_calls: api,
                ..Default::default()
            },
            regressions: reg.into_iter().map(String::from).collect(),
        };
        let ranked = rank(vec![
            mk("slow-clean", true, 10, vec![]),
            mk("fast-regress", true, 2, vec!["a"]),
            mk("lean-clean", true, 4, vec![]),
            mk("broken", false, 1, vec![]),
        ]);
        // regression-free + all-pass first, then by api_calls asc.
        assert_eq!(ranked[0].candidate_id, "lean-clean");
        assert_eq!(ranked[1].candidate_id, "slow-clean");
        // a regression outranks a cheaper all-pass, so fast-regress is last-but-one
        // only if not broken-first; broken fails all-pass so ranks below clean ones.
        assert_eq!(ranked[3].candidate_id, "fast-regress");
    }

    #[test]
    fn recommended_requires_improvement_over_baseline() {
        let baseline = CandidateReport {
            candidate_id: "baseline".into(),
            label: String::new(),
            is_baseline: true,
            metrics: CandidateMetrics {
                passed_all: true,
                total_api_calls: 5,
                ..Default::default()
            },
            regressions: vec![],
        };
        let better = CandidateReport {
            candidate_id: "loop-aggressive".into(),
            label: String::new(),
            is_baseline: false,
            metrics: CandidateMetrics {
                passed_all: true,
                total_api_calls: 3,
                ..Default::default()
            },
            regressions: vec![],
        };
        let ranked = rank(vec![better]);
        assert_eq!(
            recommended(&ranked, &baseline).as_deref(),
            Some("loop-aggressive")
        );

        // Equal cost, no strict improvement → not recommended (keep baseline).
        let equal = CandidateReport {
            metrics: CandidateMetrics {
                passed_all: true,
                total_api_calls: 5,
                ..Default::default()
            },
            ..ranked[0].clone()
        };
        let ranked_eq = rank(vec![equal]);
        assert_eq!(recommended(&ranked_eq, &baseline), None);
    }

    #[test]
    fn assemble_splits_baseline_and_ranks_rest() {
        let base = CandidateReport {
            candidate_id: "baseline".into(),
            label: "Baseline".into(),
            is_baseline: true,
            metrics: CandidateMetrics {
                passed_all: true,
                total_api_calls: 8,
                ..Default::default()
            },
            regressions: vec![],
        };
        let good = CandidateReport {
            candidate_id: "loop-aggressive".into(),
            label: String::new(),
            is_baseline: false,
            metrics: CandidateMetrics {
                passed_all: true,
                total_api_calls: 4,
                ..Default::default()
            },
            regressions: vec![],
        };
        let bad = CandidateReport {
            candidate_id: "broken".into(),
            label: String::new(),
            is_baseline: false,
            metrics: CandidateMetrics {
                passed_all: false,
                total_api_calls: 1,
                ..Default::default()
            },
            regressions: vec!["x".into()],
        };
        let report =
            assemble(vec![bad.clone(), good.clone(), base.clone()]).expect("baseline present");
        assert_eq!(report.baseline.candidate_id, "baseline");
        assert_eq!(report.ranked[0].candidate_id, "loop-aggressive");
        assert_eq!(report.recommended_id.as_deref(), Some("loop-aggressive"));
        // assemble with no baseline returns None.
        assert!(assemble(vec![good, bad]).is_none());
    }

    #[test]
    fn tuning_from_candidate_only_changes_set_knobs() {
        let current = TuningConfig::default(); // 5/3/5/15/3
        let cand = TuneCandidate {
            id: "loop-aggressive".into(),
            label: String::new(),
            overrides: AgentOverrides {
                loop_no_progress: Some(3),
                loop_read_only_streak: Some(8),
                ..Default::default()
            },
        };
        let next = tuning_from_candidate(&cand, current);
        assert_eq!(next.loop_no_progress, 3, "set knob applied");
        assert_eq!(next.loop_read_only_streak, 8, "set knob applied");
        assert_eq!(
            next.loop_ping_pong, current.loop_ping_pong,
            "unset knob preserved"
        );
        assert_eq!(next.loop_failure_streak, current.loop_failure_streak);
        assert_eq!(next.loop_repeated_read, current.loop_repeated_read);
    }

    #[test]
    fn tuning_from_baseline_candidate_is_identity() {
        // The baseline candidate has no overrides → current config unchanged.
        let current = TuningConfig::default();
        let cand = TuneCandidate {
            id: "baseline".into(),
            label: String::new(),
            overrides: AgentOverrides::default(),
        };
        let next = tuning_from_candidate(&cand, current);
        assert_eq!(next.loop_no_progress, current.loop_no_progress);
        assert_eq!(next.loop_read_only_streak, current.loop_read_only_streak);
    }
}
