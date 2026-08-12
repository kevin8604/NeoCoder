import { useState, useCallback, useEffect, useRef } from "react";
import ChatPanel from "./components/ChatPanel";
import StatusBar from "./components/StatusBar";
import Settings from "./components/Settings";
import SearchPanel from "./components/SearchPanel";
import ContextMenu from "./components/ContextMenu";
import CodeEditor from "./components/CodeEditor";
import FileExplorer from "./components/FileExplorer";
import CloudAgentPanel from "./components/CloudAgentPanel";
import CheckpointPanel from "./components/CheckpointPanel";
import TerminalPanel from "./components/TerminalPanel";
import DependencyGraph from "./components/DependencyGraph";
import MemoryPanel from "./components/MemoryPanel";
import InsightsPanel from "./components/InsightsPanel";
import { PanelLeft, Search, ListTree, Square, MessageSquare, Sparkles, BookOpen, Wrench, Landmark, Shapes, Box, Layers, MapPin, Folder, Tag, Zap, Target, Circle, Braces, FileCode, FileCode2, FileJson, FileText, File, Globe, Database, Coffee, Palette, FileCog, FileType } from "lucide-react";
import { openProject, getLspSymbols, requestCompletion, cycleCompletion, triggerAutoReview, checkLocalModel, type LSPSymbol } from "./hooks/useTauri";

type View = "editor" | "chat" | "settings" | "search" | "cloud" | "graph" | "memory" | "insights" | "checkpoints";
type Theme = "dark" | "light";

interface OpenFile {
  path: string;
  name: string;
  content: string;
}

function App() {
  const [activeView, setActiveView] = useState<View>("editor");
  const [projectPath, setProjectPath] = useState<string | null>(null);
  const [llmConnected, setLlmConnected] = useState(false);
  /** null = local models disabled; true/false = Ollama health probe result */
  const [localModelRunning, setLocalModelRunning] = useState<boolean | null>(null);
  const [showExplorer, setShowExplorer] = useState(true);
  const [showTerminal, setShowTerminal] = useState(false);
  const [openFiles, setOpenFiles] = useState<OpenFile[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [contextMenu, setContextMenu] = useState<{
    x: number;
    y: number;
    filePath: string;
  } | null>(null);
  
  // Completion state
  const [completionId, setCompletionId] = useState<string | null>(null);
  const [completionText, setCompletionText] = useState<string | null>(null);
  const [candidateIndex, setCandidateIndex] = useState(0);
  const [candidatesCount, setCandidatesCount] = useState(0);

  // Outline state
  const [showOutline, setShowOutline] = useState(false);
  const [outlineSymbols, setOutlineSymbols] = useState<LSPSymbol[]>([]);
  const [outlineLoading, setOutlineLoading] = useState(false);

  // Theme state
  const [theme, setTheme] = useState<Theme>("dark");

  // Side panel width (persisted across sessions)
  const [panelWidth, setPanelWidth] = useState(() => {
    const saved = localStorage.getItem("neecoder-panel-width");
    const n = saved ? parseInt(saved, 10) : 400;
    return Number.isFinite(n) && n >= 300 && n <= 700 ? n : 400;
  });
  const panelWidthRef = useRef(panelWidth);
  const [resizingPanel, setResizingPanel] = useState(false);

  // Terminal panel height (persisted across sessions)
  const [terminalHeight, setTerminalHeight] = useState(() => {
    const saved = localStorage.getItem("neecoder-terminal-height");
    const n = saved ? parseInt(saved, 10) : 280;
    return Number.isFinite(n) && n >= 100 && n <= 600 ? n : 280;
  });
  const terminalHeightRef = useRef(terminalHeight);
  const [resizingTerminal, setResizingTerminal] = useState(false);

  // Load theme from settings and apply to DOM
  useEffect(() => {
    async function loadTheme() {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const s = await invoke<any>("get_settings");
        const t = (s.theme === "Light" ? "light" : "dark") as Theme;
        setTheme(t);
        document.documentElement.setAttribute("data-theme", t);
        if (Array.isArray(s.project_paths) && s.project_paths.length > 0) {
          setProjectPath(s.project_paths[0]);
        }
      } catch {
        // Browser mode: use stored preference or system preference
        const stored = localStorage.getItem("neecoder-theme");
        if (stored === "light" || stored === "dark") {
          setTheme(stored);
          document.documentElement.setAttribute("data-theme", stored);
        } else if (window.matchMedia?.("(prefers-color-scheme: light)").matches) {
          setTheme("light");
          document.documentElement.setAttribute("data-theme", "light");
        }
      }
    }
    loadTheme();
  }, []);

  // ── Local model (Ollama) health polling ────────────────────────
  // Polls check_local_model while settings.local_model.enabled is on,
  // surfaces the result in the status bar (auto-degrades to remote when offline).
  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | null = null;

    async function poll() {
      try {
        const { invoke } = await import("@tauri-apps/api/core");
        const s = await invoke<any>("get_settings");
        const enabled = !!s?.local_model?.enabled;
        if (!enabled) {
          setLocalModelRunning(null);
        } else {
          const health = await checkLocalModel();
          if (!cancelled) setLocalModelRunning(health ? health.running : null);
        }
      } catch {
        // Not in Tauri environment
        setLocalModelRunning(null);
      } finally {
        if (!cancelled) timer = setTimeout(poll, 10_000);
      }
    }
    poll();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
    };
  }, []);

  const toggleTheme = useCallback(() => {
    setTheme((prev) => {
      const next = prev === "dark" ? "light" : "dark";
      document.documentElement.setAttribute("data-theme", next);
      localStorage.setItem("neecoder-theme", next);
      // Also save to backend
      try {
        import("@tauri-apps/api/core").then(({ invoke }) => {
          invoke("get_settings").then((s: any) => {
            invoke("update_settings", { settings: { ...s, theme: next === "light" ? "Light" : "Dark" } });
          });
        });
      } catch {}
      return next;
    });
  }, []);

  // ── Side panel drag-to-resize ───────────────────────────────────
  const startPanelResize = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startX = e.clientX;
    const startW = panelWidthRef.current;
    setResizingPanel(true);
    const onMove = (ev: MouseEvent) => {
      // Panel sits on the right edge: dragging left widens it
      const w = Math.min(700, Math.max(300, startW - (ev.clientX - startX)));
      panelWidthRef.current = w;
      setPanelWidth(w);
    };
    const onUp = () => {
      setResizingPanel(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      localStorage.setItem("neecoder-panel-width", String(panelWidthRef.current));
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, []);

  // ── Terminal drag-to-resize ─────────────────────────────────────
  const startTerminalResize = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const startH = terminalHeightRef.current;
    setResizingTerminal(true);
    const onMove = (ev: MouseEvent) => {
      const h = Math.min(600, Math.max(100, startH + (ev.clientY - startY)));
      terminalHeightRef.current = h;
      setTerminalHeight(h);
    };
    const onUp = () => {
      setResizingTerminal(false);
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      localStorage.setItem("neecoder-terminal-height", String(terminalHeightRef.current));
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }, []);

  // ── Completion trigger (debounced) ──────────────────────────────
  const completionTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastCompletionPosRef = useRef<{ line: number; col: number } | null>(null);

  const handleCursorMove = useCallback((line: number, col: number) => {
    // Skip if position hasn't changed meaningfully (same line)
    if (lastCompletionPosRef.current?.line === line && lastCompletionPosRef.current?.col === col) {
      return;
    }
    lastCompletionPosRef.current = { line, col };

    // Clear pending timer
    if (completionTimerRef.current) {
      clearTimeout(completionTimerRef.current);
    }

    // Debounce: wait 400ms after last cursor movement before requesting
    completionTimerRef.current = setTimeout(async () => {
      const editor = (window as any).__neecoder_editor;
      if (!editor?.getContext) return;

      try {
        const ctx = editor.getContext();
        if (!ctx.prefix || ctx.prefix.length < 2) return; // Need at least some context

        const filePath = editor.getFilePath?.() || activeFile || "";
        const lang = detectLanguage(filePath);

        const result = await requestCompletion({
          file_path: filePath,
          language: lang,
          prefix: ctx.prefix,
          suffix: ctx.suffix,
          cursor_line: ctx.line,
          cursor_column: ctx.col,
        });

        if (result) {
          setCompletionId(result.id);
          setCandidatesCount(result.candidates_count || 1);
          setCandidateIndex(0);
        }
      } catch {
        // Completion failed silently - no ghost text needed
      }
    }, 400);
  }, [activeFile]);

  const handleFileSelect = useCallback(async (path: string, line?: number) => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const content = await invoke<string>("read_file", { path });
      const name = path.split("\\").pop() || path.split("/").pop() || path;

      setOpenFiles((prev) => {
        if (prev.find((f) => f.path === path)) return prev;
        return [...prev, { path, name, content }];
      });
      setActiveFile(path);
    } catch {
      // Browser dev mode — add file without content
      const name = path.split("\\").pop() || path.split("/").pop() || path;
      setOpenFiles((prev) => {
        if (prev.find((f) => f.path === path)) return prev;
        return [...prev, { path, name, content: `// ${name}\n// Opened from ${path}\n` }];
      });
      setActiveFile(path);
    }
  }, []);

  const handleOpenProject = useCallback(async () => {
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const { open } = await import("@tauri-apps/plugin-dialog");
      const selected = await open({ directory: true, title: "Select Project" });
      if (selected) {
        const success = await openProject(selected);
        if (success) {
          setProjectPath(selected);
          setOpenFiles([]);
          setActiveFile(null);
        }
      }
    } catch {
      // Fallback for browser
      const path = prompt("Enter project path:", projectPath || "d:\\workspace\\NeeCoder");
      if (path) {
        setProjectPath(path);
        setOpenFiles([]);
        setActiveFile(null);
      }
    }
  }, [projectPath]);

  const closeFile = useCallback((path: string) => {
    setOpenFiles((prev) => {
      const idx = prev.findIndex((f) => f.path === path);
      const updated = prev.filter((f) => f.path !== path);
      if (path === activeFile && updated.length > 0) {
        const newIdx = Math.min(idx, updated.length - 1);
        setActiveFile(updated[newIdx].path);
      } else if (path === activeFile) {
        setActiveFile(null);
      }
      return updated;
    });
  }, [activeFile]);

  // File content update
  const handleContentChange = useCallback((path: string, content: string) => {
    setOpenFiles((prev) =>
      prev.map((f) => (f.path === path ? { ...f, content } : f))
    );
  }, []);

  // Completion event handling (sent from backend via Tauri events)
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    async function setup() {
      try {
        const { listen } = await import("@tauri-apps/api/event");
        unlisten = await listen<any>("completion-event", (event) => {
          const p = event.payload;
          if (p.token) {
            // Accumulate streamed tokens
            setCompletionText((prev) => (prev || "") + p.token);
            if (p.id) setCompletionId(p.id);
          } else if (p.full_text !== undefined) {
            setCompletionText(p.full_text);
          } else if (p.id && !p.token && p.full_text === undefined) {
            // Started event
            setCompletionId(p.id);
            setCompletionText(null);
          } else if (p.message) {
            // Error event
            console.error("Completion error:", p.message);
            setCompletionText(null);
            setCompletionId(null);
          }
        });
      } catch {
        // Not in Tauri environment
      }
    }

    setup();
    return () => { if (unlisten) unlisten(); };
  }, []);

  // Accept completion: insert text into editor
  const handleAcceptCompletion = useCallback(() => {
    if (completionText) {
      const editor = (window as any).__neecoder_editor;
      if (editor?.insertCompletion) {
        editor.insertCompletion(completionText);
      }
      setCompletionText(null);
      setCompletionId(null);
    }
  }, [completionText]);

  // Dismiss completion
  const handleDismissCompletion = useCallback(() => {
    setCompletionText(null);
    setCompletionId(null);
    setCandidateIndex(0);
    setCandidatesCount(0);
  }, []);

  // Cycle completion candidates (Alt+])
  const handleCycleCompletion = useCallback(async () => {
    if (!completionId || candidatesCount <= 1) return;
    const nextText = await cycleCompletion(completionId, 1);
    if (nextText) {
      setCompletionText(nextText);
      setCandidateIndex((prev) => (prev + 1) % candidatesCount);
    }
  }, [completionId, candidatesCount]);

  // Cancel completion (also calls backend)
  const handleCancelCompletion = useCallback(async () => {
    setCompletionText(null);
    setCompletionId(null);
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("cancel_completion");
    } catch {
      // Not in Tauri
    }
  }, []);

  // Outline handlers
  const handleToggleOutline = useCallback(async () => {
    const willShow = !showOutline;
    setShowOutline(willShow);
    if (willShow && activeFile) {
      setOutlineLoading(true);
      const lang = detectLanguage(activeFile);
      const symbols = await getLspSymbols(lang, activeFile);
      setOutlineSymbols(symbols);
      setOutlineLoading(false);
    } else if (!willShow) {
      setOutlineSymbols([]);
    }
  }, [showOutline, activeFile]);

  const handleGotoSymbol = useCallback((line: number) => {
    const editor = (window as any).__neecoder_editor;
    if (editor?.goToLine) {
      editor.goToLine(line);
    }
  }, []);

  const handleFind = useCallback(() => {
    const editor = (window as any).__neecoder_editor;
    if (editor?.openFind) {
      editor.openFind();
    }
  }, []);

  // Fetch outline when active file changes (if panel is open)
  useEffect(() => {
    if (showOutline && activeFile) {
      setOutlineLoading(true);
      const lang = detectLanguage(activeFile);
      getLspSymbols(lang, activeFile).then((symbols) => {
        setOutlineSymbols(symbols);
        setOutlineLoading(false);
      });
    }
  }, [activeFile]);

  const handleSearchSelect = useCallback((path: string, line?: number) => {
    handleFileSelect(path, line);
    setActiveView("editor");
  }, [handleFileSelect]);

  const handleTabContextMenu = useCallback(
    (e: React.MouseEvent, path: string) => {
      e.preventDefault();
      setContextMenu({ x: e.clientX, y: e.clientY, filePath: path });
    },
    []
  );

  const closeTab = useCallback(
    (path: string) => {
      setOpenFiles((prev) => {
        const idx = prev.findIndex((f) => f.path === path);
        const updated = prev.filter((f) => f.path !== path);
        if (path === activeFile && updated.length > 0) {
          const newIdx = Math.min(idx, updated.length - 1);
          setActiveFile(updated[newIdx].path);
        } else if (path === activeFile) {
          setActiveFile(null);
        }
        return updated;
      });
    },
    [activeFile]
  );

  const closeOtherTabs = useCallback((path: string) => {
    setOpenFiles((prev) => prev.filter((f) => f.path === path));
    setActiveFile(path);
  }, []);

  const closeAllTabs = useCallback(() => {
    setOpenFiles([]);
    setActiveFile(null);
  }, []);

  const copyFilePath = useCallback((path: string) => {
    navigator.clipboard.writeText(path);
  }, []);

  const currentFile = openFiles.find((f) => f.path === activeFile);

  // ── Save current file ───────────────────────────────────────────
  const handleSaveFile = useCallback(async () => {
    if (!currentFile || !activeFile) return;
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("write_file", { path: activeFile, content: currentFile.content });

      // Check if auto-review on save is enabled
      if (projectPath) {
        const settings = await invoke<any>("get_settings");
        if (settings?.auto_review_on_save) {
          // Trigger auto review (non-blocking)
          triggerAutoReview(projectPath).then((result) => {
            if (result) {
              // Switch to chat view to show review results
              setActiveView("chat");
              // The review prompt will be sent via the chat system
              console.log("[AutoReview] Review triggered, session:", result.sessionId);
            }
          }).catch((e) => {
            console.warn("[AutoReview] Failed to trigger review:", e);
          });
        }
      }
    } catch {
      // Browser fallback
      console.log("Save not available in browser mode");
    }
  }, [currentFile, activeFile, projectPath]);

  // ── Global Keyboard Shortcuts ───────────────────────────────────
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const mod = e.ctrlKey || e.metaKey;

      // Ctrl+S / Cmd+S: Save file
      if (mod && e.key === "s") {
        e.preventDefault();
        handleSaveFile();
        return;
      }

      // Ctrl+W / Cmd+W: Close current tab
      if (mod && e.key === "w" && activeFile) {
        e.preventDefault();
        closeFile(activeFile);
        return;
      }

      // Ctrl+B / Cmd+B: Toggle explorer
      if (mod && e.key === "b") {
        e.preventDefault();
        setShowExplorer((prev) => !prev);
        return;
      }

      // Ctrl+Shift+F / Cmd+Shift+F: Open search
      if (mod && e.shiftKey && e.key === "F") {
        e.preventDefault();
        setActiveView("search");
        return;
      }

      // Escape: Dismiss completion, close side panel
      if (e.key === "Escape") {
        if (completionText || completionId) {
          handleDismissCompletion();
          return;
        }
        if (activeView !== "editor") {
          setActiveView("editor");
          return;
        }
      }

      // Ctrl+\: Toggle chat panel
      if (mod && e.key === "\\") {
        e.preventDefault();
        setActiveView((prev) => prev === "chat" ? "editor" : "chat");
        return;
      }

      // Ctrl+,: Open settings
      if (mod && e.key === ",") {
        e.preventDefault();
        setActiveView("settings");
        return;
      }

      // Ctrl+`: Toggle terminal panel
      if (mod && e.key === "`") {
        e.preventDefault();
        setShowTerminal((prev) => !prev);
        return;
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [activeFile, currentFile, completionText, completionId, activeView, handleSaveFile, handleDismissCompletion, closeFile, showTerminal]);

  return (
    <div className="app-container">
      <div className="app-body">
        {/* File Explorer */}
        {showExplorer && projectPath && (
          <div className="explorer-panel">
            <FileExplorer
              projectPath={projectPath}
              onFileSelect={handleFileSelect}
              onClose={() => setShowExplorer(false)}
            />
          </div>
        )}

        {/* Main Editor Area */}
        <div className="editor-area">
          <div className="editor-header">
            <div className="editor-header-left">
              <button
                className="editor-toolbar-btn"
                onClick={() => setShowExplorer(!showExplorer)}
                title="Toggle Explorer"
              >
                <PanelLeft size={15} />
              </button>
              {completionId && (
                <button
                  className="editor-toolbar-btn cancel-completion-btn"
                  onClick={handleCancelCompletion}
                  title="Cancel completion"
                >
                  <Square size={11} /> Cancel
                </button>
              )}
            </div>
            <div className="editor-tabs">
              {openFiles.map((file) => (
                <div
                  key={file.path}
                  className={`tab ${file.path === activeFile ? "active" : ""}`}
                  onClick={() => setActiveFile(file.path)}
                  onMouseDown={(e) => {
                    // Middle-click closes the tab
                    if (e.button === 1) {
                      e.preventDefault();
                      closeFile(file.path);
                    }
                  }}
                  onContextMenu={(e) => handleTabContextMenu(e, file.path)}
                >
                  <span className="tab-icon">{getFileIcon(file.name)}</span>
                  <span className="tab-name">{file.name}</span>
                  <button
                    className="tab-close"
                    onClick={(e) => {
                      e.stopPropagation();
                      closeFile(file.path);
                    }}
                  >
                    ✕
                  </button>
                </div>
              ))}
            </div>
            <div className="editor-header-right">
              {currentFile && (
                <>
                  <button
                    className="editor-toolbar-btn"
                    onClick={handleFind}
                    title="Find in File (Ctrl+F)"
                  >
                    <Search size={15} />
                  </button>
                  <button
                    className={`editor-toolbar-btn ${showOutline ? "active" : ""}`}
                    onClick={handleToggleOutline}
                    title="Toggle Outline"
                  >
                    <ListTree size={15} />
                  </button>
                </>
              )}
            </div>
          </div>

          <div className="editor-content">
            {!projectPath ? (
              <div className="welcome-screen">
                <div className="welcome-logo">NeeCoder</div>
                <p className="welcome-subtitle">Your AI Coding Assistant</p>
                <p className="welcome-desc">
                  Open a project to start coding with AI-powered assistance
                </p>
                <div className="welcome-actions">
                  <button className="btn-primary" onClick={handleOpenProject}>
                    Open Project
                  </button>
                </div>
                <div className="welcome-features">
                  <div className="feature-card">
                    <span className="feature-icon"><MessageSquare size={18} /></span>
                    <span>AI Chat</span>
                  </div>
                  <div className="feature-card">
                    <span className="feature-icon"><Sparkles size={18} /></span>
                    <span>Code Completion</span>
                  </div>
                  <div className="feature-card">
                    <span className="feature-icon"><Search size={18} /></span>
                    <span>Codebase Search</span>
                  </div>
                  <div className="feature-card">
                    <span className="feature-icon"><BookOpen size={18} /></span>
                    <span>Smart Context</span>
                  </div>
                </div>
              </div>
            ) : currentFile ? (
              <div className="code-view">
                <div className="code-view-header">
                  <span className="code-view-path">{currentFile.path}</span>
                </div>
                <div className="code-view-body">
                  <div className="code-view-editor">
                    <CodeEditor
                      content={currentFile.content}
                      filePath={currentFile.path}
                      projectPath={projectPath}
                      onContentChange={(content) => handleContentChange(currentFile.path, content)}
                      onCursorMove={handleCursorMove}
                      completionText={completionText}
                      onAcceptCompletion={handleAcceptCompletion}
                      onDismissCompletion={handleDismissCompletion}
                      onCycleCompletion={handleCycleCompletion}
                      candidateInfo={candidatesCount > 1 ? { index: candidateIndex, total: candidatesCount } : null}
                    />
                  </div>
                  {showOutline && (
                    <div className="outline-panel">
                      <div className="outline-header">
                        <span>📑 Outline</span>
                        <button
                          className="outline-close"
                          onClick={() => setShowOutline(false)}
                        >
                          ✕
                        </button>
                      </div>
                      <div className="outline-list">
                        {outlineLoading ? (
                          <div className="outline-loading">Loading...</div>
                        ) : outlineSymbols.length === 0 ? (
                          <div className="outline-empty">No symbols found</div>
                        ) : (
                          outlineSymbols.map((sym, idx) => (
                            <div
                              key={`${sym.name}-${idx}`}
                              className="outline-item"
                              onClick={() => handleGotoSymbol(sym.start_line)}
                              title={sym.detail || sym.name}
                            >
                              <span className="outline-icon">
                                {getSymbolIcon(sym.kind)}
                              </span>
                              <span className="outline-name">{sym.name}</span>
                              <span className="outline-line">
                                L{sym.start_line}
                              </span>
                            </div>
                          ))
                        )}
                      </div>
                    </div>
                  )}
                </div>
              </div>
            ) : (
              <div className="editor-placeholder">
                <p>Select a file from the explorer to start editing</p>
                <p className="hint">
                  Code completion is active. Open a file to see suggestions.
                </p>
              </div>
            )}
          </div>
        </div>

        {/* Side Panel (Chat / Settings / Search) — kept mounted so the
            width transition animates the open/close; content renders only when open */}
        <div
          className={`side-panel ${activeView !== "editor" ? "side-panel-open" : ""} ${resizingPanel ? "side-panel-resizing" : ""}`}
          style={{ width: activeView !== "editor" ? panelWidth : 0 }}
        >
          {activeView !== "editor" && (
            <div className="side-panel-resizer" onMouseDown={startPanelResize} title="Drag to resize" />
          )}
          {activeView === "chat" && <ChatPanel projectPath={projectPath} />}
          {activeView === "settings" && <Settings />}
          {activeView === "cloud" && <CloudAgentPanel />}
          {activeView === "checkpoints" && projectPath && <CheckpointPanel projectPath={projectPath} />}
          {activeView === "search" && projectPath && (
            <SearchPanel
              projectPath={projectPath}
              onFileSelect={handleSearchSelect}
            />
          )}
          {activeView === "graph" && projectPath && (
            <DependencyGraph
              projectPath={projectPath}
              onFileSelect={(label) => {
                // Try to find and open the file by label
                const file = openFiles.find((f) => f.name === label);
                if (file) {
                  setActiveFile(file.path);
                  setActiveView("editor");
                }
              }}
            />
          )}
          {activeView === "memory" && <MemoryPanel />}
          {activeView === "insights" && <InsightsPanel />}
        </div>
      </div>

      {/* Terminal Panel */}
      <div
        className={`terminal-wrapper ${showTerminal ? "terminal-open" : ""} ${resizingTerminal ? "terminal-resizing" : ""}`}
        style={showTerminal ? { height: terminalHeight } : undefined}
      >
        {showTerminal && (
          <div className="terminal-resizer" onMouseDown={startTerminalResize} title="Drag to resize" />
        )}
        <TerminalPanel />
      </div>

      {/* Status Bar */}
      <StatusBar
        llmConnected={llmConnected}
        localModelRunning={localModelRunning}
        projectPath={projectPath || ""}
        fileCount={openFiles.length}
        activeView={activeView}
        theme={theme}
        onToggleTheme={toggleTheme}
        onChatClick={() =>
          setActiveView(activeView === "chat" ? "editor" : "chat")
        }
        onSettingsClick={() =>
          setActiveView(activeView === "settings" ? "editor" : "settings")
        }
        onSearchClick={() =>
          setActiveView(activeView === "search" ? "editor" : "search")
        }
        onCloudClick={() =>
          setActiveView(activeView === "cloud" ? "editor" : "cloud")
        }
        onGraphClick={() =>
          setActiveView(activeView === "graph" ? "editor" : "graph")
        }
        onMemoryClick={() =>
          setActiveView(activeView === "memory" ? "editor" : "memory")
        }
        onInsightsClick={() =>
          setActiveView(activeView === "insights" ? "editor" : "insights")
        }
        onCheckpointsClick={() =>
          setActiveView(activeView === "checkpoints" ? "editor" : "checkpoints")
        }
        onExplorerClick={() => setShowExplorer(!showExplorer)}
      />

      {/* Tab Context Menu */}
      {contextMenu && (
        <ContextMenu
          x={contextMenu.x}
          y={contextMenu.y}
          onClose={() => setContextMenu(null)}
          items={[
            {
              label: "Close",
              icon: "✕",
              shortcut: "Ctrl+W",
              action: () => closeTab(contextMenu.filePath),
            },
            {
              label: "Close Others",
              icon: "⊟",
              action: () => closeOtherTabs(contextMenu.filePath),
            },
            {
              label: "Close All",
              icon: "⊠",
              action: closeAllTabs,
            },
            {
              label: "Copy Path",
              icon: "📋",
              action: () => copyFilePath(contextMenu.filePath),
            },
          ]}
        />
      )}
    </div>
  );
}

function getFileIcon(name: string) {
  const ext = name.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "rs": return <Braces size={14} color="#e0654f" />;
    case "ts":
    case "tsx": return <FileCode2 size={14} color="#2f74c0" />;
    case "js":
    case "jsx": return <FileCode2 size={14} color="#c9b116" />;
    case "py": return <FileCode size={14} color="#4f8cc9" />;
    case "go": return <FileCode2 size={14} color="#00ADD8" />;
    case "java": return <Coffee size={14} color="#b07219" />;
    case "css":
    case "scss": return <Palette size={14} color="#563d7c" />;
    case "json": return <FileJson size={14} color="#c9b116" />;
    case "toml": return <FileCog size={14} color="#8a8aaa" />;
    case "md": return <FileText size={14} color="#6c7086" />;
    case "yml":
    case "yaml": return <FileType size={14} color="#8a8aaa" />;
    case "html": return <Globe size={14} color="#e34c26" />;
    case "sql": return <Database size={14} color="#6c7086" />;
    default: return <File size={14} color="#8a8aaa" />;
  }
}

function getSymbolIcon(kind: string) {
  const k = kind.toLowerCase();
  if (k.includes("function") || k.includes("method")) return <Wrench size={13} />;
  if (k.includes("class")) return <Landmark size={13} />;
  if (k.includes("interface")) return <Shapes size={13} />;
  if (k.includes("struct")) return <Box size={13} />;
  if (k.includes("enum")) return <Layers size={13} />;
  if (k.includes("variable") || k.includes("const") || k.includes("field")) return <MapPin size={13} />;
  if (k.includes("module") || k.includes("namespace")) return <Folder size={13} />;
  if (k.includes("type")) return <Tag size={13} />;
  if (k.includes("macro")) return <Zap size={13} />;
  if (k.includes("trait")) return <Target size={13} />;
  return <Circle size={13} />;
}

function detectLanguage(filePath: string): string {
  const ext = filePath.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "rs": return "rust";
    case "ts":
    case "tsx": return "typescript";
    case "js":
    case "jsx": return "javascript";
    case "py": return "python";
    case "go": return "go";
    case "java": return "java";
    case "html": return "html";
    case "css":
    case "scss":
    case "less": return "css";
    case "json": return "json";
    case "md":
    case "markdown": return "markdown";
    case "sql": return "sql";
    case "sh":
    case "bash": return "bash";
    default: return "plaintext";
  }
}

export default App;
