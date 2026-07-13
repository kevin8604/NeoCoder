import { useState, useEffect } from "react";

interface OverlayProps {}

export default function Overlay(_props: OverlayProps) {
  const [completion, setCompletion] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  // Listen for completion events from Tauri
  useEffect(() => {
    let unlisten: (() => void) | undefined;

    async function setup() {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<string>("completion-event", (event) => {
        const payload = event.payload as any;
        if (payload && payload.token) {
          setCompletion((prev) => (prev || "") + payload.token);
          setLoading(false);
        } else if (payload && payload.full_text) {
          setCompletion(payload.full_text);
          setLoading(false);
        } else if (payload && payload.id) {
          setLoading(true);
          setCompletion(null);
        } else if (payload && payload.message) {
          setLoading(false);
          console.error("Completion error:", payload.message);
        }
      });
    }

    setup();

    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // Global keyboard listener for Tab and Escape
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape" && completion) {
        setCompletion(null);
        e.preventDefault();
      } else if (e.key === "Tab" && completion) {
        e.preventDefault();
        // Accept completion - emit event back to backend
        // TODO: Insert completion text into editor
        setCompletion(null);
      } else if (e.key === "]" && e.altKey && completion) {
        e.preventDefault();
        // Cycle to next completion
        // TODO: Request next completion candidate
      }
    }

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [completion]);

  if (loading) {
    return (
      <div className="completion-loading">
        <span className="loading-dot" />
        <span>Generating completion...</span>
      </div>
    );
  }

  if (!completion) return null;

  return (
    <div className="completion-overlay">
      <code>{completion}</code>
      <div style={{ fontSize: 11, color: "var(--text-muted)", marginTop: 4 }}>
        Tab to accept &middot; Esc to dismiss
      </div>
    </div>
  );
}
