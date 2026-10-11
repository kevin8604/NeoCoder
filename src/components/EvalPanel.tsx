import { useState, useEffect, useCallback } from "react";
import {
  listEvalFixtures,
  runEvalSuite,
  runEvalFixture,
  runAgentTuning,
  applyAgentTuning,
  getTuningReports,
  type EvalFixtureMeta,
  type EvalResult,
  type TuningReport,
  type TuningOutcome,
  type TuningReportEntry,
  type CandidateReport,
} from "../hooks/useTauri";
import {
  FlaskConical,
  Play,
  RefreshCw,
  CheckCircle2,
  XCircle,
  ChevronDown,
  ChevronRight,
  Wand2,
  Star,
} from "lucide-react";

/**
 * Evals panel — runs the agent regression suite (mock-LLM driven, no API cost)
 * and shows pass/fail plus per-run metrics so a prompt/hook/budget change can
 * be judged by numbers rather than gut feel.
 */
export default function EvalPanel() {
  const [fixtures, setFixtures] = useState<EvalFixtureMeta[]>([]);
  const [results, setResults] = useState<Map<string, EvalResult>>(new Map());
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [tuning, setTuning] = useState(false);
  const [tuningReport, setTuningReport] = useState<TuningReport | null>(null);
  const [applying, setApplying] = useState(false);
  const [applyResult, setApplyResult] = useState<TuningOutcome | null>(null);
  const [reports, setReports] = useState<TuningReportEntry[]>([]);

  const loadFixtures = useCallback(async () => {
    const list = await listEvalFixtures();
    setFixtures(list);
  }, []);

  const loadReports = useCallback(async () => {
    setReports(await getTuningReports(20));
  }, []);

  useEffect(() => {
    loadFixtures();
    loadReports();
  }, [loadFixtures, loadReports]);

  const runAll = useCallback(async () => {
    setRunning(true);
    setError(null);
    try {
      const rs = await runEvalSuite();
      if (rs === null) {
        setError("评测仅在桌面应用中可用（非浏览器预览）。");
      } else {
        setResults(new Map(rs.map((r) => [r.fixture_id, r])));
      }
    } finally {
      setRunning(false);
    }
  }, []);

  const runOne = useCallback(async (id: string) => {
    setRunning(true);
    setError(null);
    try {
      const r = await runEvalFixture(id);
      if (r) setResults((prev) => new Map(prev).set(r.fixture_id, r));
      else setError("评测仅在桌面应用中可用（非浏览器预览）。");
    } finally {
      setRunning(false);
    }
  }, []);

  const runTune = useCallback(async () => {
    setTuning(true);
    setError(null);
    try {
      const report = await runAgentTuning();
      if (report) setTuningReport(report);
      else setError("自动调优仅在桌面应用中可用（非浏览器预览）。");
    } finally {
      setTuning(false);
    }
  }, []);

  const runApply = useCallback(async () => {
    setApplying(true);
    setError(null);
    try {
      const out = await applyAgentTuning();
      if (out) {
        setApplyResult(out);
        setTuningReport(out.report);
        await loadReports();
      } else {
        setError("自动调优仅在桌面应用中可用（非浏览器预览）。");
      }
    } finally {
      setApplying(false);
    }
  }, []);

  const toggle = (id: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });

  const passedCount = [...results.values()].filter((r) => r.passed).length;
  const totalRun = results.size;

  return (
    <div className="eval-panel">
      <div className="eval-header">
        <span className="eval-title">
          <FlaskConical size={15} /> Agent Evals
        </span>
        <div className="eval-header-actions">
          <button
            className="eval-btn"
            onClick={loadFixtures}
            disabled={running}
            title="重新加载 fixtures"
          >
            <RefreshCw size={13} />
          </button>
          <button
            className="eval-btn eval-btn-primary"
            onClick={runAll}
            disabled={running || fixtures.length === 0}
            title="运行全部评测"
          >
            {running ? <RefreshCw size={13} className="spinning" /> : <Play size={13} />}
            <span>{running ? "运行中…" : "运行全部"}</span>
          </button>
          <button
            className="eval-btn eval-btn-tune"
            onClick={runTune}
            disabled={running || tuning || fixtures.length === 0}
            title="自动搜索 agent 控制旋钮（循环阈值/预算）并按回归与效率排名"
          >
            {tuning ? <RefreshCw size={13} className="spinning" /> : <Wand2 size={13} />}
            <span>{tuning ? "调优中…" : "自动调优"}</span>
          </button>
          <button
            className="eval-btn eval-btn-tune"
            onClick={runApply}
            disabled={running || tuning || applying || fixtures.length === 0}
            title="运行调优并仅在有回归防护且更省时把胜出旋钮写回配置（否则不动）"
          >
            {applying ? <RefreshCw size={13} className="spinning" /> : <CheckCircle2 size={13} />}
            <span>{applying ? "应用中…" : "应用最优"}</span>
          </button>
        </div>
      </div>

      {totalRun > 0 && (
        <div className="eval-summary">
          <span className={`eval-score ${passedCount === totalRun ? "all-pass" : "has-fail"}`}>
            {passedCount}/{totalRun} 通过
          </span>
        </div>
      )}

      {error && <div className="eval-error">{error}</div>}

      {tuningReport && (
        <TuningBoard report={tuningReport} />
      )}

      {applyResult && <ApplyResultView out={applyResult} />}

      {reports.length > 0 && (
        <div className="tune-board">
          <div className="tune-board-title">历史调优报告（含后台）</div>
          {reports.map((r, i) => (
            <div key={i} className="tune-row">
              <span className="tune-name">
                <span className={`eval-dot ${r.applied ? "ok" : "idle"}`} />
                {r.applied ? `写回 ${r.candidate_id ?? "?"}` : "保持 baseline"}
              </span>
              <span className="tune-cost">{r.ts?.slice(0, 10) ?? ""}</span>
              <span className="tune-reg">{r.reason ?? ""}</span>
            </div>
          ))}
        </div>
      )}

      <div className="eval-list">
        {fixtures.length === 0 && !running && (
          <div className="eval-empty">未找到 fixtures（src-tauri/fixtures/eval/*.yaml）。</div>
        )}
        {fixtures.map((fx) => {
          const r = results.get(fx.id);
          const isOpen = expanded.has(fx.id);
          return (
            <div key={fx.id} className="eval-item">
              <div className="eval-item-row" onClick={() => toggle(fx.id)}>
                <span className="eval-caret">
                  {isOpen ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                </span>
                <span className="eval-status">
                  {!r ? (
                    <span className="eval-dot idle" title="未运行" />
                  ) : r.passed ? (
                    <CheckCircle2 size={14} color="var(--success)" />
                  ) : (
                    <XCircle size={14} color="var(--error)" />
                  )}
                </span>
                <span className="eval-item-name" title={fx.description}>
                  {fx.name}
                </span>
                {r && (
                  <span className="eval-metrics-inline">
                    iter {r.metrics.iterations} · api {r.metrics.api_calls} · tools{" "}
                    {r.metrics.tools_called.length}
                  </span>
                )}
                <button
                  className="eval-btn eval-btn-sm"
                  onClick={(e) => {
                    e.stopPropagation();
                    runOne(fx.id);
                  }}
                  disabled={running}
                  title="仅运行此用例"
                >
                  <Play size={11} />
                </button>
              </div>

              {isOpen && (
                <div className="eval-detail">
                  {fx.description && <p className="eval-desc">{fx.description}</p>}
                  {!r ? (
                    <div className="eval-hint">点击运行以查看指标与断言结果。</div>
                  ) : (
                    <>
                      <div className="eval-metric-grid">
                        <Metric label="iterations" value={r.metrics.iterations} />
                        <Metric label="api_calls" value={r.metrics.api_calls} />
                        <Metric label="finished" value={r.metrics.finished ? "yes" : "no"} />
                        <Metric label="errored" value={r.metrics.errored ? "yes" : "no"} />
                        <Metric
                          label="budget"
                          value={r.metrics.budget_exhausted ? "exhausted" : "ok"}
                        />
                        <Metric
                          label="events"
                          value={r.metrics.events.length}
                        />
                      </div>
                      {r.metrics.tools_called.length > 0 && (
                        <div className="eval-tools">
                          tools: {r.metrics.tools_called.join(", ")}
                        </div>
                      )}
                      {r.passed ? (
                        <div className="eval-pass">✓ 全部断言通过</div>
                      ) : (
                        <ul className="eval-failures">
                          {r.failures.map((f, i) => (
                            <li key={i}>{f}</li>
                          ))}
                        </ul>
                      )}
                      {r.metrics.final_text && (
                        <details className="eval-final">
                          <summary>final_text</summary>
                          <pre>{r.metrics.final_text}</pre>
                        </details>
                      )}
                      {r.metrics.events && r.metrics.events.length > 0 && (
                        <details className="eval-final">
                          <summary>事件轨迹 ({r.metrics.events.length})</summary>
                          <pre>{r.metrics.events.join(" → ")}</pre>
                        </details>
                      )}
                      {r.metrics.logs && r.metrics.logs.length > 0 && (
                        <details className="eval-final">
                          <summary>logs ({r.metrics.logs.length})</summary>
                          <pre>{r.metrics.logs.join("\n")}</pre>
                        </details>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string | number }) {
  return (
    <div className="eval-metric">
      <span className="eval-metric-label">{label}</span>
      <span className="eval-metric-value">{value}</span>
    </div>
  );
}

function TuningBoard({ report }: { report: TuningReport }) {
  const row = (c: CandidateReport, isRec: boolean) => (
    <div key={c.candidate_id} className={`tune-row ${c.is_baseline ? "tune-baseline" : ""} ${isRec ? "tune-recommended" : ""}`}>
      <span className="tune-name">
        {isRec && <Star size={12} className="tune-star" />}
        {c.label || c.candidate_id}
        {c.is_baseline && <span className="tune-tag">baseline</span>}
      </span>
      <span className={`tune-pass ${c.metrics.passed_all ? "ok" : "bad"}`}>
        {c.metrics.passed}/{c.metrics.total}
      </span>
      <span className="tune-cost">api {c.metrics.total_api_calls}</span>
      <span className="tune-cost">iter {c.metrics.total_iterations}</span>
      <span className={`tune-reg ${c.regressions.length ? "bad" : ""}`}>
        {c.regressions.length ? `回归 ${c.regressions.length}` : "无回归"}
      </span>
    </div>
  );

  return (
    <div className="tune-board">
      <div className="tune-board-title">控制旋钮调优排名</div>
      {row(report.baseline, false)}
      {report.ranked.map((c) => row(c, c.candidate_id === report.recommended_id))}
      <div className="tune-foot">
        {report.recommended_id
          ? `推荐：${report.recommended_id}（无回归且更省 API）`
          : "无配置优于 baseline（保持 baseline）"}
      </div>
    </div>
  );
}

function ApplyResultView({ out }: { out: TuningOutcome }) {
  const knobs: [string, number, number | undefined][] = out.applied_to
    ? [
        ["no_progress", out.previous.loop_no_progress, out.applied_to.loop_no_progress],
        ["ping_pong", out.previous.loop_ping_pong, out.applied_to.loop_ping_pong],
        ["failure_streak", out.previous.loop_failure_streak, out.applied_to.loop_failure_streak],
        ["read_only_streak", out.previous.loop_read_only_streak, out.applied_to.loop_read_only_streak],
        ["repeated_read", out.previous.loop_repeated_read, out.applied_to.loop_repeated_read],
      ]
    : [];

  return (
    <div className={`apply-result ${out.applied ? "applied" : "kept"}`}>
      <div className="apply-result-head">
        {out.applied ? "✓ 已写回最优配置" : "○ 保持 baseline（未改动）"}
        {out.candidate_id && <span className="apply-result-cand">{out.candidate_id}</span>}
      </div>
      <div className="apply-result-reason">{out.reason}</div>
      {out.applied && (
        <div className="apply-result-knobs">
          {knobs.map(([name, before, after]) =>
            before !== after ? (
              <span key={name} className="apply-knob">
                {name}: {before} → <b>{after}</b>
              </span>
            ) : null,
          )}
        </div>
      )}
    </div>
  );
}
