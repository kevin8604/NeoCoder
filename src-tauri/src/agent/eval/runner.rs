//! Eval runner — drives a real [`AgentInstance`] through its `mock_llm_responses`
//! seam and turns the events it emits into [`EvalMetrics`].
//!
//! Why this lives behind an app command rather than a unit test: `AgentInstance`
//! is bound to the `Wry` Tauri runtime (`AgentInstance::new(app: tauri::AppHandle)`),
//! so a `tauri::test::MockRuntime` handle is type-incompatible. The scoring logic
//! it relies on is pure and tested in the parent module; this file is intentionally
//! thin glue executed only when the user runs the suite from the Evals panel.
//!
//! Safety: fixtures are expected to be non-destructive. The LLM never runs (the
//! mock queue short-circuits it) and `project_path` is `None`, so file/terminal
//! tools operate with no working directory and simply fail — which is fine,
//! because `tools_called` is captured from the `ToolCall` event that fires
//! *before* execution.

use super::{
    AgentOverrides, EvalFixture, EvalMetrics, EvalResult, PlanResponse, build_queue, score,
};
use crate::agent::loop_detector::{LoopDetectionConfig, LoopDetector};
use crate::agent::{
    AgentDefinition, AgentInstance, ExecutionPhase, PlanAction, PlanApprovalAwaiters,
};
use crate::config::AppSettings;
use crate::llm;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use tauri::{Listener, Manager};

/// Run one fixture against the real agent using the current app settings.
pub async fn run_fixture(app: &tauri::AppHandle, fixture: &EvalFixture) -> EvalResult {
    let session_id = format!("eval-{}", uuid::Uuid::new_v4());

    // Read provider/model/key from live settings so budget (max_api_calls) and
    // context-window behave as they would for a real session. Fall back to inert
    // values when no settings state is present (the mock queue means the LLM is
    // never actually contacted either way).
    let (provider, api_key, chat_model) = read_settings(app).await;

    // Capture this session's events on the shared "chat-event" channel.
    let collector: Arc<Mutex<Collector>> = Arc::new(Mutex::new(Collector::default()));
    let sid = session_id.clone();
    let sink = collector.clone();
    // In plan-mode fixtures the agent blocks on the approval awaiter when it
    // emits `PlanCreated`. The real UI answers via Approve/Reject/Skip buttons;
    // here we answer programmatically so the run proceeds deterministically.
    let awaiters: Option<PlanApprovalAwaiters> = app
        .try_state::<PlanApprovalAwaiters>()
        .map(|s| s.inner().clone());
    let plan_action_for_handler = plan_action_from(fixture.plan_response.as_ref());
    let handler = app.listen("chat-event", move |payload| {
        let value: serde_json::Value = match serde_json::from_str(payload.payload()) {
            Ok(v) => v,
            Err(_) => return,
        };
        let kind = match value.as_object().and_then(|o| o.keys().next()) {
            Some(k) => k.clone(),
            None => return,
        };
        // PlanCreated carries its session id under `plan`, not top-level, so it
        // must be answered before the generic session_id filter below.
        if kind == "PlanCreated" {
            let ev_sid = value
                .get(&kind)
                .and_then(|p| p.get("plan"))
                .and_then(|p| p.get("session_id"))
                .and_then(|s| s.as_str());
            if ev_sid == Some(sid.as_str())
                && let Some(awaiters) = &awaiters
                && let Some(tx) = awaiters
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&sid)
            {
                let _ = tx.send(plan_action_for_handler.clone());
            }
        }
        // Filter by session_id when the variant carries one (all except Plan*).
        if let Some(ev_sid) = value
            .get(&kind)
            .and_then(|v| v.get("session_id"))
            .and_then(|s| s.as_str())
            && ev_sid != sid
        {
            return;
        }
        if let Ok(mut c) = sink.lock() {
            c.record(&kind, &value);
        }
    });

    let agent_def = build_definition(fixture);

    let messages = vec![llm::ChatMessage {
        role: "user".into(),
        content: fixture.prompt.clone(),
        images: None,
        tool_calls: None,
        tool_call_id: None,
    }];
    let cancelled = Arc::new(AtomicBool::new(false));

    let mut agent = AgentInstance::new(
        app.clone(),
        session_id.clone(),
        messages,
        provider,
        api_key,
        None, // base_url
        chat_model,
        None, // project_path: keep tools non-destructive
        None, // custom_instructions
        cancelled,
        agent_def.as_ref(),
        None, // memory_context
    );
    agent.mock_llm_responses = Some(build_queue(fixture.turns.clone()));
    if fixture.agent.plan_mode == Some(true) {
        agent.plan_mode = true;
        agent.execution_phase = ExecutionPhase::Planning;
    }
    // Force a small context budget so a fixture can deterministically exercise
    // the compaction path (emits `ContextTrimmed`) regardless of the live model.
    if let Some(tokens) = fixture.agent.max_context_tokens {
        agent.max_context_tokens = tokens;
    }
    // Auto-tuner knob: override loop-detection thresholds when the candidate
    // configures any. `loop_detector` is private to `crate::agent`, reachable
    // from this descendant module (same mechanism as `mock_llm_responses`).
    if let Some(cfg) = loop_config_from(&fixture.agent) {
        agent.loop_detector = LoopDetector::new(cfg);
    }

    let run_result = agent.run().await;
    let api_calls = agent.api_call_count() as u32;

    app.unlisten(handler);

    let mut collector = collector.lock().unwrap_or_else(|e| e.into_inner());
    let final_text = collector
        .finished_text
        .clone()
        .or_else(|| run_result.clone().ok())
        .unwrap_or_default();

    let mut metrics = EvalMetrics {
        iterations: collector.max_iteration,
        api_calls,
        finished: collector.finished,
        errored: collector.errored,
        budget_exhausted: collector.budget_exhausted,
        tools_called: std::mem::take(&mut collector.tools),
        final_text,
        events: std::mem::take(&mut collector.events),
        logs: std::mem::take(&mut collector.logs),
    };
    drop(collector);

    // If run() returned Err and we never saw an explicit Error event, still flag
    // the errored condition so `must_not_error` fixtures stay honest.
    if run_result.is_err() && !metrics.errored {
        metrics.errored = true;
    }

    score(fixture, metrics)
}

// ── internal helpers ────────────────────────────────────────────────────────

/// Aggregate of everything we learn from the event stream of one run.
#[derive(Default)]
struct Collector {
    events: Vec<String>,
    tools: Vec<String>,
    logs: Vec<String>,
    finished: bool,
    finished_text: Option<String>,
    errored: bool,
    budget_exhausted: bool,
    max_iteration: u32,
}

impl Collector {
    fn record(&mut self, kind: &str, value: &serde_json::Value) {
        self.events.push(kind.to_string());
        let body = value.get(kind).cloned().unwrap_or(serde_json::Value::Null);
        match kind {
            "Finished" => {
                self.finished = true;
                self.finished_text = body
                    .get("full_text")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());
            }
            "Error" => {
                self.errored = true;
                if let Some(msg) = body.get("message").and_then(|m| m.as_str()) {
                    self.logs.push(msg.to_string());
                }
            }
            "AgentLog" => {
                if let Some(msg) = body.get("message").and_then(|m| m.as_str()) {
                    self.logs.push(msg.to_string());
                }
            }
            "BudgetExhausted" => self.budget_exhausted = true,
            "ToolCall" => {
                if let Some(name) = body
                    .get("tool_call")
                    .and_then(|t| t.get("tool_name"))
                    .and_then(|n| n.as_str())
                {
                    self.tools.push(name.to_string());
                }
            }
            "AgentStatus" => {
                if let Some(it) = body.get("iteration").and_then(|i| i.as_u64()) {
                    self.max_iteration = self.max_iteration.max(it as u32);
                }
            }
            _ => {}
        }
    }
}

async fn read_settings(app: &tauri::AppHandle) -> (crate::config::LlmProvider, String, String) {
    match app.try_state::<Arc<tokio::sync::RwLock<AppSettings>>>() {
        Some(state) => {
            let g = state.read().await;
            (
                g.llm_provider.clone(),
                g.api_key.clone(),
                g.chat_model.clone(),
            )
        }
        None => (
            crate::config::LlmProvider::default(),
            String::new(),
            "gpt-4o".to_string(),
        ),
    }
}

/// Map the fixture's declared plan answer onto a real [`PlanAction`]. Defaults to
/// approving so a plan-mode run proceeds to execution instead of timing out.
fn plan_action_from(response: Option<&PlanResponse>) -> PlanAction {
    match response {
        None | Some(PlanResponse::Approve) => PlanAction::Approve,
        Some(PlanResponse::Skip) => PlanAction::Skip,
        Some(PlanResponse::Reject { reason }) => PlanAction::Reject(reason.clone()),
    }
}

/// Build a `LoopDetectionConfig` from a fixture's overrides, starting from the
/// built-in defaults and replacing only the thresholds the fixture (or the
/// auto-tuner candidate) pins. Returns `None` when nothing is overridden so the
/// agent keeps its default detector.
fn loop_config_from(o: &AgentOverrides) -> Option<LoopDetectionConfig> {
    if o.loop_no_progress.is_none()
        && o.loop_ping_pong_cycles.is_none()
        && o.loop_failure_streak.is_none()
        && o.loop_read_only_streak.is_none()
        && o.loop_repeated_read.is_none()
    {
        return None;
    }
    let mut cfg = LoopDetectionConfig::default();
    if let Some(v) = o.loop_no_progress {
        cfg.no_progress_threshold = v;
    }
    if let Some(v) = o.loop_ping_pong_cycles {
        cfg.ping_pong_cycles = v;
    }
    if let Some(v) = o.loop_failure_streak {
        cfg.failure_streak_threshold = v;
    }
    if let Some(v) = o.loop_read_only_streak {
        cfg.read_only_streak_threshold = v;
    }
    if let Some(v) = o.loop_repeated_read {
        cfg.repeated_read_threshold = v;
    }
    Some(cfg)
}

fn build_definition(fixture: &EvalFixture) -> Option<AgentDefinition> {
    let o = &fixture.agent;
    if o.is_empty() {
        return None;
    }
    Some(AgentDefinition {
        id: format!("eval-{}", fixture.id),
        name: fixture.name.clone(),
        description: fixture.description.clone(),
        system_prompt: o
            .system_prompt
            .clone()
            .unwrap_or_else(|| "You are an evaluation agent.".to_string()),
        tool_names: o.tool_names.clone(),
        model: None,
        temperature: o.temperature,
        max_iterations: o.max_iterations,
        max_tokens: o.max_tokens,
    })
}
