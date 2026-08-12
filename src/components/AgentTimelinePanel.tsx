import { useCallback, useEffect, useRef, useState } from "react";
import { listenToEvent, type UnlistenFn } from "../hooks/useTauri";
import {
  Activity,
  Trash2,
  Wrench,
  CheckCircle2,
  XCircle,
  Brain,
  GitCommit,
  FileDiff,
  Map,
  Loader2,
} from "lucide-react";

type EntryKind = "tool" | "status" | "thinking" | "checkpoint" | "edit" | "plan";
type EntryStatus = "running" | "done" | "error" | "info";

interface TimelineEntry {
  id: string;
  kind: EntryKind;
  status: EntryStatus;
  title: string;
  detail?: string;
  durationMs?: number;
  time: number;
}

interface TimelinePayload {
  ToolCall?: {
    session_id: string;
    tool_call: { id: string; tool_name: string; arguments: Record<string, any> };
  };
  ToolResult?: { session_id: string; result: string; duration_ms?: number };
  ToolRetry?: { session_id: string; tool_name: string; attempt: number; error: string };
  AgentStatus?: {
    session_id: string;
    status: string;
    iteration: number;
    total_iterations: number;
  };
  AgentThinking?: { session_id: string; thought: string };
  CheckpointCreated?: {
    session_id: string;
    iteration: number;
    commit_hash?: string | null;
    files: string[];
  };
  EditDiff?: { session_id: string; changes: Array<{ file_path: string }> };
  PlanCreated?: { plan?: { title?: string; steps?: unknown[] } };
  PlanApproved?: { plan?: { title?: string } };
  PlanRejected?: { plan?: { title?: string } };
  [key: string]: any;
}

const MAX_ENTRIES = 500;

function summarizeArgs(args?: Record<string, any>): string {
  if (!args) return "";
  const keys = ["file_path", "path", "selector", "url", "target", "command", "message"];
  for (const k of keys) {
    const v = args[k];
    if (typeof v === "string" && v) {
      return `${k}: ${v.length > 90 ? v.slice(0, 90) + "…" : v}`;
    }
  }
  const json = JSON.stringify(args);
  return json.length > 90 ? json.slice(0, 90) + "…" : json;
}

function statusIcon(status: EntryStatus, kind: EntryKind) {
  if (kind === "tool") {
    if (status === "running") return <Loader2 size={13} className="timeline-spin" />;
    if (status === "done") return <CheckCircle2 size={13} />;
    if (status === "error") return <XCircle size={13} />;
    return <Wrench size={13} />;
  }
  switch (kind) {
    case "thinking": return <Brain size={13} />;
    case "checkpoint": return <GitCommit size={13} />;
    case "edit": return <FileDiff size={13} />;
    case "plan": return <Map size={13} />;
    default: return <Activity size={13} />;
  }
}

export default function AgentTimelinePanel() {
  const [entries, setEntries] = useState<TimelineEntry[]>([]);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const scrollRef = useRef<HTMLDivElement>(null);
  const runningRef = useRef<TimelineEntry[]>([]);

  const push = useCallback((entry: TimelineEntry) => {
    setEntries((prev) => {
      const next = [...prev, entry];
      return next.length > MAX_ENTRIES ? next.slice(next.length - MAX_ENTRIES) : next;
    });
  }, []);

  const handlePayload = useCallback(
    (payload: TimelinePayload) => {
      const now = Date.now();
      let seq = 0;

      if (payload.ToolCall) {
        const tc = payload.ToolCall.tool_call;
        const entry: TimelineEntry = {
          id: `tc-${now}-${seq++}`,
          kind: "tool",
          status: "running",
          title: tc.tool_name,
          detail: summarizeArgs(tc.arguments),
          time: now,
        };
        runningRef.current = [...runningRef.current, entry];
        push(entry);
      }

      if (payload.ToolResult) {
        const open = runningRef.current;
        const result = payload.ToolResult.result || "";
        const isError =
          result.startsWith("Error") ||
          result.startsWith("[ERROR]") ||
          result.startsWith("[TIMEOUT]") ||
          result.includes("failed:");
        const detail = result.replace(/\s+/g, " ").slice(0, 160) || "(no output)";
        if (open.length > 0) {
          // Complete the most recent running tool call
          const last = open[open.length - 1];
          runningRef.current = open.slice(0, -1);
          setEntries((prev) =>
            prev.map((e) =>
              e.id === last.id
                ? {
                    ...e,
                    status: isError ? "error" : "done",
                    detail: e.detail ? `${e.detail} · ${detail}` : detail,
                    durationMs: payload.ToolResult?.duration_ms,
                  }
                : e
            )
          );
        } else {
          push({
            id: `tr-${now}-${seq++}`,
            kind: "tool",
            status: isError ? "error" : "done",
            title: "tool result",
            detail,
            durationMs: payload.ToolResult?.duration_ms,
            time: now,
          });
        }
      }

      if (payload.ToolRetry) {
        push({
          id: `tret-${now}-${seq++}`,
          kind: "tool",
          status: "error",
          title: `${payload.ToolRetry.tool_name} · retry #${payload.ToolRetry.attempt}`,
          detail: payload.ToolRetry.error.replace(/\s+/g, " ").slice(0, 160),
          time: now,
        });
      }

      if (payload.AgentStatus) {
        const st = payload.AgentStatus;
        push({
          id: `st-${now}-${seq++}`,
          kind: "status",
          status: "info",
          title: st.status,
          detail:
            st.total_iterations > 0
              ? `iteration ${st.iteration}/${st.total_iterations}`
              : undefined,
          time: now,
        });
      }

      if (payload.AgentThinking) {
        push({
          id: `th-${now}-${seq++}`,
          kind: "thinking",
          status: "info",
          title: "thinking",
          detail: payload.AgentThinking.thought.replace(/\s+/g, " ").slice(0, 200),
          time: now,
        });
      }

      if (payload.CheckpointCreated) {
        const cp = payload.CheckpointCreated;
        push({
          id: `cp-${now}-${seq++}`,
          kind: "checkpoint",
          status: "done",
          title: `checkpoint · iteration ${cp.iteration}`,
          detail: `${cp.files.length} file${cp.files.length === 1 ? "" : "s"} · ${
            cp.commit_hash ? cp.commit_hash.slice(0, 7) : "no commit"
          }`,
          time: now,
        });
      }

      if (payload.EditDiff) {
        const changes = payload.EditDiff.changes || [];
        push({
          id: `ed-${now}-${seq++}`,
          kind: "edit",
          status: "done",
          title: `edited ${changes.length} file${changes.length === 1 ? "" : "s"}`,
          detail: changes.map((c) => c.file_path).slice(0, 3).join(", "),
          time: now,
        });
      }

      if (payload.PlanCreated) {
        push({
          id: `pl-${now}-${seq++}`,
          kind: "plan",
          status: "info",
          title: "plan created",
          detail: payload.PlanCreated.plan?.title,
          time: now,
        });
      }

      if (payload.PlanApproved) {
        push({
          id: `pa-${now}-${seq++}`,
          kind: "plan",
          status: "done",
          title: "plan approved",
          detail: payload.PlanApproved.plan?.title,
          time: now,
        });
      }

      if (payload.PlanRejected) {
        push({
          id: `pr-${now}-${seq++}`,
          kind: "plan",
          status: "error",
          title: "plan rejected",
          detail: payload.PlanRejected.plan?.title,
          time: now,
        });
      }
    },
    [push]
  );

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    async function setup() {
      unlisten = await listenToEvent<TimelinePayload>("chat-event", handlePayload);
    }
    setup();
    return () => {
      if (unlisten) unlisten();
    };
  }, [handlePayload]);

  // Auto-scroll to the newest entry
  useEffect(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [entries]);

  const toggleExpand = (id: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const formatTime = (t: number) =>
    new Date(t).toLocaleTimeString("en-US", { hour12: false });

  return (
    <div className="cloud-agent-panel">
      <div className="cloud-agent-header">
        <h3>
          <Activity size={15} /> Agent Timeline
        </h3>
        <div className="timeline-header-actions">
          <button
            className="file-explorer-action-btn"
            onClick={() => {
              setEntries([]);
              runningRef.current = [];
            }}
            title="Clear timeline"
          >
            <Trash2 size={13} />
          </button>
        </div>
      </div>

      <div className="cloud-agent-list timeline-list" ref={scrollRef}>
        {entries.length === 0 ? (
          <div className="cloud-agent-empty">
            <p>No agent activity yet</p>
            <p className="hint">
              Run an Agent-mode task to see its tool calls, thinking and checkpoints here.
            </p>
          </div>
        ) : (
          entries.map((entry) => {
            const isOpen = expanded.has(entry.id);
            return (
              <div
                key={entry.id}
                className={`timeline-entry ${entry.kind} ${entry.status}`}
                onClick={() => entry.detail && toggleExpand(entry.id)}
              >
                <div className="timeline-rail">
                  <span className="timeline-dot">{statusIcon(entry.status, entry.kind)}</span>
                </div>
                <div className="timeline-body">
                  <div className="timeline-title">
                    <span className="timeline-tool">{entry.title}</span>
                    {entry.durationMs !== undefined && (
                      <span className="timeline-duration">
                        {entry.durationMs >= 1000
                          ? `${(entry.durationMs / 1000).toFixed(1)}s`
                          : `${entry.durationMs}ms`}
                      </span>
                    )}
                    <span className="timeline-time">{formatTime(entry.time)}</span>
                  </div>
                  {entry.detail && (
                    <div className={`timeline-detail ${isOpen ? "expanded" : ""}`}>
                      {entry.detail}
                    </div>
                  )}
                </div>
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
