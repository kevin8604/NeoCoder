//! Agent evaluation harness.
//!
//! The agent layer already ships sophisticated machinery (context compaction,
//! iteration/API budget, convergence notices, loop detection, failure
//! reflection, sub-agent dispatch, 13 lifecycle hooks). This module provides a
//! *regression net* for that machinery: a set of "golden" fixtures, each pairing
//! a scripted LLM conversation with expectations about how the agent should
//! behave, so we can measure — rather than eyeball — whether a prompt/hook/budget
//! change made the agent better or worse.
//!
//! Design split (this matters for what is verifiable):
//! - This file is a *pure* layer: fixture types, the scripted-response builder,
//!   the metrics struct, and the scoring function `evaluate`. It never touches
//!   Tauri or the network and is fully unit-testable.
//! - [`runner`] drives a real [`crate::agent::AgentInstance`] through its
//!   `mock_llm_responses` seam inside a Tauri `MockRuntime`, capturing the events
//!   the agent emits and turning them into [`EvalMetrics`].
//!
//! A fixture is authored as YAML under `fixtures/eval/*.yaml`.

use crate::llm::{LlmResponse, ToolCallRequest};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub mod runner;
pub mod tuner;

// ── Scripted LLM turns ─────────────────────────────────────────────────────

/// One canned LLM round-trip for a fixture. `MockTurn::Error` lets a fixture
/// exercise the error-handling hooks without a real flaky provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MockTurn {
    Text {
        content: String,
    },
    ToolCalls {
        #[serde(default)]
        content: Option<String>,
        calls: Vec<MockToolCall>,
    },
    /// Simulate the LLM call itself failing (`Err` from the provider).
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MockToolCall {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

impl MockTurn {
    /// Convert a scripted turn into the `Result<LlmResponse>` the agent's mock
    /// queue consumes. Call ids are synthesised deterministically (`call_0`, …)
    /// so fixtures stay readable.
    pub fn into_response(self) -> Result<LlmResponse, String> {
        match self {
            MockTurn::Text { content } => Ok(LlmResponse::Text(content)),
            MockTurn::Error { message } => Err(message),
            MockTurn::ToolCalls { content, calls } => Ok(LlmResponse::ToolCalls {
                content,
                calls: calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, c)| ToolCallRequest {
                        id: format!("call_{i}"),
                        name: c.name,
                        arguments: c.arguments,
                    })
                    .collect(),
            }),
        }
    }
}

/// Build the agent's `mock_llm_responses` queue from a fixture's turns.
pub fn build_queue(turns: Vec<MockTurn>) -> VecDeque<Result<LlmResponse, String>> {
    turns.into_iter().map(MockTurn::into_response).collect()
}

// ── Fixtures & expectations ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalFixture {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// The user prompt the agent is started with.
    pub prompt: String,
    /// Scripted LLM responses, consumed one per round-trip.
    pub turns: Vec<MockTurn>,
    /// Optional per-fixture agent knobs (budget, tool allowlist). When absent,
    /// the runner uses the default agent configuration.
    #[serde(default)]
    pub agent: AgentOverrides,
    /// How the harness answers a `PlanCreated` event in plan-mode fixtures.
    /// Defaults to approving. Without this, a plan-mode agent would block on the
    /// approval awaiter until its (long) timeout.
    #[serde(default)]
    pub plan_response: Option<PlanResponse>,
    #[serde(default)]
    pub expect: Expectations,
}

/// The action the eval harness injects when a plan-mode fixture emits
/// `PlanCreated`, mirroring the real UI's Approve/Reject/Skip buttons.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanResponse {
    Approve,
    Reject {
        #[serde(default)]
        reason: String,
    },
    Skip,
}

/// Per-fixture overrides fed into a synthetic `AgentDefinition` so a single
/// suite can test different budget sizes and tool surfaces.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentOverrides {
    pub max_iterations: Option<usize>,
    /// If non-empty, the agent only sees these tools (also keeps fixtures from
    /// invoking destructive tools like `run_terminal_command`).
    pub tool_names: Vec<String>,
    pub system_prompt: Option<String>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Start the agent in Planning phase so it emits `PlanCreated`.
    pub plan_mode: Option<bool>,
    /// Force the agent's context-window budget. A small value lets a fixture
    /// deterministically trigger context compaction (`ContextTrimmed`) without
    /// depending on the live settings' model context window.
    pub max_context_tokens: Option<usize>,
    /// Loop-detection thresholds, overridable so the auto-tuner can search the
    /// agentic-control surface. Each maps to a `LoopDetectionConfig` field;
    /// `None` keeps the built-in default. `0` disables that strategy.
    pub loop_no_progress: Option<usize>,
    pub loop_ping_pong_cycles: Option<usize>,
    pub loop_failure_streak: Option<usize>,
    pub loop_read_only_streak: Option<usize>,
    pub loop_repeated_read: Option<usize>,
}

impl AgentOverrides {
    pub fn is_empty(&self) -> bool {
        self.max_iterations.is_none()
            && self.tool_names.is_empty()
            && self.system_prompt.is_none()
            && self.temperature.is_none()
            && self.max_tokens.is_none()
            && self.plan_mode.is_none()
            && self.max_context_tokens.is_none()
            && self.loop_no_progress.is_none()
            && self.loop_ping_pong_cycles.is_none()
            && self.loop_failure_streak.is_none()
            && self.loop_read_only_streak.is_none()
            && self.loop_repeated_read.is_none()
    }
}

/// Lightweight metadata for listing fixtures without running them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixtureMeta {
    pub id: String,
    pub name: String,
    pub description: String,
}

impl EvalFixture {
    pub fn meta(&self) -> FixtureMeta {
        FixtureMeta {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
        }
    }
}

/// All expectations are optional; `None`/empty means "don't check".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Expectations {
    pub min_iterations: Option<u32>,
    pub max_iterations: Option<u32>,
    pub min_api_calls: Option<u32>,
    pub max_api_calls: Option<u32>,
    /// When Some(true) the agent must emit `Finished` (clean completion).
    pub must_finish: Option<bool>,
    /// When Some(true) the agent must surface an error (Error event or `run()` Err).
    pub must_error: Option<bool>,
    /// When Some(true) the agent must not surface an `Error` event.
    pub must_not_error: Option<bool>,
    /// When Some(true) the agent must hit `BudgetExhausted` (a budget test).
    pub expect_budget_exhausted: Option<bool>,
    /// Substrings the final answer must each contain.
    pub final_text_contains: Vec<String>,
    /// Tool names that must have been called at least once.
    pub tools_called: Vec<String>,
    /// Tool names that must never have been called.
    pub forbidden_tools: Vec<String>,
    /// `ChatEvent` variant names (PascalCase) that must have been emitted,
    /// e.g. `["PlanCreated"]`, `["ContextTrimmed"]`, `["BudgetExhausted"]`.
    pub expect_events_contains: Vec<String>,
    /// Substrings that must appear in at least one `AgentLog`/`Error` message —
    /// used to assert internal decisions like loop detection ("Hard stop").
    pub expect_log_contains: Vec<String>,
}

// ── Metrics captured from a live run ─────────────────────────────────────────

/// Raw observations from one agent run, before scoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EvalMetrics {
    pub iterations: u32,
    pub api_calls: u32,
    pub finished: bool,
    pub errored: bool,
    pub budget_exhausted: bool,
    pub tools_called: Vec<String>,
    pub final_text: String,
    /// Event kind names seen during the run (e.g. "ToolCall", "TodoUpdate"),
    /// captured via serde variant detection.
    pub events: Vec<String>,
    /// `AgentLog`/`Error` message texts seen during the run, for asserting
    /// internal decisions (loop detection, convergence, compaction notices).
    pub logs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalResult {
    pub fixture_id: String,
    pub name: String,
    pub passed: bool,
    pub failures: Vec<String>,
    pub metrics: EvalMetrics,
}

/// Record the event kind of a [`crate::chat::ChatEvent`] into metrics.
///
/// `ChatEvent` is externally tagged (no `#[serde(tag)]`), so serializing it
/// yields `{"VariantName": {...}}`; the first object key is the variant name.
/// For `AgentLog`/`Error` variants the message text is also captured so
/// `expect_log_contains` can assert internal decisions.
pub fn record_event_kind(metrics: &mut EvalMetrics, event: &crate::chat::ChatEvent) {
    let value = serde_json::to_value(event).unwrap_or_default();
    if let serde_json::Value::Object(map) = &value
        && let Some(kind) = map.keys().next()
    {
        metrics.events.push(kind.clone());
        if (kind == "AgentLog" || kind == "Error")
            && let Some(msg) = map
                .get(kind)
                .and_then(|b| b.get("message"))
                .and_then(|m| m.as_str())
        {
            metrics.logs.push(msg.to_string());
        }
    }
}

// ── Scoring (pure) ────────────────────────────────────────────────────────────

/// Compare expectations against observed metrics. Returns `(passed, failures)`.
pub fn evaluate(expect: &Expectations, m: &EvalMetrics) -> (bool, Vec<String>) {
    let mut f = Vec::new();

    if let Some(v) = expect.min_iterations
        && m.iterations < v
    {
        f.push(format!("iterations {} < min {}", m.iterations, v));
    }
    if let Some(v) = expect.max_iterations
        && m.iterations > v
    {
        f.push(format!("iterations {} > max {}", m.iterations, v));
    }
    if let Some(v) = expect.min_api_calls
        && m.api_calls < v
    {
        f.push(format!("api_calls {} < min {}", m.api_calls, v));
    }
    if let Some(v) = expect.max_api_calls
        && m.api_calls > v
    {
        f.push(format!("api_calls {} > max {}", m.api_calls, v));
    }
    if expect.must_finish == Some(true) && !m.finished {
        f.push("expected Finished event but agent did not finish".to_string());
    }
    if expect.must_finish == Some(false) && m.finished {
        f.push("expected agent NOT to finish but it did".to_string());
    }
    if expect.must_error == Some(true) && !m.errored {
        f.push("expected an error but agent did not error".to_string());
    }
    if expect.must_not_error == Some(true) && m.errored {
        f.push("expected no Error event but agent errored".to_string());
    }
    if expect.expect_budget_exhausted == Some(true) && !m.budget_exhausted {
        f.push("expected BudgetExhausted but agent did not exhaust budget".to_string());
    }
    if expect.expect_budget_exhausted == Some(false) && m.budget_exhausted {
        f.push("expected budget NOT exhausted but it was".to_string());
    }
    for s in &expect.final_text_contains {
        if !m.final_text.contains(s.as_str()) {
            f.push(format!("final_text missing substring {:?}", s));
        }
    }
    for t in &expect.tools_called {
        if !m.tools_called.iter().any(|x| x == t) {
            f.push(format!("expected tool {:?} was never called", t));
        }
    }
    for t in &expect.forbidden_tools {
        if m.tools_called.iter().any(|x| x == t) {
            f.push(format!("forbidden tool {:?} was called", t));
        }
    }
    for e in &expect.expect_events_contains {
        if !m.events.iter().any(|x| x == e) {
            f.push(format!("expected event {:?} was never emitted", e));
        }
    }
    for l in &expect.expect_log_contains {
        if !m.logs.iter().any(|x| x.contains(l.as_str())) {
            f.push(format!("no AgentLog/Error message contained {:?}", l));
        }
    }

    (f.is_empty(), f)
}

/// Evaluate a fixture's expectations against metrics into an [`EvalResult`].
pub fn score(fixture: &EvalFixture, metrics: EvalMetrics) -> EvalResult {
    let (passed, failures) = evaluate(&fixture.expect, &metrics);
    EvalResult {
        fixture_id: fixture.id.clone(),
        name: fixture.name.clone(),
        passed,
        failures,
        metrics,
    }
}

// ── Fixture loading (YAML) ─────────────────────────────────────────────────────

/// Load all `*.yaml`/`*.yml` fixtures from a directory, sorted by path.
/// Malformed files are skipped with a warning rather than aborting the suite.
pub fn load_fixtures(dir: &std::path::Path) -> Vec<EvalFixture> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            log::warn!("[eval] cannot read fixtures dir {}: {}", dir.display(), e);
            return out;
        }
    };
    let mut paths: Vec<_> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for p in paths {
        let ext = p.extension().and_then(|s| s.to_str()).unwrap_or("");
        if ext != "yaml" && ext != "yml" {
            continue;
        }
        match std::fs::read_to_string(&p) {
            Ok(text) => match parse_fixture(&text) {
                Ok(f) => out.push(f),
                Err(e) => log::warn!("[eval] skip {}: {}", p.display(), e),
            },
            Err(e) => log::warn!("[eval] read {}: {}", p.display(), e),
        }
    }
    out
}

/// Parse a single fixture from YAML text. Exposed for tests.
pub fn parse_fixture(text: &str) -> Result<EvalFixture, String> {
    serde_yaml::from_str(text).map_err(|e| e.to_string())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics_with(
        iterations: u32,
        api_calls: u32,
        finished: bool,
        tools: &[&str],
        final_text: &str,
    ) -> EvalMetrics {
        EvalMetrics {
            iterations,
            api_calls,
            finished,
            errored: false,
            budget_exhausted: false,
            tools_called: tools.iter().map(|s| s.to_string()).collect(),
            final_text: final_text.to_string(),
            events: vec![],
            logs: vec![],
        }
    }

    #[test]
    fn evaluate_empty_expectations_always_pass() {
        let (passed, failures) = evaluate(
            &Expectations::default(),
            &metrics_with(0, 0, false, &[], ""),
        );
        assert!(passed);
        assert!(failures.is_empty());
    }

    #[test]
    fn evaluate_iterations_bounds() {
        let exp = Expectations {
            min_iterations: Some(2),
            max_iterations: Some(4),
            ..Default::default()
        };
        assert!(evaluate(&exp, &metrics_with(3, 0, false, &[], "")).0);
        let (passed, f) = evaluate(&exp, &metrics_with(1, 0, false, &[], ""));
        assert!(!passed);
        assert!(f[0].contains("min"));
        let (passed, _) = evaluate(&exp, &metrics_with(5, 0, false, &[], ""));
        assert!(!passed);
    }

    #[test]
    fn evaluate_api_calls_bounds() {
        let exp = Expectations {
            max_api_calls: Some(3),
            ..Default::default()
        };
        assert!(evaluate(&exp, &metrics_with(0, 2, false, &[], "")).0);
        assert!(!evaluate(&exp, &metrics_with(0, 4, false, &[], "")).0);
    }

    #[test]
    fn evaluate_finish_and_error_flags() {
        let exp = Expectations {
            must_finish: Some(true),
            must_not_error: Some(true),
            ..Default::default()
        };
        let good = metrics_with(1, 1, true, &[], "done");
        assert!(evaluate(&exp, &good).0);

        let mut not_finished = good.clone();
        not_finished.finished = false;
        let (passed, f) = evaluate(&exp, &not_finished);
        assert!(!passed);
        assert!(f.iter().any(|x| x.contains("Finished")));

        let mut errored = good.clone();
        errored.errored = true;
        let (passed, f) = evaluate(&exp, &errored);
        assert!(!passed);
        assert!(f.iter().any(|x| x.contains("Error")));
    }

    #[test]
    fn evaluate_budget_exhausted() {
        let exp = Expectations {
            expect_budget_exhausted: Some(true),
            ..Default::default()
        };
        let mut m = metrics_with(25, 25, false, &[], "");
        m.budget_exhausted = true;
        assert!(evaluate(&exp, &m).0);
        let m2 = metrics_with(1, 1, true, &[], "ok");
        assert!(!evaluate(&exp, &m2).0);
    }

    #[test]
    fn evaluate_final_text_contains() {
        let exp = Expectations {
            final_text_contains: vec!["cargo".into(), "test".into()],
            ..Default::default()
        };
        assert!(
            evaluate(
                &exp,
                &metrics_with(0, 0, true, &[], "run cargo test please")
            )
            .0
        );
        let (passed, f) = evaluate(&exp, &metrics_with(0, 0, true, &[], "only cargo here"));
        assert!(!passed);
        assert!(f.iter().any(|x| x.contains("test")));
    }

    #[test]
    fn evaluate_tools_called_and_forbidden() {
        let exp = Expectations {
            tools_called: vec!["read_file".into()],
            forbidden_tools: vec!["run_terminal_command".into()],
            ..Default::default()
        };
        assert!(evaluate(&exp, &metrics_with(0, 0, true, &["read_file"], "")).0);
        // missing expected tool
        assert!(!evaluate(&exp, &metrics_with(0, 0, true, &[], "")).0);
        // forbidden tool present
        let (passed, f) = evaluate(
            &exp,
            &metrics_with(0, 0, true, &["read_file", "run_terminal_command"], ""),
        );
        assert!(!passed);
        assert!(f.iter().any(|x| x.contains("forbidden")));
    }

    #[test]
    fn evaluate_must_error_positive() {
        let exp = Expectations {
            must_error: Some(true),
            ..Default::default()
        };
        let mut m = metrics_with(5, 5, false, &[], "");
        assert!(!evaluate(&exp, &m).0); // not errored yet
        m.errored = true;
        assert!(evaluate(&exp, &m).0);
    }

    #[test]
    fn evaluate_expect_events_contains() {
        let exp = Expectations {
            expect_events_contains: vec!["PlanCreated".into(), "TodoUpdate".into()],
            ..Default::default()
        };
        let mut m = metrics_with(0, 0, true, &[], "");
        m.events = vec!["PlanCreated".into()];
        let (passed, f) = evaluate(&exp, &m);
        assert!(!passed);
        assert!(f.iter().any(|x| x.contains("TodoUpdate")));
        m.events.push("TodoUpdate".into());
        assert!(evaluate(&exp, &m).0);
    }

    #[test]
    fn evaluate_expect_log_contains() {
        let exp = Expectations {
            expect_log_contains: vec!["Hard stop".into()],
            ..Default::default()
        };
        let mut m = metrics_with(0, 0, false, &[], "");
        assert!(!evaluate(&exp, &m).0); // no logs
        m.logs = vec![
            "Loop detected: ping-pong".into(),
            "Hard stop: terminated".into(),
        ];
        assert!(evaluate(&exp, &m).0);
    }

    #[test]
    fn build_queue_preserves_error_variants() {
        let turns = vec![
            MockTurn::Text {
                content: "hi".into(),
            },
            MockTurn::Error {
                message: "boom".into(),
            },
            MockTurn::ToolCalls {
                content: None,
                calls: vec![MockToolCall {
                    name: "read_file".into(),
                    arguments: serde_json::json!({"path": "a.txt"}),
                }],
            },
        ];
        let mut q = build_queue(turns);
        assert_eq!(q.len(), 3);
        assert!(matches!(q.pop_front(), Some(Ok(LlmResponse::Text(_)))));
        assert!(matches!(q.pop_front(), Some(Err(_))));
        match q.pop_front() {
            Some(Ok(LlmResponse::ToolCalls { calls, .. })) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_0");
                assert_eq!(calls[0].name, "read_file");
            }
            other => panic!("expected tool calls, got {other:?}"),
        }
    }

    #[test]
    fn tool_call_ids_are_sequential_per_turn() {
        let q = build_queue(vec![MockTurn::ToolCalls {
            content: Some("thinking".into()),
            calls: vec![
                MockToolCall {
                    name: "glob".into(),
                    arguments: serde_json::json!({}),
                },
                MockToolCall {
                    name: "grep".into(),
                    arguments: serde_json::json!({}),
                },
            ],
        }]);
        match &q[0] {
            Ok(LlmResponse::ToolCalls { calls, content }) => {
                assert_eq!(calls[0].id, "call_0");
                assert_eq!(calls[1].id, "call_1");
                assert_eq!(content.as_deref(), Some("thinking"));
            }
            _ => panic!("expected tool calls"),
        }
    }

    #[test]
    fn parse_fixture_from_yaml() {
        let yaml = r#"
id: echo-task
name: Echo simple task
description: Agent answers with no tool use
prompt: Say hello
turns:
  - text:
      content: Hello there
agent:
  max_iterations: 5
  tool_names:
    - read_file
expect:
  must_finish: true
  final_text_contains:
    - Hello
"#;
        let f = parse_fixture(yaml).unwrap();
        assert_eq!(f.id, "echo-task");
        assert_eq!(f.turns.len(), 1);
        assert_eq!(f.expect.must_finish, Some(true));
        assert_eq!(f.expect.final_text_contains, vec!["Hello".to_string()]);
        assert_eq!(f.agent.max_iterations, Some(5));
        assert_eq!(f.agent.tool_names, vec!["read_file".to_string()]);
    }

    #[test]
    fn parse_fixture_defaults_when_optional_absent() {
        let yaml = r#"
id: min
name: Minimal
prompt: hi
turns:
  - text:
      content: yo
"#;
        let f = parse_fixture(yaml).unwrap();
        assert!(f.agent.is_empty());
        assert!(f.expect.tools_called.is_empty());
        assert_eq!(f.expect.must_finish, None);
    }

    #[test]
    fn parse_fixture_plan_mode_and_response() {
        let yaml = r#"
id: plan-x
name: Plan
prompt: plan it
turns:
  - text:
      content: hi
agent:
  plan_mode: true
  max_iterations: 8
plan_response:
  reject:
    reason: too risky
expect:
  must_finish: true
"#;
        let f = parse_fixture(yaml).unwrap();
        assert_eq!(f.agent.plan_mode, Some(true));
        assert!(matches!(f.plan_response, Some(PlanResponse::Reject { .. })));
    }

    #[test]
    fn parse_fixture_plan_approve_roundtrip() {
        let yaml = r#"
id: plan-y
name: PlanApprove
prompt: p
turns:
  - text:
      content: hi
plan_response: approve
"#;
        let f = parse_fixture(yaml).unwrap();
        assert!(matches!(f.plan_response, Some(PlanResponse::Approve)));
        assert_eq!(f.agent.plan_mode, None);
    }

    #[test]
    fn parse_fixture_max_context_tokens() {
        let yaml = r#"
id: ctx
name: Ctx
prompt: p
turns:
  - text:
      content: hi
agent:
  max_context_tokens: 300
"#;
        let f = parse_fixture(yaml).unwrap();
        assert_eq!(f.agent.max_context_tokens, Some(300));
        assert!(!f.agent.is_empty());
    }

    /// CI guard: every YAML on disk under `fixtures/eval/` must parse into a
    /// well-formed fixture, carry a non-empty id/name, and have unique ids.
    /// Runs headless under `cargo test --workspace` (no Tauri runtime needed),
    /// so malformed fixtures fail CI before the Evals panel is ever opened.
    #[test]
    fn all_fixtures_on_disk_parse_and_are_unique() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/eval");
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("fixtures dir {} unreadable: {}", dir.display(), e));

        let mut ids: Vec<String> = Vec::new();
        let mut count = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
            if ext != "yaml" && ext != "yml" {
                continue;
            }
            count += 1;
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
            let fixture = parse_fixture(&text)
                .unwrap_or_else(|e| panic!("fixture {} failed to parse: {}", path.display(), e));
            assert!(!fixture.id.is_empty(), "{} has empty id", path.display());
            assert!(
                !fixture.name.is_empty(),
                "{} has empty name",
                path.display()
            );
            assert!(
                !fixture.turns.is_empty(),
                "fixture {} has no mock turns",
                path.display()
            );
            ids.push(fixture.id);
        }

        assert!(count > 0, "no fixtures found in {}", dir.display());
        let before = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate fixture ids detected: {ids:?}");
    }

    #[test]
    fn score_wraps_result() {
        let f = EvalFixture {
            id: "x".into(),
            name: "X".into(),
            description: String::new(),
            prompt: "p".into(),
            turns: vec![],
            agent: AgentOverrides::default(),
            plan_response: None,
            expect: Expectations {
                must_finish: Some(true),
                ..Default::default()
            },
        };
        let r = score(&f, metrics_with(1, 1, true, &[], ""));
        assert_eq!(r.fixture_id, "x");
        assert!(r.passed);
        assert!(r.failures.is_empty());
    }

    #[test]
    fn record_event_kind_captures_variant_name() {
        use crate::chat::ChatEvent;
        let mut m = EvalMetrics::default();
        record_event_kind(
            &mut m,
            &ChatEvent::Finished {
                session_id: "s".into(),
                agent_id: None,
                full_text: "hi".into(),
            },
        );
        assert_eq!(m.events, vec!["Finished".to_string()]);
    }
}
