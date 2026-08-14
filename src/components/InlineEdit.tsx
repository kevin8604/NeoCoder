import { useState, useRef, useEffect, useCallback } from "react";
import { Sparkles, X, Check } from "lucide-react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ── Types ──────────────────────────────────────────────────────────────────

interface EditTurn {
  instruction: string;
  edited: string;
}

interface InlineEditRequest {
  instruction: string;
  file_path: string;
  selected_code: string;
  prefix_context: string;
  suffix_context: string;
  /** Previous turns of this conversation (multi-turn inline chat) */
  history?: EditTurn[];
}

interface InlineEditResponse {
  original: string;
  edited: string;
  diff_lines: string[];
}

interface EditInlineEvent {
  Started?: true;
  Delta?: { token: string };
  Finished?: { edited: string };
  Error?: { message: string };
}

export interface InlineEditState {
  visible: boolean;
  loading: boolean;
  error: string | null;
  original: string;
  edited: string;
  selectionStart: number;
  selectionEnd: number;
  /** Conversation turns: each entry records the instruction + produced code */
  history: EditTurn[];
  turn: number;
}

// ── Inline Edit Bar Component ──────────────────────────────────────────────

interface InlineEditBarProps {
  visible: boolean;
  loading: boolean;
  turn: number;
  onSubmit: (instruction: string) => void;
  onCancel: () => void;
}

export function InlineEditBar({ visible, loading, turn, onSubmit, onCancel }: InlineEditBarProps) {
  const [instruction, setInstruction] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (visible && inputRef.current) {
      inputRef.current.focus();
      setInstruction("");
    }
  }, [visible]);

  const handleSubmit = () => {
    if (instruction.trim() && !loading) {
      onSubmit(instruction.trim());
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSubmit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      onCancel();
    }
  };

  if (!visible) return null;

  return (
    <div
      style={{
        position: "absolute",
        top: -48,
        left: 0,
        right: 0,
        zIndex: 100,
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "8px 12px",
        background: "var(--bg-secondary, #1e1e2e)",
        borderTop: "1px solid var(--border, #313244)",
        borderBottom: "1px solid var(--border, #313244)",
        boxShadow: "0 2px 8px rgba(0,0,0,0.2)",
      }}
    >
      <span style={{ color: "var(--accent, #89b4fa)", fontSize: 12, fontWeight: 600 }}>
        <Sparkles size={12} /> Edit {turn + 1}
      </span>
      <input
        ref={inputRef}
        type="text"
        value={instruction}
        onChange={(e) => setInstruction(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={
          turn === 0
            ? "Describe the edit (e.g., 'add error handling')..."
            : "Refine the previous edit (e.g., 'also handle empty input')..."
        }
        disabled={loading}
        style={{
          flex: 1,
          padding: "6px 10px",
          background: "var(--bg-tertiary, #181825)",
          border: "1px solid var(--border, #313244)",
          borderRadius: 4,
          color: "var(--text-primary, #cdd6f4)",
          fontSize: 13,
          outline: "none",
        }}
      />
      <button
        onClick={handleSubmit}
        disabled={loading || !instruction.trim()}
        style={{
          padding: "6px 12px",
          background: loading ? "var(--bg-tertiary, #313244)" : "var(--accent, #89b4fa)",
          color: loading ? "var(--text-muted, #6c7086)" : "var(--bg-primary, #1e1e2e)",
          border: "none",
          borderRadius: 4,
          fontSize: 12,
          fontWeight: 600,
          cursor: loading ? "not-allowed" : "pointer",
        }}
      >
        {loading ? "..." : "Apply"}
      </button>
      <button
        onClick={onCancel}
        disabled={loading}
        style={{
          padding: "6px 10px",
          background: "transparent",
          color: "var(--text-muted, #6c7086)",
          border: "1px solid var(--border, #313244)",
          borderRadius: 4,
          fontSize: 12,
          cursor: "pointer",
        }}
      >
        <X size={12} />
      </button>
    </div>
  );
}

// ── Inline Diff View Component ─────────────────────────────────────────────

interface InlineDiffViewProps {
  original: string;
  edited: string;
  turn: number;
  onAccept: () => void;
  /** Reject this version and go back to the input bar to refine it further */
  onContinue: () => void;
}

export function InlineDiffView({ original, edited, turn, onAccept, onContinue }: InlineDiffViewProps) {
  const originalLines = original.split("\n");
  const editedLines = edited.split("\n");

  return (
    <div
      style={{
        position: "absolute",
        top: 0,
        left: 0,
        right: 0,
        bottom: 0,
        zIndex: 99,
        display: "flex",
        flexDirection: "column",
        background: "var(--bg-primary, #1e1e2e)",
        overflow: "auto",
      }}
    >
      {/* Action bar */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "8px 12px",
          background: "var(--bg-secondary, #181825)",
          borderBottom: "1px solid var(--border, #313244)",
        }}
      >
        <span style={{ color: "var(--text-primary, #cdd6f4)", fontSize: 12 }}>
          Review changes{turn > 0 ? ` (turn ${turn + 1})` : ""}:
        </span>
        <button
          onClick={onAccept}
          style={{
            padding: "4px 12px",
            background: "var(--success, #a6e3a1)",
            color: "#1e1e2e",
            border: "none",
            borderRadius: 4,
            fontSize: 12,
            fontWeight: 600,
            cursor: "pointer",
          }}
        >
          <Check size={12} /> Accept (Tab)
        </button>
        <button
          onClick={onContinue}
          title="Reject this version and keep refining it with another instruction"
          style={{
            padding: "4px 12px",
            background: "transparent",
            color: "var(--text-primary, #cdd6f4)",
            border: "1px solid var(--border, #313244)",
            borderRadius: 4,
            fontSize: 12,
            fontWeight: 600,
            cursor: "pointer",
          }}
        >
          <X size={12} /> Reject & refine (Esc)
        </button>
      </div>

      {/* Diff content */}
      <div
        style={{
          flex: 1,
          overflow: "auto",
          fontFamily: "monospace",
          fontSize: 13,
          lineHeight: 1.5,
        }}
      >
        {/* Show edited code with line-by-line comparison */}
        {editedLines.map((line, i) => {
          const originalLine = originalLines[i];
          const isChanged = line !== originalLine;

          return (
            <div
              key={i}
              style={{
                display: "flex",
                background: isChanged
                  ? "rgba(166, 227, 161, 0.1)"
                  : "transparent",
                borderBottom: "1px solid var(--border-subtle, #21212e)",
              }}
            >
              <span
                style={{
                  width: 40,
                  textAlign: "right",
                  padding: "2px 8px",
                  color: "var(--text-muted, #6c7086)",
                  background: "var(--bg-secondary, #181825)",
                  userSelect: "none",
                  flexShrink: 0,
                }}
              >
                {i + 1}
              </span>
              <pre
                style={{
                  flex: 1,
                  margin: 0,
                  padding: "2px 12px",
                  color: isChanged ? "var(--success, #a6e3a1)" : "var(--text-primary, #cdd6f4)",
                  whiteSpace: "pre-wrap",
                  wordBreak: "break-word",
                }}
              >
                {line || " "}
              </pre>
            </div>
          );
        })}
      </div>
    </div>
  );
}

// ── Hook: useInlineEdit ────────────────────────────────────────────────────

export function useInlineEdit(filePath: string) {
  const [state, setState] = useState<InlineEditState>({
    visible: false,
    loading: false,
    error: null,
    original: "",
    edited: "",
    selectionStart: 0,
    selectionEnd: 0,
    history: [],
    turn: 0,
  });

  // Listen for edit-inline events
  useEffect(() => {
    const unlisten = listen<EditInlineEvent>("edit-inline-event", (event) => {
      const payload = event.payload;
      if (payload.Error) {
        setState((prev) => ({ ...prev, loading: false, error: payload.Error!.message }));
      }
    });
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  const showEditBar = useCallback((selectedCode: string, start: number, end: number) => {
    if (!selectedCode.trim()) return;
    setState({
      visible: true,
      loading: false,
      error: null,
      original: selectedCode,
      edited: "",
      selectionStart: start,
      selectionEnd: end,
      history: [],
      turn: 0,
    });
  }, []);

  const hideEditBar = useCallback(() => {
    setState({
      visible: false,
      loading: false,
      error: null,
      original: "",
      edited: "",
      selectionStart: 0,
      selectionEnd: 0,
      history: [],
      turn: 0,
    });
  }, []);

  const submitEdit = useCallback(
    async (instruction: string, prefixContext: string, suffixContext: string) => {
      setState((prev) => ({ ...prev, loading: true, error: null }));

      // Multi-turn: base the next edit on the latest produced version (the
      // original selection is only used on the first turn).
      const latest = state.history.length > 0 ? state.history[state.history.length - 1].edited : state.original;

      const request: InlineEditRequest = {
        instruction,
        file_path: filePath,
        selected_code: latest,
        prefix_context: prefixContext,
        suffix_context: suffixContext,
        history: state.history,
      };

      try {
        const response = await invoke<InlineEditResponse>("edit_inline", { request });
        setState((prev) => ({
          ...prev,
          loading: false,
          edited: response.edited,
          history: [...prev.history, { instruction, edited: response.edited }],
          turn: prev.turn + 1,
        }));
      } catch (err) {
        setState((prev) => ({
          ...prev,
          loading: false,
          error: String(err),
        }));
      }
    },
    [filePath, state.original, state.history]
  );

  const acceptEdit = useCallback((): { edited: string; start: number; end: number } | null => {
    if (!state.edited) return null;
    const result = { edited: state.edited, start: state.selectionStart, end: state.selectionEnd };
    hideEditBar();
    return result;
  }, [state.edited, state.selectionStart, state.selectionEnd, hideEditBar]);

  // Reject the current diff and go back to the input bar, keeping the
  // conversation history so the user can refine the previous result.
  // (Press Esc again on the bar to fully close.)
  const rejectEdit = useCallback(() => {
    setState((prev) => ({ ...prev, visible: true, edited: "" }));
  }, []);

  return {
    state,
    showEditBar,
    hideEditBar,
    submitEdit,
    acceptEdit,
    rejectEdit,
  };
}
