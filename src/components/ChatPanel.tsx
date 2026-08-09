import { useState, useRef, useEffect, useCallback, Fragment } from "react";
import ReactMarkdown from "react-markdown";
import SyntaxHighlighterWrapper from "./SyntaxHighlighterWrapper";
import type { Components } from "react-markdown";
import MentionMenu, { type MentionItem } from "./MentionMenu";
import {
  sendChatMessage,
  newSession as createNewSession,
  writeFile,
  readFile,
  answerAgentQuestion,
  answerConfirm,
  approvePlan,
  rejectPlan,
  skipPlan,
  getAgents,
  getSessions,
  getSessionMessages,
  deleteSession as deleteSessionApi,
  forkSession,
  listenToEvent,
  getFileTree,
  flattenFileTree,
  listSkills,
  executeSkill,
  type UnlistenFn,
  type AgentDefinition,
  type SessionInfo,
  type FlatFileItem,
  type SessionMessage,
  type SkillDefinition,
} from "../hooks/useTauri";
import { ChevronRight, Brain, ClipboardList, HelpCircle, X, AlertTriangle, Check, CheckCircle2, Clock, XCircle, Circle, FileText, Copy, Play, Pause, Sparkles } from "lucide-react";

// ── Image attachment helpers ────────────────────────────────────────

const MAX_IMAGE_SIZE = 10 * 1024 * 1024; // 10MB

function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = reject;
    reader.readAsDataURL(file);
  });
}

interface Message {
  role: "user" | "assistant" | "tool" | "thinking";
  content: string;
  toolCalls?: Array<{
    id: string;
    tool_name: string;
    arguments: Record<string, any>;
  }>;
  toolResult?: string;
  toolName?: string;
  isRunning?: boolean;
  durationMs?: number;
  timestamp?: number;
  isRetry?: boolean;
  retryAttempt?: number;
}

interface ToolCallPayload {
  tool_call?: {
    id: string;
    tool_name: string;
    arguments: Record<string, any>;
  };
  ToolCall?: {
    session_id: string;
    tool_call: {
      id: string;
      tool_name: string;
      arguments: Record<string, any>;
      timestamp?: number;
    };
  };
  ToolResult?: {
    session_id: string;
    result: string;
    duration_ms?: number;
  };
  ToolRetry?: {
    session_id: string;
    tool_name: string;
    attempt: number;
    error: string;
  };
  AgentThinking?: {
    session_id: string;
    thought: string;
  };
  AgentLog?: {
    session_id: string;
    level: string;
    message: string;
  };
  ContextTrimmed?: {
    session_id: string;
    trimmed_count: number;
    total_before: number;
    total_after: number;
  };
  ConfirmRequest?: {
    session_id: string;
    confirm_id: string;
    tool_name: string;
    description: string;
  };
  result?: string;
  tool_result?: string;
  session_id?: string;
  token?: string;
  full_text?: string;
  [key: string]: any;
}

interface TodoItem {
  id: string;
  content: string;
  status: string;
}

interface QuestionOption {
  label: string;
  description: string;
}

interface QuestionItem {
  header: string;
  question: string;
  options: QuestionOption[];
}

interface AskQuestionPayload {
  AskUserQuestion?: {
    session_id: string;
    question_id: string;
    questions: QuestionItem[];
  };
  TodoUpdate?: {
    session_id: string;
    todos: TodoItem[];
  };
  EditDiff?: {
    session_id: string;
    changes: FileChange[];
  };
  [key: string]: any;
}

interface AgentProgress {
  iteration: number;
  totalIterations: number;
  estimatedTokens: number;
  elapsedMs: number;
  status: string;
}

interface LogEntry {
  level: string;
  message: string;
  timestamp: number;
}

interface FileChange {
  file_path: string;
  hunks: DiffHunk[];
}

interface DiffHunk {
  type: "added" | "removed" | "unchanged";
  content: string;
  old_start: number;
  new_start: number;
}

// ── Thinking Bubble (collapsible) ─────────────────────────────────────────

function ThinkingBubble({ content }: { content: string }) {
  const [expanded, setExpanded] = useState(false);

  return (
    <div className="thinking-bubble">
      <div
        className="thinking-bubble-header"
        onClick={() => setExpanded(!expanded)}
        style={{ cursor: "pointer", display: "flex", alignItems: "center", gap: 6, userSelect: "none" }}
      >
        <span style={{ transform: expanded ? "rotate(90deg)" : "rotate(0deg)", transition: "transform 0.15s", display: "inline-flex" }}>
          <ChevronRight size={12} />
        </span>
        <Sparkles size={13} />
        <span>Reasoning</span>
        <span style={{ fontSize: 11, color: "var(--text-muted)", marginLeft: "auto" }}>
          {content.length} chars
        </span>
      </div>
      {expanded && (
        <div
          className="thinking-bubble-content"
          style={{
            maxHeight: 400,
            overflow: "auto",
            fontSize: 12,
            lineHeight: 1.5,
            whiteSpace: "pre-wrap",
            wordBreak: "break-word",
          }}
        >
          {content}
        </div>
      )}
    </div>
  );
}

function TodoListCard({ todos }: { todos: TodoItem[] }) {
  const total = todos.length;
  const complete = todos.filter((t) => t.status === "complete").length;
  const inProgress = todos.filter((t) => t.status === "in_progress").length;
  const pending = todos.filter((t) => t.status === "pending").length;
  const cancelled = todos.filter((t) => t.status === "cancelled").length;

  const statusIcon = (s: string) =>
    s === "complete" ? <CheckCircle2 size={12} color="var(--success)" />
    : s === "in_progress" ? <Clock size={12} color="var(--warning)" />
    : s === "cancelled" ? <XCircle size={12} color="var(--error)" />
    : <Circle size={12} color="var(--text-muted)" />;

  return (
    <div className="todo-list-card">
      <div className="todo-list-header">
        <span className="todo-list-title"><ClipboardList size={13} /> Task List</span>
        <span className="todo-list-summary">
          {complete}/{total} done · {inProgress} active · {pending} pending
          {cancelled > 0 && ` · ${cancelled} cancelled`}
        </span>
      </div>
      {total > 0 && (
        <div className="todo-progress-bar">
          <div className="todo-progress-fill" style={{ width: `${total > 0 ? ((complete + cancelled) / total) * 100 : 0}%` }} />
        </div>
      )}
      <div className="todo-list-items">
        {todos.map((t) => (
          <div key={t.id} className={`todo-item status-${t.status}`}>
            <span className="todo-item-icon">{statusIcon(t.status)}</span>
            <span className="todo-item-content">{t.content}</span>
            <span className="todo-item-status">{t.status.replace("_", " ")}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function AskQuestionDialog({
  payload,
  onAnswer,
  onDismiss,
}: {
  payload: NonNullable<AskQuestionPayload["AskUserQuestion"]>;
  onAnswer: (answers: string[]) => void;
  onDismiss: () => void;
}) {
  const [answers, setAnswers] = useState<Record<number, string>>({});

  function handleSubmit() {
    const result = payload.questions.map((_, i) => answers[i] || "");
    onAnswer(result);
  }

  return (
    <div className="ask-question-overlay" onClick={onDismiss}>
      <div className="ask-question-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="ask-question-header">
          <h3><HelpCircle size={15} /> Agent needs your input</h3>
          <button className="ask-question-close" onClick={onDismiss}><X size={14} /></button>
        </div>
        <div className="ask-question-body">
          {payload.questions.map((q, i) => (
            <div key={i} className="ask-question-item">
              <div className="ask-question-label">{q.header || `Question ${i + 1}`}</div>
              <div className="ask-question-text">{q.question}</div>
              {q.options && q.options.length > 0 ? (
                <div className="ask-question-options">
                  {q.options.map((opt, j) => (
                    <label key={j} className={`ask-question-option ${answers[i] === opt.label ? "selected" : ""}`}>
                      <input
                        type="radio"
                        name={`q-${i}`}
                        value={opt.label}
                        checked={answers[i] === opt.label}
                        onChange={() => setAnswers((prev) => ({ ...prev, [i]: opt.label }))}
                      />
                      <span className="ask-option-label">{opt.label}</span>
                      <span className="ask-option-desc">{opt.description}</span>
                    </label>
                  ))}
                </div>
              ) : (
                <textarea
                  className="ask-question-input"
                  rows={2}
                  placeholder="Type your answer..."
                  value={answers[i] || ""}
                  onChange={(e) => setAnswers((prev) => ({ ...prev, [i]: e.target.value }))}
                />
              )}
            </div>
          ))}
        </div>
        <div className="ask-question-footer">
          <button className="ask-question-cancel" onClick={onDismiss}>Cancel</button>
          <button className="ask-question-submit" onClick={handleSubmit}>Submit</button>
        </div>
      </div>
    </div>
  );
}

function ConfirmDangerDialog({
  payload,
  onAllow,
  onDeny,
}: {
  payload: NonNullable<ToolCallPayload["ConfirmRequest"]>;
  onAllow: () => void;
  onDeny: () => void;
}) {
  return (
    <div className="ask-question-overlay" onClick={onDeny}>
      <div className="ask-question-dialog confirm-danger-dialog" onClick={(e) => e.stopPropagation()}>
        <div className="ask-question-header">
          <h3><AlertTriangle size={15} /> Dangerous Operation</h3>
        </div>
        <div className="ask-question-body">
          <div className="confirm-danger-desc">
            <div className="confirm-danger-tool">{payload.tool_name}</div>
            <div className="confirm-danger-detail">{payload.description}</div>
          </div>
          <p className="confirm-danger-warning">This action may be irreversible. Do you want to proceed?</p>
        </div>
        <div className="ask-question-footer">
          <button className="ask-question-cancel" onClick={onDeny}>Deny</button>
          <button className="confirm-danger-allow" onClick={onAllow}>Allow</button>
        </div>
      </div>
    </div>
  );
}

function CopyButton({ code }: { code: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="code-copy-btn"
      onClick={() => {
        navigator.clipboard.writeText(code);
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      }}
    >
      {copied ? <><Check size={12} /> Copied</> : <><Copy size={12} /> Copy</>}
    </button>
  );
}

// ── Plan Mode Types ──────────────────────────────────────────────────

interface PlanStep {
  order: number;
  description: string;
  file_path?: string;
  tool_hint?: string;
}

interface PlanCardPayload {
  plan: {
    session_id: string;
    agent_id?: string;
    plan_summary: string;
    plan_steps: PlanStep[];
    affected_files: string[];
  };
}

// ── PlanCard (approval card shown during Planning phase) ────────────

function PlanCard({
  payload,
  onApprove,
  onReject,
  onSkip,
  disabled,
}: {
  payload: PlanCardPayload;
  onApprove: () => void;
  onReject: (reason: string) => void;
  onSkip: () => void;
  disabled: boolean;
}) {
  const [rejectReason, setRejectReason] = useState("");
  const [showRejectInput, setShowRejectInput] = useState(false);

  const plan = payload.plan;
  const sessionId = plan.session_id;

  function handleApprove() {
    onApprove();
  }

  function handleReject() {
    if (showRejectInput) {
      onReject(rejectReason);
      setShowRejectInput(false);
      setRejectReason("");
    } else {
      setShowRejectInput(true);
    }
  }

  function handleSkip() {
    onSkip();
  }

  return (
    <div className="plan-card">
      <div className="plan-card-header">
        <span className="plan-card-icon"><ClipboardList size={15} /></span>
        <span className="plan-card-title">Execution Plan</span>
        <span className="plan-card-session">#{sessionId.substring(0, 8)}</span>
      </div>

      <div className="plan-card-body">
        {plan.plan_summary && (
          <div className="plan-card-summary">
            <div className="plan-card-section-title">Summary</div>
            <p>{plan.plan_summary}</p>
          </div>
        )}

        {plan.plan_steps.length > 0 && (
          <div className="plan-card-steps">
            <div className="plan-card-section-title">
              Steps ({plan.plan_steps.length})
            </div>
            <ol className="plan-step-list">
              {plan.plan_steps.map((step, i) => (
                <li key={i} className="plan-step-item">
                  <span className="plan-step-order">{step.order}</span>
                  <span className="plan-step-desc">{step.description}</span>
                  {step.file_path && (
                    <span className="plan-step-file"><FileText size={12} /> {step.file_path}</span>
                  )}
                </li>
              ))}
            </ol>
          </div>
        )}

        {plan.affected_files.length > 0 && (
          <div className="plan-card-files">
            <div className="plan-card-section-title">
              Affected Files ({plan.affected_files.length})
            </div>
            <div className="plan-file-chips">
              {plan.affected_files.map((f, i) => (
                <span key={i} className="plan-file-chip">{f}</span>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className="plan-card-actions">
        {showRejectInput && (
          <div className="plan-reject-input-area">
            <input
              type="text"
              className="plan-reject-input"
              placeholder="Reason for rejection (optional)..."
              value={rejectReason}
              onChange={(e) => setRejectReason(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleReject();
                if (e.key === "Escape") {
                  setShowRejectInput(false);
                  setRejectReason("");
                }
              }}
              autoFocus
              disabled={disabled}
            />
          </div>
        )}
        <div className="plan-card-buttons">
          <button
            className="plan-btn plan-btn-approve"
            onClick={handleApprove}
            disabled={disabled}
          >
            <Check size={14} /> Approve
          </button>
          <button
            className="plan-btn plan-btn-reject"
            onClick={handleReject}
            disabled={disabled}
          >
            {showRejectInput ? "✗ Confirm Reject" : "✗ Reject"}
          </button>
          <button
            className="plan-btn plan-btn-skip"
            onClick={handleSkip}
            disabled={disabled}
          >
            ⏭ Skip
          </button>
        </div>
      </div>
    </div>
  );
}

function DiffCard({ changes }: { changes: FileChange[] }) {
  const [fileStates, setFileStates] = useState<Record<string, "pending" | "accepted" | "rejected">>({});
  const [expandedFiles, setExpandedFiles] = useState<Set<string>>(new Set(changes.map(c => c.file_path)));
  const [batchState, setBatchState] = useState<"pending" | "accepted" | "rejected">("pending");

  useEffect(() => {
    // Reset states when changes update
    const initial: Record<string, "pending" | "accepted" | "rejected"> = {};
    changes.forEach((fc) => {
      initial[fc.file_path] = "pending";
    });
    setFileStates(initial);
    setExpandedFiles(new Set(changes.map(c => c.file_path)));
    setBatchState("pending");
  }, [changes]);

  async function handleAccept(filePath: string) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("accept_change", { filePath });
      setFileStates((prev) => ({ ...prev, [filePath]: "accepted" }));
    } catch (e) {
      console.error("Failed to accept:", e);
    }
  }

  async function handleReject(filePath: string) {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("reject_change", { filePath });
      setFileStates((prev) => ({ ...prev, [filePath]: "rejected" }));
    } catch (e) {
      console.error("Failed to reject:", e);
    }
  }

  async function handleAcceptAll() {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const count = await invoke<number>("accept_all_changes");
      const allAccepted: Record<string, "accepted"> = {};
      changes.forEach(c => { allAccepted[c.file_path] = "accepted"; });
      setFileStates(allAccepted);
      setBatchState("accepted");
      console.log(`Accepted ${count} files`);
    } catch (e) {
      console.error("Failed to accept all:", e);
    }
  }

  async function handleRejectAll() {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const count = await invoke<number>("reject_all_changes");
      const allRejected: Record<string, "rejected"> = {};
      changes.forEach(c => { allRejected[c.file_path] = "rejected"; });
      setFileStates(allRejected);
      setBatchState("rejected");
      console.log(`Rejected ${count} files`);
    } catch (e) {
      console.error("Failed to reject all:", e);
    }
  }

  function toggleFile(filePath: string) {
    setExpandedFiles(prev => {
      const next = new Set(prev);
      if (next.has(filePath)) next.delete(filePath);
      else next.add(filePath);
      return next;
    });
  }

  // Compute per-file stats
  function computeStats(hunks: DiffHunk[]) {
    let added = 0, removed = 0;
    hunks.forEach(h => {
      if (h.type === "added") added++;
      else if (h.type === "removed") removed++;
    });
    return { added, removed };
  }

  // Filter hunks to show only changes with context (hide long unchanged sections)
  function filterHunks(hunks: DiffHunk[], contextLines = 3): DiffHunk[] {
    const result: DiffHunk[] = [];
    let unchangedBuffer: DiffHunk[] = [];
    let lastChangeIdx = -1;

    hunks.forEach((hunk, idx) => {
      if (hunk.type === "unchanged") {
        unchangedBuffer.push(hunk);
      } else {
        // Flush context before this change
        if (unchangedBuffer.length > 0) {
          if (lastChangeIdx >= 0) {
            // Show trailing context from previous change
            const trailing = unchangedBuffer.slice(0, Math.min(contextLines, unchangedBuffer.length));
            result.push(...trailing);
          }
          // Show separator if there are hidden unchanged lines
          if (unchangedBuffer.length > contextLines * 2 && lastChangeIdx >= 0) {
            result.push({
              type: "unchanged",
              content: `... ${unchangedBuffer.length - contextLines * 2} lines hidden ...`,
              old_start: 0,
              new_start: 0,
            } as DiffHunk);
          }
          // Show leading context for this change
          if (lastChangeIdx >= 0) {
            const leading = unchangedBuffer.slice(Math.max(0, unchangedBuffer.length - contextLines));
            result.push(...leading);
          } else {
            // First change: show leading context
            const leading = unchangedBuffer.slice(Math.max(0, unchangedBuffer.length - contextLines));
            result.push(...leading);
          }
          unchangedBuffer = [];
        }
        result.push(hunk);
        lastChangeIdx = idx;
      }
    });

    // Flush trailing unchanged lines at the end
    if (unchangedBuffer.length > 0 && lastChangeIdx >= 0) {
      const trailing = unchangedBuffer.slice(0, Math.min(contextLines, unchangedBuffer.length));
      result.push(...trailing);
      if (unchangedBuffer.length > contextLines) {
        result.push({
          type: "unchanged",
          content: `... ${unchangedBuffer.length - contextLines} lines hidden ...`,
          old_start: 0,
          new_start: 0,
        } as DiffHunk);
      }
    }

    return result;
  }

  const pendingCount = Object.values(fileStates).filter(s => s === "pending").length;
  const hasAnyChanges = changes.length > 0;

  return (
    <div className="diff-card">
      <div className="diff-card-header">
        <div className="diff-card-header-left">
          <span className="diff-card-title">📝 File Changes</span>
          <span className="diff-card-count">{changes.length} file{changes.length !== 1 ? "s" : ""} modified</span>
        </div>
        {hasAnyChanges && pendingCount > 0 && (
          <div className="diff-batch-actions">
            <button className="diff-batch-btn diff-batch-accept" onClick={handleAcceptAll} title="Accept all changes">
              ✓ Accept All
            </button>
            <button className="diff-batch-btn diff-batch-reject" onClick={handleRejectAll} title="Reject all changes">
              ✕ Reject All
            </button>
          </div>
        )}
        {batchState !== "pending" && (
          <span className={`diff-batch-state ${batchState}`}>
            {batchState === "accepted" ? "✓ All accepted" : "✕ All rejected"}
          </span>
        )}
      </div>
      <div className="diff-file-list">
        {changes.map((fc, idx) => {
          const state = fileStates[fc.file_path] || "pending";
          const stats = computeStats(fc.hunks);
          const isExpanded = expandedFiles.has(fc.file_path);
          const fileName = fc.file_path.split(/[\\/]/).pop() || fc.file_path;
          const displayHunks = filterHunks(fc.hunks);

          return (
            <div key={idx} className={`diff-file ${state !== "pending" ? `diff-file-${state}` : ""}`}>
              <div className="diff-file-header" onClick={() => toggleFile(fc.file_path)}>
                <span className="diff-file-toggle">{isExpanded ? "▼" : "▶"}</span>
                <span className="diff-file-path" title={fc.file_path}>{fileName}</span>
                <span className="diff-file-stats">
                  {stats.added > 0 && <span className="diff-stat-added">+{stats.added}</span>}
                  {stats.removed > 0 && <span className="diff-stat-removed">-{stats.removed}</span>}
                </span>
                <div className="diff-file-actions" onClick={(e) => e.stopPropagation()}>
                  {state === "pending" && (
                    <>
                      <button className="diff-btn-accept" onClick={() => handleAccept(fc.file_path)} title="Keep changes">✓</button>
                      <button className="diff-btn-reject" onClick={() => handleReject(fc.file_path)} title="Restore original">✕</button>
                    </>
                  )}
                  {state === "accepted" && <span className="diff-state-label accepted">✓ Accepted</span>}
                  {state === "rejected" && <span className="diff-state-label rejected">✕ Rejected</span>}
                </div>
              </div>
              {isExpanded && state !== "rejected" && (
                <div className="diff-hunks">
                  {displayHunks.map((hunk, hi) => (
                    <div
                      key={hi}
                      className={`diff-line diff-line-${hunk.type}`}
                    >
                      <span className="diff-line-prefix">
                        {hunk.type === "added" ? "+" : hunk.type === "removed" ? "-" : hunk.content.startsWith("...") ? "" : " "}
                      </span>
                      <span className="diff-line-num">
                        {hunk.old_start > 0 ? hunk.old_start : ""}
                        {hunk.old_start > 0 && hunk.new_start > 0 ? "→" : ""}
                        {hunk.new_start > 0 ? hunk.new_start : ""}
                      </span>
                      <code>{hunk.content}</code>
                    </div>
                  ))}
                </div>
              )}
              {isExpanded && state === "rejected" && (
                <div className="diff-rejected-msg">Changes have been reverted</div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function AgentProgressBar({ progress }: { progress: AgentProgress | null }) {
  if (!progress) return null;
  const pct = progress.totalIterations > 0
    ? Math.round((progress.iteration / progress.totalIterations) * 100)
    : 0;
  const elapsedStr = progress.elapsedMs >= 60000
    ? `${(progress.elapsedMs / 60000).toFixed(1)}m`
    : `${Math.round(progress.elapsedMs / 1000)}s`;
  return (
    <div className="agent-progress-bar">
      <div className="agent-progress-header">
        <span className="agent-progress-status">{progress.status}</span>
        <span className="agent-progress-metrics">
          Step {progress.iteration + 1}/{progress.totalIterations}
          {progress.estimatedTokens > 0 && ` · ~${progress.estimatedTokens} tokens`}
          {progress.elapsedMs > 0 && ` · ${elapsedStr}`}
        </span>
      </div>
      <div className="agent-progress-track">
        <div className="agent-progress-fill" style={{ width: `${Math.min(pct, 100)}%` }} />
      </div>
    </div>
  );
}

function LogPanel({ logs, open, onToggle }: { logs: LogEntry[]; open: boolean; onToggle: () => void }) {
  return (
    <div className={`agent-log-panel ${open ? "open" : ""}`}>
      <div className="agent-log-header" onClick={onToggle}>
        <span>{open ? "▼" : "▶"} Agent Log ({logs.length})</span>
      </div>
      {open && (
        <div className="agent-log-body">
          {logs.length === 0 && <div className="agent-log-empty">No log entries yet</div>}
          {logs.map((entry, i) => (
            <div key={i} className={`agent-log-entry level-${entry.level}`}>
              <span className="agent-log-level">[{entry.level}]</span>
              <span className="agent-log-msg">{entry.message}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function ToolCard({ toolName, arguments: args, result, isRunning, durationMs, isRetry }: {
  toolName: string;
  arguments?: Record<string, any>;
  result?: string;
  isRunning?: boolean;
  durationMs?: number;
  isRetry?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(true);

  const toolIcon = toolName === "read_file" ? "📖"
    : toolName === "write_file" ? "✏️"
    : toolName === "append_file" ? "➕"
    : toolName === "delete_file" ? "🗑️"
    : toolName === "list_directory" ? "📂"
    : toolName === "create_directory" ? "📁"
    : toolName === "delete_directory" ? "🔥"
    : toolName === "get_symbols" ? "🔍"
    : "🔧";

  const durationStr = durationMs !== undefined && durationMs > 0
    ? durationMs >= 1000
      ? `${(durationMs / 1000).toFixed(1)}s`
      : `${durationMs}ms`
    : null;

  return (
    <div className={`tool-card ${isRunning ? "running" : "done"} ${isRetry ? "retry" : ""}`}>
      <div className="tool-card-header">
        <span className="tool-card-icon">{isRunning ? "⏳" : isRetry ? "⚠️" : "✓"}</span>
        <span className="tool-card-name">{toolIcon} {toolName}</span>
        {durationStr && <span className="tool-card-duration">{durationStr}</span>}
        {isRunning && <span className="tool-card-spinner" />}
      </div>
      {isRetry && <div className="tool-card-retry-badge">Auto-retry after failure</div>}
      {args && Object.keys(args).length > 0 && (
        <div className="tool-card-args">
          {Object.entries(args).slice(0, 3).map(([key, val]) => (
            <span key={key} className="tool-card-arg">
              <span className="tool-card-arg-key">{key}:</span>
              <span className="tool-card-arg-val">{String(val).substring(0, 80)}</span>
            </span>
          ))}
        </div>
      )}
      {result !== undefined && (
        <div className="tool-card-result">
          <button
            className="tool-card-toggle"
            onClick={() => setCollapsed(!collapsed)}
          >
            {collapsed ? "▶" : "▼"} Result
          </button>
          {!collapsed && (
            <pre className="tool-card-result-content">
              <code>{result.substring(0, 2000)}{result.length > 2000 ? "..." : ""}</code>
            </pre>
          )}
        </div>
      )}
    </div>
  );
}

// ── Edit Code Block with Diff Preview ──────────────────────────────────────

function EditCodeBlock({ fileLang, filePath, codeString }: { fileLang: string; filePath: string; codeString: string }) {
  const [diffState, setDiffState] = useState<"idle" | "loading" | "reviewing" | "applied" | "error">("idle");
  const [originalContent, setOriginalContent] = useState("");

  const handleApplyClick = async () => {
    try {
      setDiffState("loading");
      const current = await readFile(filePath);
      if (current === null) {
        // File doesn't exist — write directly (new file)
        const success = await writeFile(filePath, codeString);
        if (!success) throw new Error("Write file failed");
        setDiffState("applied");
        setTimeout(() => setDiffState("idle"), 2000);
        return;
      }
      // Show diff preview
      setOriginalContent(current);
      setDiffState("reviewing");
    } catch (err) {
      console.error("Failed to read file for diff:", err);
      setDiffState("error");
      setTimeout(() => setDiffState("idle"), 2000);
    }
  };

  const handleAccept = async () => {
    try {
      const success = await writeFile(filePath, codeString);
      if (!success) throw new Error("Write file failed");
      setDiffState("applied");
      setTimeout(() => setDiffState("idle"), 2000);
    } catch (err) {
      console.error("Failed to apply edit:", err);
      setDiffState("error");
      setTimeout(() => setDiffState("idle"), 2000);
    }
  };

  const handleReject = () => {
    setDiffState("idle");
  };

  // Compute simple line-level diff for review
  const computeDiff = () => {
    const oldLines = originalContent.split("\n");
    const newLines = codeString.split("\n");
    const maxLen = Math.max(oldLines.length, newLines.length);
    const diffs: { type: "same" | "removed" | "added" | "changed"; oldLine?: string; newLine?: string; lineNum: number }[] = [];
    for (let i = 0; i < maxLen; i++) {
      const ol = oldLines[i];
      const nl = newLines[i];
      if (ol === nl) {
        diffs.push({ type: "same", oldLine: ol, newLine: nl, lineNum: i + 1 });
      } else if (ol === undefined) {
        diffs.push({ type: "added", newLine: nl, lineNum: i + 1 });
      } else if (nl === undefined) {
        diffs.push({ type: "removed", oldLine: ol, lineNum: i + 1 });
      } else {
        diffs.push({ type: "changed", oldLine: ol, newLine: nl, lineNum: i + 1 });
      }
    }
    return diffs;
  };

  const btnLabel = diffState === "loading" ? "..." : diffState === "applied" ? "✓ Applied" : diffState === "error" ? "Failed" : "Apply";
  const btnClass = `edit-apply-btn${diffState === "applied" ? " applied" : diffState === "error" ? " error" : ""}`;

  return (
    <div className="code-block-wrapper edit-code-block">
      <div className="edit-code-block-header">
        <span className="code-lang">{fileLang}</span>
        <span className="edit-file-path">{filePath}</span>
        {diffState !== "reviewing" && (
          <button className={btnClass} onClick={handleApplyClick} disabled={diffState === "loading"}>
            {btnLabel}
          </button>
        )}
        {diffState === "reviewing" && (
          <>
            <button className="edit-apply-btn" style={{ background: "var(--success, #a6e3a1)", color: "#1e1e2e" }} onClick={handleAccept}>
              ✓ Accept
            </button>
            <button className="edit-apply-btn" style={{ background: "var(--error, #f38ba8)", color: "#1e1e2e" }} onClick={handleReject}>
              ✕ Reject
            </button>
          </>
        )}
      </div>

      {diffState === "reviewing" ? (
        <div className="edit-diff-view" style={{ maxHeight: 400, overflow: "auto", fontFamily: "monospace", fontSize: 12, lineHeight: 1.6 }}>
          {computeDiff().map((d, i) => {
            if (d.type === "same") {
              return (
                <div key={i} style={{ display: "flex", background: "transparent", borderBottom: "1px solid var(--border-subtle, #21212e)" }}>
                  <span style={{ width: 36, textAlign: "right", padding: "1px 6px", color: "var(--text-muted, #6c7086)", userSelect: "none", flexShrink: 0, fontSize: 11 }}>{d.lineNum}</span>
                  <pre style={{ flex: 1, margin: 0, padding: "1px 8px", color: "var(--text-muted, #6c7086)", whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{d.newLine || " "}</pre>
                </div>
              );
            }
            if (d.type === "changed") {
              return (
                <Fragment key={i}>
                  <div style={{ display: "flex", background: "rgba(243, 139, 168, 0.12)", borderBottom: "1px solid var(--border-subtle, #21212e)" }}>
                    <span style={{ width: 36, textAlign: "right", padding: "1px 6px", color: "var(--error, #f38ba8)", userSelect: "none", flexShrink: 0, fontSize: 11 }}>{d.lineNum}</span>
                    <pre style={{ flex: 1, margin: 0, padding: "1px 8px", color: "var(--error, #f38ba8)", whiteSpace: "pre-wrap", wordBreak: "break-word" }}>- {d.oldLine}</pre>
                  </div>
                  <div style={{ display: "flex", background: "rgba(166, 227, 161, 0.12)", borderBottom: "1px solid var(--border-subtle, #21212e)" }}>
                    <span style={{ width: 36, textAlign: "right", padding: "1px 6px", color: "var(--success, #a6e3a1)", userSelect: "none", flexShrink: 0, fontSize: 11 }}>+</span>
                    <pre style={{ flex: 1, margin: 0, padding: "1px 8px", color: "var(--success, #a6e3a1)", whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{d.newLine}</pre>
                  </div>
                </Fragment>
              );
            }
            if (d.type === "removed") {
              return (
                <div key={i} style={{ display: "flex", background: "rgba(243, 139, 168, 0.12)", borderBottom: "1px solid var(--border-subtle, #21212e)" }}>
                  <span style={{ width: 36, textAlign: "right", padding: "1px 6px", color: "var(--error, #f38ba8)", userSelect: "none", flexShrink: 0, fontSize: 11 }}>{d.lineNum}</span>
                  <pre style={{ flex: 1, margin: 0, padding: "1px 8px", color: "var(--error, #f38ba8)", whiteSpace: "pre-wrap", wordBreak: "break-word" }}>- {d.oldLine}</pre>
                </div>
              );
            }
            // added
            return (
              <div key={i} style={{ display: "flex", background: "rgba(166, 227, 161, 0.12)", borderBottom: "1px solid var(--border-subtle, #21212e)" }}>
                <span style={{ width: 36, textAlign: "right", padding: "1px 6px", color: "var(--success, #a6e3a1)", userSelect: "none", flexShrink: 0, fontSize: 11 }}>+</span>
                <pre style={{ flex: 1, margin: 0, padding: "1px 8px", color: "var(--success, #a6e3a1)", whiteSpace: "pre-wrap", wordBreak: "break-word" }}>{d.newLine}</pre>
              </div>
            );
          })}
        </div>
      ) : (
        <SyntaxHighlighterWrapper language={fileLang} code={codeString} />
      )}
    </div>
  );
}

const markdownComponents: Components = {
  code({ className, children, ...props }) {
    const match = /language-(\w+):(.+)/.exec(className || "");
    const codeString = String(children).replace(/\n$/, "");

    // Edit mode: code block with file path (```language:path/to/file)
    if (match) {
      const fileLang = match[1];
      const filePath = match[2].trim();
      return <EditCodeBlock fileLang={fileLang} filePath={filePath} codeString={codeString} />;
    }

    const langMatch = /language-(\w+)/.exec(className || "");
    if (langMatch) {
      return (
        <div className="code-block-wrapper">
          <div className="code-block-header">
            <span className="code-lang">{langMatch[1]}</span>
            <CopyButton code={codeString} />
          </div>
          <SyntaxHighlighterWrapper
            language={langMatch[1]}
            code={codeString}
          />
        </div>
      );
    }

    return (
      <code className="inline-code" {...props}>
        {children}
      </code>
    );
  },
};

export default function ChatPanel({ projectPath }: { projectPath?: string | null }) {
  const [messages, setMessages] = useState<Message[]>([]);
  const [input, setInput] = useState("");
  const [sessionId, setSessionId] = useState<string>("");
  const [mode, setMode] = useState<"ask" | "edit" | "agent">("ask");
  const [agentId, setAgentId] = useState<string>("orchestrator");
  const [agents, setAgents] = useState<AgentDefinition[]>([]);
  const [streaming, setStreaming] = useState(false);
  const [todoList, setTodoList] = useState<TodoItem[]>([]);
  const [fileChanges, setFileChanges] = useState<FileChange[]>([]);
  const [questionDialog, setQuestionDialog] = useState<AskQuestionPayload["AskUserQuestion"] | null>(null);
  const [agentProgress, setAgentProgress] = useState<AgentProgress | null>(null);
  const [paused, setPaused] = useState(false);
  const [agentLogs, setAgentLogs] = useState<LogEntry[]>([]);
  const [logPanelOpen, setLogPanelOpen] = useState(false);
  const [thinkingText, setThinkingText] = useState("");
  const [trimNotice, setTrimNotice] = useState<string | null>(null);
  const [confirmDialog, setConfirmDialog] = useState<ToolCallPayload["ConfirmRequest"] | null>(null);
  const [planDialog, setPlanDialog] = useState<PlanCardPayload | null>(null);
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [showSessionList, setShowSessionList] = useState(false);
  const [errorMessage, setErrorMessage] = useState<string | null>(null);
  const [planMode, setPlanMode] = useState(false);
  const [budgetExhausted, setBudgetExhausted] = useState<{ summary: string; maxIterations: number } | null>(null);

  const getEffectiveProjectPath = useCallback(() => {
    const editor = (window as any).__neecoder_editor;
    return editor?.getProjectPath?.() || projectPath || "";
  }, [projectPath]);

  // ── Mention menu state ──
  const [mentionItems, setMentionItems] = useState<MentionItem[]>([]);
  const [mentionIndex, setMentionIndex] = useState(0);
  const [mentionVisible, setMentionVisible] = useState(false);
  const [mentionTriggerPos, setMentionTriggerPos] = useState<{ top: number; left: number }>({ top: 60, left: 0 });
  const [mentionQuery, setMentionQuery] = useState("");
  const [mentionTriggerStart, setMentionTriggerStart] = useState(-1); // position of @ or # in input
  const [attachedFiles, setAttachedFiles] = useState<FlatFileItem[]>([]);
  const [attachedImages, setAttachedImages] = useState<string[]>([]);
  const [editingIndex, setEditingIndex] = useState<number | null>(null);
  const [editContent, setEditContent] = useState("");
  const [projectFiles, setProjectFiles] = useState<FlatFileItem[]>([]);
  const [dragOver, setDragOver] = useState(false);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const mentionDebounceRef = useRef<number>(0);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  // ── Skill state ──
  const [skills, setSkills] = useState<SkillDefinition[]>([]);
  const [skillMenuVisible, setSkillMenuVisible] = useState(false);
  const [skillMenuIndex, setSkillMenuIndex] = useState(0);
  const [skillMenuItems, setSkillMenuItems] = useState<SkillDefinition[]>([]);
  const [skillTriggerStart, setSkillTriggerStart] = useState(-1);

  // Create session on mount & load session history
  useEffect(() => {
    createNewSession().then((id) => {
      if (id) setSessionId(id);
    });
    // Load available agents
    getAgents().then((list) => {
      if (list.length > 0) setAgents(list);
    });
    // Load session history
    getSessions().then((list) => {
      if (list.length > 0) setSessions(list);
    });
    // Load project file tree for mention menu
    getFileTree(".").then((tree) => {
      if (tree.length > 0) setProjectFiles(flattenFileTree(tree));
    }).catch(() => { /* ignore */ });
    // Load available skills
    listSkills().then(setSkills).catch(() => { /* ignore */ });
  }, []);

  // Auto-scroll
  useEffect(() => {
    messagesEndRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages]);

  // Listen for streaming events
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;

    async function setup() {
      unlisten = await listenToEvent<any>("chat-event", (payload: ToolCallPayload) => {
        // ── ToolRetry 事件 ──
        if (payload?.ToolRetry) {
          setMessages((prev) => {
            const updated = [...prev];
            for (let i = updated.length - 1; i >= 0; i--) {
              if (updated[i].role === "tool" && updated[i].toolName === payload.ToolRetry?.tool_name && !updated[i].isRunning) {
                updated[i] = { ...updated[i], isRetry: true, retryAttempt: payload.ToolRetry?.attempt };
                break;
              }
            }
            return updated;
          });
          return;
        }

        // ── AgentThinking 事件 ──
        if (payload?.AgentThinking) {
          const thought = payload.AgentThinking.thought;
          setThinkingText((prev) => prev + thought);
          return;
        }

        // ── AgentLog 事件 ──
        if (payload?.AgentLog) {
          setAgentLogs((prev) => [...prev, {
            level: payload.AgentLog!.level,
            message: payload.AgentLog!.message,
            timestamp: Date.now(),
          }]);
          return;
        }

        // ── ContextTrimmed 事件 ──
        if (payload?.ContextTrimmed) {
          const ct = payload.ContextTrimmed;
          setTrimNotice(`Context trimmed: ${ct.total_before} → ${ct.total_after} messages (removed ${ct.trimmed_count})`);
          setTimeout(() => setTrimNotice(null), 5000);
          return;
        }

        // ── ConfirmRequest 事件（危险操作确认） ──
        if (payload?.ConfirmRequest) {
          setConfirmDialog(payload.ConfirmRequest);
          return;
        }

        // ── ToolCall 事件 ──
        const toolCallData = payload?.ToolCall?.tool_call || payload?.tool_call;
        if (toolCallData) {
          // Flush accumulated thinking text as a thinking message
          setThinkingText((prev) => {
            if (prev.trim()) {
              setMessages((msgs) => [...msgs, { role: "thinking", content: prev.trim() }]);
            }
            return "";
          });
          setMessages((prev) => [...prev, {
            role: "tool",
            content: "",
            toolName: toolCallData.tool_name,
            toolCalls: [toolCallData],
            isRunning: true,
            timestamp: (toolCallData as any).timestamp || Date.now(),
          }]);
          return;
        }

        // ── ToolResult 事件 ──
        const toolResultStr = payload?.ToolResult?.result ?? payload?.result ?? payload?.tool_result;
        if (toolResultStr !== undefined && toolResultStr !== null) {
          setMessages((prev) => {
            const updated = [...prev];
            for (let i = updated.length - 1; i >= 0; i--) {
              if (updated[i].role === "tool" && updated[i].isRunning) {
                updated[i] = {
                  ...updated[i],
                  toolResult: String(toolResultStr),
                  isRunning: false,
                  durationMs: payload?.ToolResult?.duration_ms || 0,
                };
                break;
              }
            }
            return updated;
          });
          return;
        }

        // ── AgentStatus 事件（带进度信息） ──
        if (payload?.AgentStatus) {
          // Handle both old and new format
          const s = payload.AgentStatus as any;
          setAgentProgress({
            iteration: s.iteration ?? 0,
            totalIterations: s.total_iterations ?? 10,
            estimatedTokens: s.estimated_tokens ?? 0,
            elapsedMs: s.elapsed_ms ?? 0,
            status: s.status ?? "",
          });
          // C1: track paused state from the agent main loop
          setPaused(s.status === "paused");
          return;
        }

        // ── EditDiff 事件 ──
        if (payload?.EditDiff) {
          setFileChanges(payload.EditDiff.changes);
          return;
        }

        // ── TodoUpdate 事件 ──
        if (payload?.TodoUpdate) {
          setTodoList(payload.TodoUpdate.todos);
          return;
        }

        // ── PlanCreated 事件（显示审批卡片） ──
        if (payload?.PlanCreated) {
          setPlanDialog(payload as PlanCardPayload);
          return;
        }

        // ── PlanApproved / PlanRejected 事件（关闭审批卡片） ──
        if (payload?.PlanApproved || payload?.PlanRejected) {
          setPlanDialog(null);
          return;
        }

        // ── AskUserQuestion 事件 ──
        if (payload?.AskUserQuestion) {
          setQuestionDialog(payload.AskUserQuestion);
          return;
        }

        // ── Delta 事件（流式 token） ──
        const deltaToken = payload?.Delta?.token ?? payload?.token;
        if (deltaToken) {
          // Check if this is thinking content (prefixed with [THINKING] by backend)
          if (deltaToken.startsWith("[THINKING]")) {
            const thinkingContent = deltaToken.slice("[THINKING]".length);
            setThinkingText((prev) => prev + thinkingContent);
            return;
          }

          // If we were accumulating thinking text and now get regular text,
          // flush the thinking text as a thinking message first
          setThinkingText((prev) => {
            if (prev.trim()) {
              setMessages((msgs) => [...msgs, { role: "thinking", content: prev.trim() }]);
            }
            return "";
          });

          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.role === "assistant") {
              const updated = [...prev];
              updated[updated.length - 1] = {
                ...last,
                content: last.content + deltaToken,
              };
              return updated;
            }
            return [...prev, { role: "assistant" as const, content: deltaToken }];
          });
          return;
        }

        // ── Finished 事件 ──
        const finishedPayload = payload?.Finished ?? (payload?.full_text !== undefined ? payload : null);
        if (finishedPayload) {
          const fullText = finishedPayload.full_text ?? finishedPayload.Finished?.full_text ?? "";
          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.role === "assistant") {
              const updated = [...prev];
              updated[updated.length - 1] = {
                ...last,
                content: fullText || last.content,
              };
              return updated;
            }
            // If no assistant message yet but we got full_text, add one
            if (fullText) {
              return [...prev, { role: "assistant" as const, content: fullText }];
            }
            return prev;
          });
          setStreaming(false);
          setAgentProgress(null);
          return;
        }

        // ── BudgetExhausted 事件 ──
        const budgetPayload = payload?.BudgetExhausted;
        if (budgetPayload) {
          const summary = budgetPayload.summary || "";
          const maxIter = budgetPayload.max_iterations || 0;
          setMessages((prev) => {
            const last = prev[prev.length - 1];
            if (last?.role === "assistant") {
              const updated = [...prev];
              updated[updated.length - 1] = {
                ...last,
                content: summary || last.content,
              };
              return updated;
            }
            if (summary) {
              return [...prev, { role: "assistant" as const, content: summary }];
            }
            return prev;
          });
          setStreaming(false);
          setAgentProgress(null);
          setBudgetExhausted({ summary, maxIterations: maxIter });
          return;
        }

        // ── Error 事件 ──
        const errorPayload = payload?.Error;
        if (errorPayload) {
          const errMsg = errorPayload.message || "Unknown error";
          setMessages((prev) => [
            ...prev,
            { role: "assistant" as const, content: `⚠️ **Error**: ${errMsg}` },
          ]);
          setStreaming(false);
          setAgentProgress(null);
          return;
        }

        // ── Cancelled 事件 ──
        if (payload?.Cancelled) {
          setMessages((prev) => [
            ...prev,
            { role: "assistant" as const, content: "⏹️ Agent cancelled." },
          ]);
          setStreaming(false);
          setAgentProgress(null);
          return;
        }
      });
    }

    setup();
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // ── Tauri native file-drop event listener ──
  // Listens for files/directories dropped from the OS file explorer,
  // providing real filesystem paths (not browser File objects).
  useEffect(() => {
    let unlistenDrop: (() => void) | null = null;
    let unlistenHover: (() => void) | null = null;
    let unlistenCancel: (() => void) | null = null;

    async function setup() {
      try {
        const { listen } = await import("@tauri-apps/api/event");

        // File drop: extract real paths and add to attached files
        unlistenDrop = await listen<{ paths: string[] }>(
          "tauri://file-drop",
          (event) => {
            const paths = event.payload?.paths;
            if (!paths || paths.length === 0) return;

            const newItems: FlatFileItem[] = paths.map((p) => {
              const name = p.split(/[\\/]/).filter(Boolean).pop() || p;
              // Heuristic: no extension + path separator at end → likely a directory
              const isDir = !name.includes(".") || p.endsWith("/") || p.endsWith("\\");
              return { name, path: p, is_dir: isDir };
            });

            setAttachedFiles((prev) => {
              const existing = new Set(prev.map((f) => f.path));
              const unique = newItems.filter((item) => !existing.has(item.path));
              return [...prev, ...unique];
            });
            setDragOver(false);
          }
        );

        // File drop hover: show drag-over visual
        unlistenHover = await listen("tauri://file-drop-hover", () => {
          setDragOver(true);
        });

        // File drop cancelled: remove drag-over visual
        unlistenCancel = await listen("tauri://file-drop-cancelled", () => {
          setDragOver(false);
        });
      } catch {
        // Not in Tauri environment — ignore
      }
    }

    setup();
    return () => {
      unlistenDrop?.();
      unlistenHover?.();
      unlistenCancel?.();
    };
  }, []);

  // ── Mention menu logic ──
  const SPECIAL_COMMANDS: MentionItem[] = [
    { id: "codebase", label: "codebase", type: "codebase", description: "Search the entire codebase (RAG)" },
  ];

  const updateMentionMenu = useCallback((value: string, cursorPos: number) => {
    // Find the last @ or # before cursor
    const beforeCursor = value.slice(0, cursorPos);
    const lastAt = beforeCursor.lastIndexOf("@");
    const lastHash = beforeCursor.lastIndexOf("#");
    const triggerPos = Math.max(lastAt, lastHash);

    if (triggerPos === -1) {
      setMentionVisible(false);
      return;
    }

    // Check if trigger is at start or after whitespace
    if (triggerPos > 0 && !/\s/.test(value[triggerPos - 1])) {
      setMentionVisible(false);
      return;
    }

    const query = beforeCursor.slice(triggerPos + 1).toLowerCase();
    // If query contains space, close menu
    if (query.includes(" ") || query.includes("\n")) {
      setMentionVisible(false);
      return;
    }

    // Support `@file:line` references — strip the `:line` suffix for matching
    // (e.g. `chat:42` matches the file `chat` and references line 42)
    const lineRefMatch = query.match(/^(.+?):(\d+)$/);
    const matchQuery = lineRefMatch ? lineRefMatch[1] : query;
    const lineRef = lineRefMatch ? parseInt(lineRefMatch[2], 10) : undefined;

    setMentionQuery(query);
    setMentionTriggerStart(triggerPos);
    setMentionTriggerPos({ top: 60, left: Math.min(triggerPos * 7, 300) });
    setMentionIndex(0);

    // Build items list
    const items: MentionItem[] = [];

    // Add special commands
    for (const cmd of SPECIAL_COMMANDS) {
      if (cmd.label.toLowerCase().includes(query)) {
        items.push(cmd);
      }
    }

    // Add matching files AND directories (limit to 20)
    // Show directories first, then files — directories get a "folder" type
    const matched = projectFiles
      .filter((f) => f.name.toLowerCase().includes(matchQuery) || f.path.toLowerCase().includes(matchQuery))
      .sort((a, b) => {
        // Directories first
        if (a.is_dir && !b.is_dir) return -1;
        if (!a.is_dir && b.is_dir) return 1;
        return a.name.localeCompare(b.name);
      })
      .slice(0, 20);
    for (const f of matched) {
      items.push({
        id: f.path,
        label: f.name,
        type: f.is_dir ? "folder" : "file",
        description: f.path,
        path: f.path,
        line: lineRef,
      });
    }

    setMentionItems(items.slice(0, 15));
    setMentionVisible(items.length > 0);
  }, [projectFiles]);

  const handleMentionSelect = useCallback((item: MentionItem) => {
    const before = input.slice(0, mentionTriggerStart);
    const afterTrigger = input.slice(mentionTriggerStart + 1 + mentionQuery.length);

    if ((item.type === "file" || item.type === "folder") && item.path) {
      // Add to attached files (both files and directories use the same state)
      const isDir = item.type === "folder";
      const refLabel = item.line ? `${item.label}:${item.line}` : item.label;
      if (!attachedFiles.find((f) => f.path === item.path && f.line === item.line)) {
        setAttachedFiles((prev) => [...prev, { name: item.label, path: item.path!, is_dir: isDir, line: item.line }]);
      }
      // Replace @filename with chip marker in text
      setInput(before + `[${refLabel}] ` + afterTrigger);
    } else {
      // For commands like @codebase, just replace with the command text
      setInput(before + `@${item.label} ` + afterTrigger);
    }

    setMentionVisible(false);
    setMentionTriggerStart(-1);
    textareaRef.current?.focus();
  }, [input, mentionTriggerStart, mentionQuery, attachedFiles]);

  const handleInputChange = useCallback((e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const val = e.target.value;
    const cursorPos = e.target.selectionStart ?? val.length;
    setInput(val);

    // Debounce mention menu update
    if (mentionDebounceRef.current) clearTimeout(mentionDebounceRef.current);
    mentionDebounceRef.current = window.setTimeout(() => {
      // Check for slash command at start of line or after whitespace
      const beforeCursor = val.slice(0, cursorPos);
      const lastNewline = beforeCursor.lastIndexOf("\n");
      const lineStart = lastNewline + 1;
      const lineBeforeCursor = beforeCursor.slice(lineStart);

      // Slash command: line starts with / and no space yet
      if (lineBeforeCursor.startsWith("/") && !lineBeforeCursor.slice(1).includes(" ")) {
        const query = lineBeforeCursor.slice(1).toLowerCase();
        const matched = skills.filter(
          (s) => s.trigger.toLowerCase().includes(query) || s.name.toLowerCase().includes(query)
        );
        setSkillMenuItems(matched);
        setSkillMenuVisible(matched.length > 0);
        setSkillMenuIndex(0);
        setSkillTriggerStart(lineStart);
        // Close mention menu when slash menu is open
        setMentionVisible(false);
        return;
      }

      setSkillMenuVisible(false);
      updateMentionMenu(val, cursorPos);
    }, 50);
  }, [updateMentionMenu, skills]);

  const handleSkillSelect = useCallback((skill: SkillDefinition) => {
    // Replace the /query text with the full trigger + space for arguments
    const before = input.slice(0, skillTriggerStart);
    const afterSlash = input.slice(skillTriggerStart);
    // Find end of the slash command word
    const afterCmd = afterSlash.replace(/^\/\S*/, "");
    setInput(before + skill.trigger + " " + afterCmd);
    setSkillMenuVisible(false);
    setSkillMenuIndex(0);
    textareaRef.current?.focus();
  }, [input, skillTriggerStart]);

  // ── Image attachment handlers ──
  const removeImage = useCallback((index: number) => {
    setAttachedImages((prev) => prev.filter((_, i) => i !== index));
  }, []);

  const handlePaste = useCallback(async (e: React.ClipboardEvent) => {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.type.startsWith("image/")) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) {
          if (file.size > MAX_IMAGE_SIZE) {
            setErrorMessage(`Image too large (${(file.size / 1024 / 1024).toFixed(1)}MB). Max ${MAX_IMAGE_SIZE / 1024 / 1024}MB.`);
            return;
          }
          const dataUrl = await readFileAsBase64(file);
          setAttachedImages((prev) => [...prev, dataUrl]);
        }
        return;
      }
    }
  }, []);

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(true);
  }, []);

  const handleDragLeave = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
  }, []);

  const handleDrop = useCallback(async (e: React.DragEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setDragOver(false);
    // Non-image files are handled by Tauri native file-drop events (real paths).
    // This handler only processes images (pasted/dragged from browser or clipboard).
    const files = e.dataTransfer?.files;
    if (!files) return;
    for (let i = 0; i < files.length; i++) {
      const file = files[i];
      if (file.type.startsWith("image/")) {
        if (file.size > MAX_IMAGE_SIZE) {
          setErrorMessage(`Image too large (${(file.size / 1024 / 1024).toFixed(1)}MB). Max ${MAX_IMAGE_SIZE / 1024 / 1024}MB.`);
          continue;
        }
        const dataUrl = await readFileAsBase64(file);
        setAttachedImages((prev) => [...prev, dataUrl]);
      }
      // Non-image files: skip — Tauri file-drop event handles them with real paths
    }
  }, []);

  const handleImagePicker = useCallback(() => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "image/*";
    input.multiple = true;
    input.onchange = async () => {
      const files = input.files;
      if (!files) return;
      for (let i = 0; i < files.length; i++) {
        const file = files[i];
        if (file.size > MAX_IMAGE_SIZE) {
          setErrorMessage(`Image too large (${(file.size / 1024 / 1024).toFixed(1)}MB). Max ${MAX_IMAGE_SIZE / 1024 / 1024}MB.`);
          continue;
        }
        const dataUrl = await readFileAsBase64(file);
        setAttachedImages((prev) => [...prev, dataUrl]);
      }
    };
    input.click();
  }, []);

  // ── Message edit / regenerate ──

  const handleStartEdit = useCallback((index: number) => {
    const msg = messages[index];
    if (msg.role !== "user") return;
    setEditingIndex(index);
    setEditContent(msg.content);
  }, [messages]);

  const handleCancelEdit = useCallback(() => {
    setEditingIndex(null);
    setEditContent("");
  }, []);

  const handleSaveEdit = useCallback(async (index: number) => {
    if (!editContent.trim() || streaming) return;
    const userMessage = editContent.trim();
    setEditingIndex(null);
    setEditContent("");

    // Remove messages from this index onward, then resend
    setMessages((prev) => {
      const updated = prev.slice(0, index);
      updated.push({ role: "user", content: userMessage, timestamp: Date.now() });
      return updated;
    });

    // Resend
    setStreaming(true);
    await sendChatMessage(
      sessionId,
      userMessage,
      mode,
      mode === "agent" ? agentId : undefined,
      undefined,
      planMode || undefined,
      undefined,
      undefined,
      getEffectiveProjectPath() || undefined,
    );
    setStreaming(false);
  }, [editContent, streaming, sessionId, mode, agentId, planMode, getEffectiveProjectPath]);

  const handleRegenerate = useCallback(async (assistantIndex: number) => {
    if (streaming) return;
    // Find the user message before this assistant message
    let userIndex = assistantIndex - 1;
    while (userIndex >= 0 && messages[userIndex]?.role !== "user") {
      userIndex--;
    }
    if (userIndex < 0) return;
    const userMessage = messages[userIndex].content;

    // Remove messages from userIndex onward, then resend
    setMessages((prev) => {
      const updated = prev.slice(0, userIndex);
      updated.push({ role: "user", content: userMessage, timestamp: Date.now() });
      return updated;
    });

    setStreaming(true);
    await sendChatMessage(
      sessionId,
      userMessage,
      mode,
      mode === "agent" ? agentId : undefined,
      undefined,
      planMode || undefined,
      undefined,
      undefined,
      getEffectiveProjectPath() || undefined,
    );
    setStreaming(false);
  }, [streaming, messages, sessionId, mode, agentId, planMode, getEffectiveProjectPath]);

  const handleSend = useCallback(async () => {
    if (!input.trim() || streaming) return;

    setBudgetExhausted(null); // Clear budget exhausted state

    const userMessage = input.trim();
    // Separate file paths from directory paths
    // `@file:line` references are sent as `path:line` so the backend can inject
    // only the referenced line and its surroundings
    const contextFilePaths = attachedFiles
      .filter((f) => !f.is_dir)
      .map((f) => (f.line ? `${f.path}:${f.line}` : f.path));
    const contextFolderPaths = attachedFiles.filter((f) => f.is_dir).map((f) => f.path);
    const imageUrls = attachedImages.length > 0 ? [...attachedImages] : undefined;
    setInput("");
    setAttachedFiles([]);
    setAttachedImages([]);
    setMentionVisible(false);
    setSkillMenuVisible(false);

    // ── Slash command detection ──
    if (userMessage.startsWith("/")) {
      const parts = userMessage.split(/\s+(.*)/s);
      const trigger = parts[0]; // "/review"
      const arguments_ = parts[1] || "";

      const skill = skills.find((s) => s.trigger === trigger);
      if (skill) {
        // Gather template variables from the editor
        const editor = (window as any).__neecoder_editor;
        const selection = editor?.getSelection?.() || "";
        const filePath = editor?.getFilePath?.() || "";
        const projectPathFromEditor = editor?.getProjectPath?.() || getEffectiveProjectPath();

        try {
          const result = await executeSkill({
            trigger,
            selection: selection || undefined,
            file_path: filePath || undefined,
            project_path: projectPathFromEditor || undefined,
            arguments: arguments_ || undefined,
          });

          if (result) {
            // Auto-switch mode and agent based on skill config
            setMode(result.mode as "ask" | "edit" | "agent");
            if (result.agent) {
              setAgentId(result.agent);
            }

            const renderedMessage = result.rendered_message;
            setMessages((prev) => [...prev, { role: "user", content: userMessage }]);
            setStreaming(true);

            // Reset agent observability state
            if (result.mode === "agent") {
              setAgentProgress(null);
              setAgentLogs([]);
              setThinkingText("");
              setTrimNotice(null);
              setTodoList([]);
              setFileChanges([]);
            }

            await sendChatMessage(
              sessionId,
              renderedMessage,
              result.mode,
              result.mode === "agent" ? (result.agent || agentId) : undefined,
              contextFilePaths.length > 0 ? contextFilePaths : undefined,
              planMode || undefined,
              imageUrls,
              contextFolderPaths.length > 0 ? contextFolderPaths : undefined,
              projectPathFromEditor || undefined,
            );
            return;
          }
        } catch (err) {
          const errMsg = err instanceof Error ? err.message : String(err);
          setErrorMessage(`Failed to execute skill: ${errMsg}`);
          setStreaming(false);
          return;
        }
      }
    }

    // ── Normal message flow ──
    setMessages((prev) => [...prev, { role: "user", content: userMessage }]);
    setStreaming(true);

    // Reset agent observability state
    if (mode === "agent") {
      setAgentProgress(null);
      setAgentLogs([]);
      setThinkingText("");
      setTrimNotice(null);
      setTodoList([]);
      setFileChanges([]);
    }

    setErrorMessage(null);
    try {
      await sendChatMessage(
        sessionId,
        userMessage,
        mode,
        mode === "agent" ? agentId : undefined,
        contextFilePaths.length > 0 ? contextFilePaths : undefined,
        planMode || undefined,
        imageUrls,
        contextFolderPaths.length > 0 ? contextFolderPaths : undefined,
        getEffectiveProjectPath() || undefined,
      );
    } catch (err) {
      const errMsg = err instanceof Error ? err.message : String(err);
      setErrorMessage(`Failed to send message: ${errMsg}`);
      setStreaming(false);
    }
  }, [input, streaming, sessionId, mode, agentId, attachedFiles, skills, projectPath, getEffectiveProjectPath]);

  const handleCancel = useCallback(async () => {
    setStreaming(false);
    setBudgetExhausted(null);
    setPaused(false);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      if (mode === "agent") {
        await invoke("cancel_agent", { sessionId });
      } else {
        await invoke("cancel_completion");
      }
    } catch {
      // Not in Tauri
    }
  }, [sessionId, mode]);

  // C1: Agent 暂停 / 恢复
  const handlePause = useCallback(async () => {
    if (mode !== "agent") return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("pause_agent", { sessionId });
      setPaused(true);
    } catch {
      // Not in Tauri
    }
  }, [sessionId, mode]);

  const handleResume = useCallback(async () => {
    if (mode !== "agent") return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("resume_agent", { sessionId });
      setPaused(false);
    } catch {
      // Not in Tauri
    }
  }, [sessionId, mode]);

  const handleContinue = useCallback(async () => {
    if (streaming || !budgetExhausted) return;
    setBudgetExhausted(null);
    setMessages((prev) => [...prev, { role: "user" as const, content: "Continue with the remaining tasks." }]);
    setStreaming(true);
    if (mode === "agent") {
      setAgentProgress(null);
      setAgentLogs([]);
      setThinkingText("");
      setTrimNotice(null);
      setTodoList([]);
      setFileChanges([]);
    }
    try {
      await sendChatMessage(
        sessionId,
        "Continue with the remaining tasks.",
        mode,
        mode === "agent" ? agentId : undefined,
        undefined,
        planMode || undefined,
        undefined,
        undefined,
        getEffectiveProjectPath() || undefined,
      );
    } catch (err) {
      const errMsg = err instanceof Error ? err.message : String(err);
      setErrorMessage(`Failed to continue: ${errMsg}`);
      setStreaming(false);
    }
  }, [streaming, budgetExhausted, sessionId, mode, agentId, planMode, getEffectiveProjectPath]);

  const currentAgent = agents.find((a) => a.id === agentId);

  const handleNewSession = useCallback(async () => {
    const id = await createNewSession();
    if (id) {
      setSessionId(id);
      setMessages([]);
      setShowSessionList(false);
      // Refresh session list
      const list = await getSessions();
      if (list.length > 0) setSessions(list);
    }
  }, []);

  const handleSwitchSession = useCallback(async (sid: string) => {
    setSessionId(sid);
    setShowSessionList(false);
    // Load session message history from backend
    try {
      const msgs = await getSessionMessages(sid);
      if (msgs.length > 0) {
        setMessages(msgs.map((m: SessionMessage) => ({
          role: m.role as "user" | "assistant",
          content: m.content,
        })));
      } else {
        setMessages([]);
      }
    } catch {
      setMessages([]);
    }
  }, []);

  const handleDeleteSession = useCallback(async (sid: string, e: React.MouseEvent) => {
    e.stopPropagation();
    await deleteSessionApi(sid);
    const list = await getSessions();
    setSessions(list.length > 0 ? list : []);
  }, []);

  async function handleAnswerQuestion(answers: string[]) {
    if (!questionDialog) return;
    await answerAgentQuestion(questionDialog.question_id, answers);
    setQuestionDialog(null);
  }

  async function handleApprovePlan() {
    if (!planDialog) return;
    await approvePlan(planDialog.plan.session_id);
    setPlanDialog(null);
  }

  async function handleRejectPlan(reason: string) {
    if (!planDialog) return;
    await rejectPlan(planDialog.plan.session_id, reason || undefined);
    setPlanDialog(null);
  }

  async function handleSkipPlan() {
    if (!planDialog) return;
    await skipPlan(planDialog.plan.session_id);
    setPlanDialog(null);
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    // When skill menu is open, intercept arrow keys / Enter / Escape
    if (skillMenuVisible && skillMenuItems.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setSkillMenuIndex((prev) => (prev + 1) % skillMenuItems.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setSkillMenuIndex((prev) => (prev - 1 + skillMenuItems.length) % skillMenuItems.length);
        return;
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleSkillSelect(skillMenuItems[skillMenuIndex]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setSkillMenuVisible(false);
        return;
      }
    }
    // When mention menu is open, intercept arrow keys / Enter / Escape
    if (mentionVisible && mentionItems.length > 0) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setMentionIndex((prev) => (prev + 1) % mentionItems.length);
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setMentionIndex((prev) => (prev - 1 + mentionItems.length) % mentionItems.length);
        return;
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        handleMentionSelect(mentionItems[mentionIndex]);
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setMentionVisible(false);
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  }

  return (
    <div className={`chat-panel${dragOver ? " drag-over" : ""}`} onDrop={handleDrop} onDragOver={handleDragOver} onDragLeave={handleDragLeave}>
      <div className="chat-header">
        <h3>
          Chat
          {streaming && <span className="chat-streaming-indicator">●</span>}
        </h3>
        <div className="chat-header-actions">
          <div className="chat-mode-selector">
            <select
              className="settings-select"
              style={{ width: 90, fontSize: 11, padding: "2px 6px" }}
              value={mode}
              onChange={(e) => setMode(e.target.value as any)}
            >
              <option value="ask">Ask</option>
              <option value="edit">Edit</option>
              <option value="agent">Agent</option>
            </select>
            {mode === "agent" && agents.length > 0 && (
              <select
                className="settings-select"
                style={{ width: 110, fontSize: 11, padding: "2px 6px", marginLeft: 4 }}
                value={agentId}
                onChange={(e) => setAgentId(e.target.value)}
              >
                {agents.map((a) => (
                  <option key={a.id} value={a.id} title={a.description}>
                    {a.name}
                  </option>
                ))}
              </select>
            )}
          </div>
          <button className="chat-new-btn" onClick={handleNewSession} title="New Session">
            +
          </button>
          <button
            className="chat-new-btn"
            onClick={() => { setShowSessionList(!showSessionList); getSessions().then(setSessions); }}
            title="Session History"
            style={{ fontSize: 14 }}
          >
            ☰
          </button>
        </div>
      </div>

      {showSessionList && sessions.length > 0 && (
        <div className="chat-session-list">
          {sessions.map((s) => (
            <div
              key={s.id}
              className={`chat-session-item ${s.id === sessionId ? "active" : ""}`}
              onClick={() => handleSwitchSession(s.id)}
            >
              <span className="chat-session-title">{s.title || s.id.substring(0, 8)}</span>
              <span className="chat-session-meta">
                {s.message_count} msgs
                <button
                  className="chat-session-delete"
                  onClick={(e) => handleDeleteSession(s.id, e)}
                  title="Delete"
                >×</button>
              </span>
            </div>
          ))}
        </div>
      )}

      <div className="chat-messages">
        {messages.length === 0 && (
          <div className="chat-welcome">
            <div className="chat-welcome-icon">💡</div>
            <p>Ask me anything about your code</p>
            <p className="chat-welcome-hint">
              {mode === "ask" && "Questions & answers about your codebase"}
              {mode === "edit" && "Describe code changes you want to make"}
              {mode === "agent" && currentAgent
                ? `Agent mode: ${currentAgent.name} - ${currentAgent.description}`
                : mode === "agent"
                ? "Full agent mode with tool access"
                : ""}
            </p>
            <div className="chat-suggestions">
              <button className="chat-suggestion" onClick={() => setInput("Explain the current file")}>
                Explain code
              </button>
              <button className="chat-suggestion" onClick={() => setInput("Find a bug in this code")}>
                Find bugs
              </button>
              <button className="chat-suggestion" onClick={() => setInput("Write tests for this function")}>
                Write tests
              </button>
            </div>
          </div>
        )}

        {agentProgress && <AgentProgressBar progress={agentProgress} />}

        {trimNotice && <div className="trim-notice">{trimNotice}</div>}

        {messages.map((msg, i) => (
          <div key={i} className={`chat-message ${msg.role}`}>
            <div className="chat-message-header">
              <span className="chat-message-role">
                {msg.role === "user" ? "You"
                  : msg.role === "thinking" ? "NeeCoder"
                  : msg.role === "tool" ? "Agent"
                  : "NeeCoder"}
              </span>
              <div className="chat-message-actions">
                {msg.role === "user" && editingIndex !== i && (
                  <button
                    className="chat-message-action-btn"
                    onClick={() => handleStartEdit(i)}
                    title="Edit message"
                  >
                    ✏️
                  </button>
                )}
                {msg.role === "user" && (
                  <button
                    className="chat-message-action-btn"
                    onClick={async () => {
                      const newSessionId = await forkSession(sessionId, i);
                      if (newSessionId) {
                        // Switch to the new session
                        const sessions = await getSessions();
                        const newSession = sessions.find(s => s.id === newSessionId);
                        if (newSession) {
                          setSessionId(newSessionId);
                          setMessages([]);
                          // Load messages from the new session
                          const msgs = await getSessionMessages(newSessionId);
                          setMessages(msgs.map(m => ({
                            role: m.role as "user" | "assistant" | "tool",
                            content: m.content,
                          })));
                        }
                      }
                    }}
                    title="Branch from here (create new session from this point)"
                  >
                    🔀
                  </button>
                )}
                {msg.role === "assistant" &&
                  i === messages.length - 1 &&
                  !streaming &&
                  messages[i - 1]?.role === "user" && (
                    <button
                      className="chat-message-action-btn"
                      onClick={() => handleRegenerate(i)}
                      title="Regenerate response"
                    >
                      🔄
                    </button>
                  )}
              </div>
            </div>
            <div className="chat-message-content">
              {editingIndex === i ? (
                <div className="chat-message-edit-area">
                  <textarea
                    className="chat-message-edit-input"
                    value={editContent}
                    onChange={(e) => setEditContent(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
                        e.preventDefault();
                        handleSaveEdit(i);
                      }
                      if (e.key === "Escape") handleCancelEdit();
                    }}
                    autoFocus
                  />
                  <div className="chat-message-edit-btns">
                    <button
                      className="chat-message-edit-cancel"
                      onClick={handleCancelEdit}
                    >
                      Cancel
                    </button>
                    <button
                      className="chat-message-edit-save"
                      onClick={() => handleSaveEdit(i)}
                    >
                      Save & Send
                    </button>
                  </div>
                </div>
              ) : msg.role === "assistant" ? (
                <ReactMarkdown components={markdownComponents}>
                  {msg.content}
                </ReactMarkdown>
              ) : msg.role === "thinking" ? (
                <ThinkingBubble content={msg.content} />
              ) : msg.role === "tool" ? (
                <ToolCard
                  toolName={msg.toolName || ""}
                  arguments={msg.toolCalls?.[0]?.arguments}
                  result={msg.toolResult}
                  isRunning={msg.isRunning}
                  durationMs={msg.durationMs}
                  isRetry={msg.isRetry}
                />
              ) : (
                msg.content
              )}
            </div>
          </div>
        ))}
        {todoList.length > 0 && <TodoListCard todos={todoList} />}
        {fileChanges.length > 0 && <DiffCard changes={fileChanges} />}
        {streaming && messages[messages.length - 1]?.role !== "assistant" && (
          <div className="chat-message assistant">
            <div className="chat-message-header">
              <span className="chat-message-role">NeeCoder</span>
            </div>
            <div className="chat-typing">
              <span className="typing-dot" />
              <span className="typing-dot" />
              <span className="typing-dot" />
            </div>
          </div>
        )}
        <div ref={messagesEndRef} />
      </div>

      <LogPanel logs={agentLogs} open={logPanelOpen} onToggle={() => setLogPanelOpen(!logPanelOpen)} />

      {errorMessage && (
        <div className="chat-error-banner">
          <span className="chat-error-icon">⚠</span>
          <span className="chat-error-text">{errorMessage}</span>
          <button className="chat-error-close" onClick={() => setErrorMessage(null)}>✕</button>
        </div>
      )}

      <div
        className="chat-input-area"
        onDrop={handleDrop}
        onDragOver={handleDragOver}
      >
        {/* Attached file chips */}
        {attachedFiles.length > 0 && (
          <div className="chat-attached-files">
            {attachedFiles.map((f) => (
              <span key={f.path + (f.line ? `:${f.line}` : "")} className="mention-chip">
                {f.is_dir ? "📁" : "📄"} {f.name}
                {f.line ? <span className="mention-chip-line">:{f.line}</span> : null}
                <button
                  className="mention-chip-remove"
                  onClick={() => setAttachedFiles((prev) => prev.filter((x) => !(x.path === f.path && x.line === f.line)))}
                >×</button>
              </span>
            ))}
          </div>
        )}

        {/* Attached image previews */}
        {attachedImages.length > 0 && (
          <div className="chat-attached-images">
            {attachedImages.map((dataUrl, idx) => (
              <div key={idx} className="image-chip">
                <img src={dataUrl} alt={`Attachment ${idx + 1}`} className="image-chip-preview" />
                <button
                  className="image-chip-remove"
                  onClick={() => removeImage(idx)}
                  title="Remove image"
                >×</button>
              </div>
            ))}
          </div>
        )}

        {/* Mention autocomplete menu */}
        <MentionMenu
          visible={mentionVisible}
          items={mentionItems}
          selectedIndex={mentionIndex}
          position={mentionTriggerPos}
          onSelect={handleMentionSelect}
        />

        {/* Skill slash-command menu */}
        {skillMenuVisible && skillMenuItems.length > 0 && (
          <div className="mention-menu skill-menu" style={{ bottom: 60, left: 0 }}>
            {skillMenuItems.map((skill, idx) => (
              <div
                key={skill.trigger}
                className={`mention-item skill-menu-item ${idx === skillMenuIndex ? "selected" : ""}`}
                onMouseDown={(e) => {
                  e.preventDefault();
                  handleSkillSelect(skill);
                }}
              >
                <span className="mention-icon">⚡</span>
                <div className="mention-info">
                  <span className="mention-label">{skill.trigger}</span>
                  {skill.description && (
                    <span className="mention-desc">{skill.description}</span>
                  )}
                </div>
                <span className="mention-type-tag">{skill.mode}</span>
              </div>
            ))}
          </div>
        )}

        <textarea
          ref={textareaRef}
          className="chat-input"
          rows={2}
          placeholder="Ask something... (@file, @codebase, /skill, paste images)"
          value={input}
          onChange={handleInputChange}
          onKeyDown={handleKeyDown}
          onPaste={handlePaste}
          disabled={streaming}
        />
        <div className="chat-input-actions">
          <button
            className="chat-image-btn"
            onClick={handleImagePicker}
            title="Attach images (or paste/drag)"
            disabled={streaming}
          >
            🖼️
          </button>
          {mode === "agent" && (
            <button
              className={`chat-plan-btn ${planMode ? "active" : ""}`}
              onClick={() => setPlanMode(!planMode)}
              title="Toggle Planning Mode (read-only analysis)"
              disabled={streaming}
            >
              {planMode ? "Plan ON" : "Plan"}
            </button>
          )}
          <button
            className="chat-send-btn"
            onClick={handleSend}
            disabled={!input.trim() || streaming}
          >
            {streaming ? "..." : "Send"}
          </button>
          {streaming && (
            <button
              className="chat-cancel-btn"
              onClick={handleCancel}
              title="Cancel"
            >
              ■ Cancel
            </button>
          )}
          {streaming && mode === "agent" && (
            paused ? (
              <button
                className="chat-continue-btn"
                onClick={handleResume}
                title="Resume agent"
              >
                <Play size={13} /> Resume
              </button>
            ) : (
              <button
                className="chat-pause-btn"
                onClick={handlePause}
                title="Pause agent"
              >
                <Pause size={13} /> Pause
              </button>
            )
          )}
          {budgetExhausted && !streaming && (
            <button
              className="chat-continue-btn"
              onClick={handleContinue}
              title={`Iteration budget exhausted (${budgetExhausted.maxIterations}). Click to continue.`}
            >
              ▶ Continue
            </button>
          )}
        </div>
      </div>

      {questionDialog && (
        <AskQuestionDialog
          payload={questionDialog}
          onAnswer={handleAnswerQuestion}
          onDismiss={() => setQuestionDialog(null)}
        />
      )}

      {planDialog && (
        <PlanCard
          payload={planDialog}
          onApprove={handleApprovePlan}
          onReject={handleRejectPlan}
          onSkip={handleSkipPlan}
          disabled={false}
        />
      )}

      {confirmDialog && (
        <ConfirmDangerDialog
          payload={confirmDialog}
          onAllow={() => {
            answerConfirm(confirmDialog.confirm_id, true);
            setConfirmDialog(null);
          }}
          onDeny={() => {
            answerConfirm(confirmDialog.confirm_id, false);
            setConfirmDialog(null);
          }}
        />
      )}
    </div>
  );
}

function mockResponse(message: string, mode: string): string {
  if (message.toLowerCase().includes("hello") || message.toLowerCase().includes("hi")) {
    return "Hello! I'm **NeeCoder**, your AI coding assistant. How can I help you today?";
  }
  if (message.toLowerCase().includes("explain")) {
    return `Here's an explanation of the code:

\`\`\`rust
// This function handles the main request flow
async fn process_request(input: &str) -> Result<String, Error> {
    // Validate input
    if input.is_empty() {
        return Err(Error::EmptyInput);
    }

    // Process asynchronously
    let result = some_async_operation(input).await?;
    Ok(result)
}
\`\`\`

This pattern is common in Rust async applications. Key points:
- Uses \`async/await\` for non-blocking operations
- Returns \`Result\` for error handling
- The \`?\` operator propagates errors automatically`;
  }
  if (mode === "edit") {
    return `I'll make the following changes:

**File**: \`src/main.rs\`

\`\`\`diff
- let x = 5;
+ let x = 10;
\`\`\`

**Reason**: This change updates the initialization value to match the new requirements.`;
  }
  if (mode === "agent") {
    return `**Plan**:
1. 📖 Read the current file structure
2. 🔍 Identify the relevant module
3. ✏️ Apply the changes

Let me start by examining the project...

\`\`\`
📁 src/
  ├── main.rs
  ├── lib.rs
  └── components/
\`\`\`

I've analyzed the code. Here's what I found:

- The main entry point is in \`main.rs\`
- Core logic is in \`lib.rs\`
- The components directory has UI modules`;
  }
  return `I understand you're asking about "${message.substring(0, 50)}..."

Here's my analysis in **${mode}** mode:

\`\`\`typescript
function example() {
  console.log("Hello from NeeCoder!");
}
\`\`\`

Let me know if you need more details!`;
}
