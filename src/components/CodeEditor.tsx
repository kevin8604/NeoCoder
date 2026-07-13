import { useEffect, useRef, useState, useCallback, useMemo } from "react";
import { EditorView, keymap, placeholder, Decoration, WidgetType, ViewPlugin, ViewUpdate, DecorationSet } from "@codemirror/view";
import { EditorState, StateEffect, StateField, RangeSetBuilder } from "@codemirror/state";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { syntaxHighlighting, defaultHighlightStyle, bracketMatching, indentOnInput } from "@codemirror/language";
import { closeBrackets } from "@codemirror/autocomplete";
import { highlightSelectionMatches, searchKeymap, openSearchPanel } from "@codemirror/search";
import { oneDark } from "@codemirror/theme-one-dark";
import { rust } from "@codemirror/lang-rust";
import { javascript } from "@codemirror/lang-javascript";
import { python } from "@codemirror/lang-python";
import { html } from "@codemirror/lang-html";
import { css } from "@codemirror/lang-css";
import { json } from "@codemirror/lang-json";
import { markdown } from "@codemirror/lang-markdown";
import { InlineEditBar, InlineDiffView, useInlineEdit } from "./InlineEdit";

// ── Light theme for CodeMirror ─────────────────────────────────────────────

const lightEditorTheme = EditorView.theme({
  "&": {
    backgroundColor: "#ffffff",
    color: "#1a1a2e",
  },
  ".cm-content": {
    caretColor: "#5b6abf",
  },
  ".cm-cursor, .cm-dropCursor": {
    borderLeftColor: "#5b6abf",
  },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "#c5d3f0",
  },
  ".cm-activeLine": {
    backgroundColor: "rgba(91, 106, 191, 0.04)",
  },
  ".cm-selectionMatch": {
    backgroundColor: "rgba(91, 106, 191, 0.08)",
  },
  ".cm-gutters": {
    backgroundColor: "#f6f8fa",
    color: "#8a8aaa",
    borderRight: "1px solid #d0d7de",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "#e4e8ec",
    color: "#4a4a6a",
  },
  ".cm-foldPlaceholder": {
    backgroundColor: "#f0f2f4",
    borderColor: "#d0d7de",
    color: "#8a8aaa",
  },
  ".cm-tooltip": {
    backgroundColor: "#ffffff",
    borderColor: "#d0d7de",
    color: "#1a1a2e",
  },
}, { dark: false });

// ── Types ──────────────────────────────────────────────────────────────────

interface CodeEditorProps {
  content: string;
  filePath: string;
  projectPath?: string | null;
  onContentChange: (content: string) => void;
  onCursorMove?: (line: number, col: number) => void;
  completionText?: string | null;
  onAcceptCompletion?: () => void;
  onDismissCompletion?: () => void;
}

// ── Ghost Text Widget ──────────────────────────────────────────────────────

class GhostTextWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }

  eq(other: GhostTextWidget): boolean {
    return other.text === this.text;
  }

  toDOM(): HTMLElement {
    const span = document.createElement("span");
    span.className = "cm-ghost-text";
    span.textContent = this.text;
    span.style.color = "var(--text-muted)";
    span.style.opacity = "0.5";
    span.style.fontStyle = "italic";
    return span;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

// ── Ghost Text State Field ─────────────────────────────────────────────────

const ghostTextEffect = StateEffect.define<{ from: number; text: string } | null>();

const ghostTextField = StateField.define<DecorationSet>({
  create() {
    return Decoration.none;
  },
  update(decorations, tr) {
    for (const e of tr.effects) {
      if (e.is(ghostTextEffect)) {
        if (e.value === null) return Decoration.none;
        const { from, text } = e.value;
        if (!text) return Decoration.none;
        const widget = Decoration.widget({
          widget: new GhostTextWidget(text),
          side: 1,
        });
        return Decoration.set([widget.range(from)]);
      }
    }
    return decorations;
  },
  provide: (f) => EditorView.decorations.from(f),
});

// ── Keybinding for ghost text ──────────────────────────────────────────────

const ghostTextTheme = EditorView.baseTheme({
  ".cm-ghost-text": {
    pointerEvents: "none",
    userSelect: "none",
  },
});

// ── Language detector ──────────────────────────────────────────────────────

function getLanguageExtension(filePath: string) {
  const ext = filePath.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "rs": return rust();
    case "ts":
    case "tsx":
    case "js":
    case "jsx": return javascript();
    case "py": return python();
    case "html":
    case "htm": return html();
    case "css":
    case "scss":
    case "less": return css();
    case "json": return json();
    case "md":
    case "markdown": return markdown();
    default: return javascript(); // fallback
  }
}

// ── Main Component ─────────────────────────────────────────────────────────

export default function CodeEditor({
  content,
  filePath,
  projectPath,
  onContentChange,
  onCursorMove,
  completionText,
  onAcceptCompletion,
  onDismissCompletion,
}: CodeEditorProps) {
  const editorRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const [editorKey, setEditorKey] = useState(0);

  // ── Inline Edit state ──
  const {
    state: inlineEditState,
    showEditBar,
    hideEditBar,
    submitEdit,
    acceptEdit,
    rejectEdit,
  } = useInlineEdit(filePath);

  // Create editor
  useEffect(() => {
    if (!editorRef.current) return;

    const langExt = getLanguageExtension(filePath);
    const isLight = document.documentElement.getAttribute("data-theme") === "light";

    const state = EditorState.create({
      doc: content,
      extensions: [
        // Core
        keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
        history(),
        bracketMatching(),
        closeBrackets(),
        indentOnInput(),
        highlightSelectionMatches(),
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        isLight ? lightEditorTheme : oneDark,
        ghostTextTheme,

        // Language
        langExt,

        // Ghost text state
        ghostTextField,

        // Keymap for ghost text handling
        EditorView.domEventHandlers({
          keydown: (event, view) => {
            // Ctrl+K / Cmd+K: Inline Edit
            if (event.key === "k" && (event.ctrlKey || event.metaKey)) {
              event.preventDefault();
              const sel = view.state.selection.main;
              const doc = view.state.doc;
              const selectedText = doc.slice(sel.from, sel.to).toString();
              if (selectedText.trim()) {
                showEditBar(selectedText, sel.from, sel.to);
              }
              return true;
            }

            // Escape: dismiss diff view or completion
            if (event.key === "Escape") {
              if (inlineEditState.edited) {
                event.preventDefault();
                rejectEdit();
                return true;
              }
              if (completionText) {
                event.preventDefault();
                onDismissCompletion?.();
                view.dispatch({ effects: ghostTextEffect.of(null) });
                return true;
              }
            }

            // Tab: accept diff or completion
            if (event.key === "Tab") {
              if (inlineEditState.edited) {
                event.preventDefault();
                const result = acceptEdit();
                if (result) {
                  view.dispatch({
                    changes: { from: result.start, to: result.end, insert: result.edited },
                  });
                }
                return true;
              }
              if (completionText) {
                event.preventDefault();
                onAcceptCompletion?.();
                view.dispatch({ effects: ghostTextEffect.of(null) });
                return true;
              }
            } else if (event.key === "]" && event.altKey && completionText) {
              event.preventDefault();
              // Cycle to next completion — handled by parent
              return true;
            }
            return false;
          },
        }),

        // Track content changes
        EditorView.updateListener.of((update: ViewUpdate) => {
          if (update.docChanged) {
            onContentChange(update.state.doc.toString());
          }
          // Track cursor position
          if (update.selectionSet) {
            const pos = update.state.selection.main.head;
            const line = update.state.doc.lineAt(pos);
            onCursorMove?.(line.number, pos - line.from);
          }
        }),

        placeholder("Start typing..."),
      ],
    });

    const view = new EditorView({
      state,
      parent: editorRef.current,
    });

    viewRef.current = view;

    return () => {
      view.destroy();
      viewRef.current = null;
    };
  }, [editorKey]);

  // Re-create editor when file changes
  useEffect(() => {
    setEditorKey((k) => k + 1);
  }, [filePath]);

  // Update ghost text decoration when completionText changes
  useEffect(() => {
    const view = viewRef.current;
    if (!view) return;

    if (completionText) {
      const pos = view.state.selection.main.head;
      view.dispatch({
        effects: ghostTextEffect.of({ from: pos, text: completionText }),
      });
    } else {
      view.dispatch({
        effects: ghostTextEffect.of(null),
      });
    }
  }, [completionText]);

  // Accept completion — insert text at cursor
  const insertCompletion = useCallback((text: string) => {
    const view = viewRef.current;
    if (!view || !text) return;

    view.dispatch({
      changes: { from: view.state.selection.main.head, insert: text },
    });
  }, []);

  // Expose insertCompletion via ref pattern
  useEffect(() => {
    if (viewRef.current) {
      (window as any).__neecoder_editor = {
        insertCompletion,
        openFind: () => {
          const view = viewRef.current;
          if (!view) return;
          openSearchPanel(view);
        },
        getFilePath: () => filePath,
        getProjectPath: () => projectPath || "",
        getSelection: () => {
          const view = viewRef.current;
          if (!view) return "";
          const sel = view.state.selection.main;
          if (sel.empty) return "";
          const from = Math.min(sel.from, sel.to);
          const to = Math.max(sel.from, sel.to);
          return view.state.doc.slice(from, to).toString();
        },
        goToLine: (line: number) => {
          const view = viewRef.current;
          if (!view) return;
          const doc = view.state.doc;
          const targetLine = Math.max(1, Math.min(line, doc.lines));
          const lineObj = doc.line(targetLine);
          view.dispatch({
            selection: { anchor: lineObj.from, head: lineObj.from },
            scrollIntoView: true,
          });
        },
        getCursor: () => {
          const pos = viewRef.current!.state.selection.main.head;
          const line = viewRef.current!.state.doc.lineAt(pos);
          return { line: line.number, col: pos - line.from };
        },
        getContext: () => {
          const view = viewRef.current!;
          const state = view.state;
          const doc = state.doc;
          const pos = state.selection.main.head;
          const line = doc.lineAt(pos);
          const prefix = doc.slice(0, pos).toString();
          const suffix = doc.slice(pos, doc.length).toString();
          return { prefix, suffix, line: line.number, col: pos - line.from };
        },
      };
    }
    return () => {
      delete (window as any).__neecoder_editor;
    };
  }, [insertCompletion, filePath, projectPath]);

  // Handle inline edit submit
  const handleInlineEditSubmit = useCallback(
    (instruction: string) => {
      const view = viewRef.current;
      if (!view) return;

      const doc = view.state.doc;
      const sel = view.state.selection.main;

      // Get prefix context (20 lines before selection)
      const startLine = doc.lineAt(sel.from).number;
      const prefixStart = Math.max(1, startLine - 20);
      const prefixContext = doc.slice(doc.line(prefixStart).from, sel.from).toString();

      // Get suffix context (20 lines after selection)
      const endLine = doc.lineAt(sel.to).number;
      const suffixEnd = Math.min(doc.lines, endLine + 20);
      const suffixContext = doc.slice(sel.to, doc.line(suffixEnd).to).toString();

      submitEdit(instruction, prefixContext, suffixContext);
    },
    [submitEdit]
  );

  // Handle inline edit accept
  const handleInlineEditAccept = useCallback(() => {
    const view = viewRef.current;
    if (!view) return;

    const result = acceptEdit();
    if (result) {
      view.dispatch({
        changes: { from: result.start, to: result.end, insert: result.edited },
      });
    }
  }, [acceptEdit]);

  return (
    <div ref={editorRef} className="cm-editor-container" style={{ position: "relative" }}>
      {/* Inline Edit Bar */}
      <InlineEditBar
        visible={inlineEditState.visible && !inlineEditState.edited}
        loading={inlineEditState.loading}
        onSubmit={handleInlineEditSubmit}
        onCancel={hideEditBar}
      />

      {/* Inline Diff View */}
      {inlineEditState.edited && (
        <InlineDiffView
          original={inlineEditState.original}
          edited={inlineEditState.edited}
          onAccept={handleInlineEditAccept}
          onReject={rejectEdit}
        />
      )}

      {/* Error display */}
      {inlineEditState.error && (
        <div
          style={{
            position: "absolute",
            top: 4,
            right: 4,
            padding: "4px 8px",
            background: "var(--error, #f38ba8)",
            color: "#1e1e2e",
            borderRadius: 4,
            fontSize: 12,
            zIndex: 101,
          }}
        >
          {inlineEditState.error}
        </div>
      )}
    </div>
  );
}
