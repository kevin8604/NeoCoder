import { useEffect, useRef, useCallback, useState } from "react";

/**
 * TerminalPanel — xterm.js frontend connected to Tauri PTY backend
 *
 * Architecture:
 * - On mount, calls `start_terminal` Tauri command to spawn a shell
 * - Listens to "pty-output" events from backend and writes to xterm
 * - On user input, sends data via `write_stdin` Tauri command
 * - Handles terminal resize via `resize_terminal` command
 * - Cleans up (stop_terminal) on unmount
 */

// Terminal dimensions (cols x rows)
const DEFAULT_COLS = 80;
const DEFAULT_ROWS = 24;

export default function TerminalPanel() {
  const terminalRef = useRef<HTMLDivElement>(null);
  const xtermRef = useRef<any>(null);
  const fitAddonRef = useRef<any>(null);
  const [isRunning, setIsRunning] = useState(false);
  const [status, setStatus] = useState<string>("Initializing...");
  const unlistenRef = useRef<Array<() => void>>([]);

  // ── Initialize xterm.js ────────────────────────────────────────
  useEffect(() => {
    let mounted = true;
    let xterm: any = null;
    let fitAddon: any = null;

    async function init() {
      try {
        // Dynamically import xterm and addon
        const { Terminal } = await import("@xterm/xterm");
        const { FitAddon } = await import("@xterm/addon-fit");

        if (!mounted || !terminalRef.current) return;

        // Create FitAddon
        fitAddon = new FitAddon();
        fitAddonRef.current = fitAddon;

        // Determine theme based on data-theme attribute
        const isDark = document.documentElement.getAttribute("data-theme") !== "light";
        const theme = isDark
          ? {
              background: "#1e1e2e",
              foreground: "#cdd6f4",
              cursor: "#89b4fa",
              selectionBackground: "rgba(137, 180, 250, 0.3)",
              black: "#45475a",
              red: "#f38ba8",
              green: "#a6e3a1",
              yellow: "#f9e2af",
              blue: "#89b4fa",
              magenta: "#f5c2e7",
              cyan: "#94e2d5",
              white: "#bac2de",
              brightBlack: "#585b70",
              brightRed: "#f38ba8",
              brightGreen: "#a6e3a1",
              brightYellow: "#f9e2af",
              brightBlue: "#89b4fa",
              brightMagenta: "#f5c2e7",
              brightCyan: "#94e2d5",
              brightWhite: "#a6adc8",
            }
          : {
              background: "#ffffff",
              foreground: "#1a1a2e",
              cursor: "#5b6abf",
              selectionBackground: "rgba(91, 106, 191, 0.2)",
              black: "#d0d7de",
              red: "#cf222e",
              green: "#2da44e",
              yellow: "#bf8700",
              blue: "#5b6abf",
              magenta: "#8250df",
              cyan: "#1b7c83",
              white: "#4a4a6a",
              brightBlack: "#8a8aaa",
              brightRed: "#cf222e",
              brightGreen: "#2da44e",
              brightYellow: "#bf8700",
              brightBlue: "#5b6abf",
              brightMagenta: "#8250df",
              brightCyan: "#1b7c83",
              brightWhite: "#1a1a2e",
            };

        // Create terminal
        xterm = new Terminal({
          cols: DEFAULT_COLS,
          rows: DEFAULT_ROWS,
          cursorBlink: true,
          cursorStyle: "block",
          fontSize: 13,
          fontFamily: "'Fira Code', 'Cascadia Code', 'JetBrains Mono', 'Consolas', monospace",
          theme,
          allowTransparency: false,
          disableStdin: false,
          convertEol: true,
          scrollback: 5000,
        });
        xtermRef.current = xterm;

        // Open terminal in the container div
        xterm.open(terminalRef.current);

        // Load FitAddon
        xterm.loadAddon(fitAddon);
        fitAddon.fit();

        // Handle resize
        const handleResize = () => {
          try {
            fitAddon.fit();
            const dims = fitAddon.proposeDimensions();
            if (dims && xtermRef.current) {
              import("@tauri-apps/api/core").then(({ invoke }) => {
                invoke("resize_terminal", {
                  cols: dims.cols,
                  rows: dims.rows,
                }).catch(() => {});
              });
            }
          } catch {
            // Ignore resize errors
          }
        };

        window.addEventListener("resize", handleResize);

        // ── Setup Tauri event listeners ────────────────────────
        const { listen } = await import("@tauri-apps/api/event");

        // Listen for pty output
        const unlistenOutput = await listen<string>("pty-output", (event) => {
          if (xtermRef.current) {
            xtermRef.current.write(event.payload);
          }
        });

        // Listen for pty errors
        const unlistenError = await listen<string>("pty-error", (event) => {
          if (xtermRef.current) {
            xtermRef.current.write(`\r\n\x1b[31m[ERROR]\x1b[0m ${event.payload}\r\n`);
          }
        });

        // Listen for pty exit
        const unlistenExit = await listen<string>("pty-exit", (event) => {
          if (xtermRef.current) {
            xtermRef.current.write(`\r\n\x1b[33m${event.payload}\x1b[0m\r\n`);
          }
          setIsRunning(false);
          setStatus("Terminated");
        });

        unlistenRef.current = [unlistenOutput, unlistenError, unlistenExit];

        // ── Handle user input ──────────────────────────────────
        xterm.onData((data: string) => {
          import("@tauri-apps/api/core").then(({ invoke }) => {
            invoke("write_stdin", { data }).catch((err) => {
              console.error("Failed to write to terminal:", err);
            });
          });
        });

        // ── Start the terminal process ─────────────────────────
        try {
          const { invoke } = await import("@tauri-apps/api/core");
          await invoke("start_terminal");
          setIsRunning(true);
          setStatus("Ready");
          // Write initial newline to get a prompt
          setTimeout(() => {
            if (xtermRef.current) {
              xtermRef.current.write("\r\n");
            }
          }, 200);
        } catch (err: any) {
          setStatus(`Error: ${err}`);
          if (xtermRef.current) {
            xtermRef.current.write(
              `\r\n\x1b[31mFailed to start terminal: ${err}\x1b[0m\r\n`
            );
          }
        }
      } catch (err: any) {
        if (mounted) {
          setStatus(`Error: ${err?.message || err}`);
        }
      }
    }

    init();

    return () => {
      mounted = false;
      // Cleanup event listeners
      unlistenRef.current.forEach((fn) => fn());
      unlistenRef.current = [];
      // Stop terminal
      import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke("stop_terminal").catch(() => {}))
        .catch(() => {});
      // Dispose xterm
      if (xterm) {
        xterm.dispose();
        xtermRef.current = null;
      }
    };
  }, []);

  // ── Handle terminal panel resize via ResizeObserver ──────────
  useEffect(() => {
    if (!terminalRef.current) return;

    const observer = new ResizeObserver(() => {
      if (fitAddonRef.current) {
        try {
          fitAddonRef.current.fit();
          const dims = fitAddonRef.current.proposeDimensions();
          if (dims) {
            import("@tauri-apps/api/core").then(({ invoke }) => {
              invoke("resize_terminal", {
                cols: dims.cols,
                rows: dims.rows,
              }).catch(() => {});
            });
          }
        } catch {
          // Ignore
        }
      }
    });

    observer.observe(terminalRef.current);
    return () => observer.disconnect();
  }, []);

  // ── Handle restart ────────────────────────────────────────────
  const handleRestart = useCallback(async () => {
    setStatus("Restarting...");
    try {
      const { invoke } = await import("@tauri-apps/api/core");
      await invoke("stop_terminal");
      await invoke("start_terminal");
      setIsRunning(true);
      setStatus("Ready");
      if (xtermRef.current) {
        xtermRef.current.clear();
        xtermRef.current.write("\r\n");
      }
    } catch (err: any) {
      setStatus(`Error: ${err}`);
    }
  }, []);

  return (
    <div className="terminal-panel">
      <div className="terminal-header">
        <div className="terminal-header-left">
          <span className="terminal-icon"></span>
          <span className="terminal-title">Terminal</span>
          <span className={`terminal-status ${isRunning ? "running" : "stopped"}`}>
            {status}
          </span>
        </div>
        <div className="terminal-header-right">
          <button
            className="terminal-toolbar-btn"
            onClick={handleRestart}
            title="Restart Terminal"
          >
            ⟳
          </button>
        </div>
      </div>
      <div className="terminal-body" ref={terminalRef} />
    </div>
  );
}
