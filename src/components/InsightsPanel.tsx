import { useState, useEffect, useCallback } from "react";
import {
  getTelemetrySummary,
  getTelemetryEvents,
  replaySession,
  type TelemetrySummary,
  type TelemetryEventPayload,
  type AgentLogEntry,
} from "../hooks/useTauri";
import { TrendingUp, Rocket, CheckCircle2, Square, XCircle, Wrench, AlertTriangle, Compass, Scissors, MapPin, User, Bot, Zap, Package } from "lucide-react";

// ── 工具函数 ────────────────────────────────────────────────────────────────

function fmtNum(v: number): string {
  return v.toLocaleString();
}

function fmtDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`;
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.floor(ms / 60000)}m ${Math.floor((ms % 60000) / 1000)}s`;
}

function fmtTime(ts: number): string {
  return new Date(ts * 1000).toLocaleTimeString();
}

function eventIcon(ev: TelemetryEventPayload) {
  switch (ev.type) {
    case "session_start": return <Rocket size={12} />;
    case "session_end": return ev.outcome === "success" ? <CheckCircle2 size={12} color="var(--success)" /> : ev.outcome === "cancelled" ? <Square size={12} /> : <XCircle size={12} color="var(--error)" />;
    case "tool_call": return ev.success ? <Wrench size={12} /> : <AlertTriangle size={12} color="var(--warning)" />;
    case "routing_decision": return <Compass size={12} />;
    case "context_trim": return <Scissors size={12} />;
    default: return <MapPin size={12} />;
  }
}

function entryIcon(entry: AgentLogEntry) {
  switch (entry.type) {
    case "UserMessage": return <User size={12} />;
    case "AssistantMessage": return <Bot size={12} />;
    case "ToolCall": return <Wrench size={12} />;
    case "ToolResult": return entry.duration_ms ? <Zap size={12} /> : <Package size={12} />;
    case "CompactionSummary": return <Scissors size={12} />;
    case "Error": return <XCircle size={12} color="var(--error)" />;
    case "Cancelled": return <Square size={12} />;
    case "Completed": return <CheckCircle2 size={12} color="var(--success)" />;
    default: return <MapPin size={12} />;
  }
}

export default function InsightsPanel() {
  const [summary, setSummary] = useState<TelemetrySummary | null>(null);
  const [events, setEvents] = useState<TelemetryEventPayload[]>([]);
  const [loading, setLoading] = useState(true);
  // Agent 执行日志 replay
  const [sessionId, setSessionId] = useState("");
  const [log, setLog] = useState<AgentLogEntry[] | null>(null);
  const [logLoading, setLogLoading] = useState(false);
  const [logError, setLogError] = useState<string | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    const [s, e] = await Promise.all([getTelemetrySummary(), getTelemetryEvents(80)]);
    setSummary(s);
    setEvents(e ?? []);
    setLoading(false);
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const handleReplay = async () => {
    if (!sessionId.trim()) return;
    setLogLoading(true);
    setLogError(null);
    const result = await replaySession(sessionId.trim());
    if (result === null) {
      setLogError("未找到该会话的执行日志（session 目录下无 .jsonl）");
      setLog(null);
    } else if (result.length === 0) {
      setLogError("该会话没有持久化的执行日志");
      setLog([]);
    } else {
      setLog(result);
    }
    setLogLoading(false);
  };

  const cards = summary
    ? [
        { label: "Agent 会话", value: fmtNum(summary.total_sessions), sub: `${summary.successful_sessions} 成功 / ${summary.failed_sessions} 失败 / ${summary.cancelled_sessions} 取消` },
        { label: "成功率", value: `${(summary.success_rate * 100).toFixed(1)}%`, sub: `平均 ${summary.avg_iterations.toFixed(1)} 轮/会话` },
        { label: "工具调用", value: fmtNum(summary.total_tool_calls), sub: `${summary.total_tool_failures} 次失败 (${(summary.tool_failure_rate * 100).toFixed(1)}%)` },
        { label: "Token 消耗", value: fmtNum(summary.total_prompt_tokens + summary.total_completion_tokens), sub: `prompt ${fmtNum(summary.total_prompt_tokens)} / completion ${fmtNum(summary.total_completion_tokens)}` },
        { label: "内联补全", value: fmtNum(summary.total_inline_edits), sub: "Inline Edit 次数" },
      ]
    : [];

  const topTools = summary?.top_tools ?? [];
  const modelUsage = summary?.model_usage ? Object.entries(summary.model_usage).sort((a, b) => b[1] - a[1]) : [];
  const maxTool = topTools.length > 0 ? topTools[0][1] : 1;

  return (
    <div className="insights-panel">
      <div className="memory-panel-header">
        <span className="memory-panel-title"><TrendingUp size={15} /> Insights</span>
        <button className="memory-btn" onClick={load}>刷新</button>
      </div>

      {/* 遥测统计卡片 */}
      <div className="memory-stats-grid">
        {cards.map((c) => (
          <div className="memory-stat-card" key={c.label}>
            <div>
              <div className="memory-stat-value">{c.value}</div>
              <div className="memory-stat-label">{c.label}</div>
              <div className="memory-stat-sub">{c.sub}</div>
            </div>
          </div>
        ))}
      </div>

      <div className="insights-body">
        {/* 左：工具 + 模型使用 */}
        <div className="insights-col">
          <div className="memory-col-title">常用工具 Top {topTools.length}</div>
          {topTools.length === 0 && <div className="memory-notes-empty">暂无工具调用数据</div>}
          {topTools.map(([name, count]) => (
            <div className="insights-bar-row" key={name}>
              <span className="insights-bar-label">{name}</span>
              <div className="insights-bar-track">
                <div className="insights-bar-fill" style={{ width: `${(count / maxTool) * 100}%` }} />
              </div>
              <span className="insights-bar-count">{count}</span>
            </div>
          ))}

          <div className="memory-col-title" style={{ marginTop: 16 }}>模型使用分布</div>
          {modelUsage.length === 0 && <div className="memory-notes-empty">暂无模型数据</div>}
          {modelUsage.map(([model, count]) => (
            <div className="insights-bar-row" key={model}>
              <span className="insights-bar-label">{model}</span>
              <div className="insights-bar-track">
                <div className="insights-bar-fill" style={{ width: `${(count / modelUsage[0][1]) * 100}%` }} />
              </div>
              <span className="insights-bar-count">{count}</span>
            </div>
          ))}
        </div>

        {/* 中：Agent 执行日志 replay */}
        <div className="insights-col">
          <div className="memory-col-title">Agent 执行回放</div>
          <div className="memory-search-row">
            <input
              className="memory-search-input"
              placeholder="输入 session_id 回放执行日志…"
              value={sessionId}
              onChange={(e) => setSessionId(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleReplay()}
            />
            <button className="memory-btn" onClick={handleReplay} disabled={logLoading}>
              {logLoading ? "加载…" : "回放"}
            </button>
          </div>
          {logError && <div className="memory-error">{logError}</div>}
          {log && (
            <div className="insights-log">
              {log.map((entry) => (
                <div className="insights-log-entry" key={entry.seq}>
                  <span className="insights-log-icon">{entryIcon(entry)}</span>
                  <span className="insights-log-time">{fmtTime(entry.timestamp)}</span>
                  <span className="insights-log-type">{entry.type}</span>
                  <span className="insights-log-detail">
                    {entry.type === "ToolCall" && String(entry.name ?? "")}
                    {entry.type === "ToolResult" && `${String(entry.name ?? "")} · ${fmtDuration(Number(entry.duration_ms ?? 0))}`}
                    {entry.type === "Error" && String(entry.message ?? "").slice(0, 120)}
                    {entry.type === "CompactionSummary" && `${String(entry.tokens_before ?? "")} → ${String(entry.tokens_after ?? "")} tokens`}
                    {entry.type === "UserMessage" && String(entry.content ?? "").slice(0, 120)}
                    {entry.type === "AssistantMessage" && String(entry.content ?? "").slice(0, 120)}
                  </span>
                </div>
              ))}
            </div>
          )}
        </div>

        {/* 右：最近遥测事件时间线 */}
        <div className="insights-col">
          <div className="memory-col-title">最近事件</div>
          <div className="insights-log">
            {events.map((ev, i) => (
              <div className="insights-log-entry" key={i}>
                <span className="insights-log-icon">{eventIcon(ev)}</span>
                <span className="insights-log-time">{fmtTime(Number(ev.timestamp ?? 0))}</span>
                <span className="insights-log-type">{String(ev.type ?? "")}</span>
                <span className="insights-log-detail">
                  {ev.type === "tool_call" && `${String(ev.tool ?? "")} · ${fmtDuration(Number(ev.duration_ms ?? 0))}${ev.success ? "" : " · 失败"}`}
                  {ev.type === "session_start" && String(ev.model ?? "")}
                  {ev.type === "session_end" && `${String(ev.outcome ?? "")} · ${ev.iterations ?? ""} 轮 · ${fmtDuration(Number(ev.duration_ms ?? 0))}`}
                </span>
              </div>
            ))}
            {events.length === 0 && <div className="memory-notes-empty">暂无事件数据</div>}
          </div>
        </div>
      </div>
    </div>
  );
}
