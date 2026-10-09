import { useState, useEffect, useCallback } from "react";
import {
  listEvalFixtures,
  runEvalSuite,
  runEvalFixture,
  type EvalFixtureMeta,
  type EvalResult,
} from "../hooks/useTauri";
import {
  FlaskConical,
  Play,
  RefreshCw,
  CheckCircle2,
  XCircle,
  ChevronDown,
  ChevronRight,
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

  const loadFixtures = useCallback(async () => {
    const list = await listEvalFixtures();
    setFixtures(list);
  }, []);

  useEffect(() => {
    loadFixtures();
  }, [loadFixtures]);

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
