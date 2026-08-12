import { useState, useEffect, useCallback } from "react";
import {
  getSessions,
  listCheckpoints,
  checkpointDiff,
  restoreCheckpoint,
  type Checkpoint,
  type FileChange,
} from "../hooks/useTauri";
import {
  History,
  RefreshCw,
  GitCompare,
  RotateCcw,
  ChevronDown,
  ChevronRight,
  X,
} from "lucide-react";

interface CheckpointEntry extends Checkpoint {
  sessionId: string;
  sessionTitle: string;
}

export default function CheckpointPanel({ projectPath }: { projectPath: string }) {
  const [entries, setEntries] = useState<CheckpointEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<{ sessionId: string; iteration: number } | null>(null);
  const [diff, setDiff] = useState<FileChange[] | null>(null);
  const [diffLoading, setDiffLoading] = useState(false);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<{ sessionId: string; iteration: number } | null>(null);

  const loadCheckpoints = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const sessions = await getSessions();
      const all: CheckpointEntry[] = [];
      for (const s of sessions) {
        const cps = await listCheckpoints(s.id);
        for (const cp of cps) {
          all.push({ ...cp, sessionId: s.id, sessionTitle: s.title || s.id.slice(0, 8) });
        }
      }
      all.sort((a, b) => b.timestamp - a.timestamp);
      setEntries(all);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    loadCheckpoints();
  }, [loadCheckpoints]);

  const handleToggleDiff = async (entry: CheckpointEntry) => {
    if (expanded?.sessionId === entry.sessionId && expanded.iteration === entry.iteration) {
      setExpanded(null);
      setDiff(null);
      setDiffError(null);
      return;
    }
    setExpanded({ sessionId: entry.sessionId, iteration: entry.iteration });
    setDiff(null);
    setDiffError(null);
    setDiffLoading(true);
    try {
      const result = await checkpointDiff(entry.sessionId, entry.iteration, projectPath);
      setDiff(result);
    } catch (e) {
      setDiffError(e instanceof Error ? e.message : String(e));
    }
    setDiffLoading(false);
  };

  const handleRestore = async (entry: CheckpointEntry) => {
    if (!window.confirm(`Restore files to checkpoint iteration ${entry.iteration}?\n\nThis will git-checkout the files from that snapshot.`)) {
      return;
    }
    setRestoring({ sessionId: entry.sessionId, iteration: entry.iteration });
    try {
      await restoreCheckpoint(entry.sessionId, entry.iteration, projectPath);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
    setRestoring(null);
  };

  const formatTime = (ts: number): string => {
    const d = new Date(ts * 1000);
    return d.toLocaleString();
  };

  const diffLineClass = (type: string): string => {
    switch (type) {
      case "add": return "ckpt-diff-add";
      case "remove": return "ckpt-diff-remove";
      case "hunk": return "ckpt-diff-hunk";
      default: return "ckpt-diff-context";
    }
  };

  return (
    <div className="cloud-agent-panel">
      <div className="cloud-agent-header">
        <h3><History size={15} /> Checkpoints</h3>
        <button className="file-explorer-action-btn" onClick={loadCheckpoints} title="Refresh">
          <RefreshCw size={13} />
        </button>
      </div>

      {error && (
        <div className="file-explorer-error">
          {error}
          <button onClick={() => setError(null)}><X size={12} /></button>
        </div>
      )}

      <div className="cloud-agent-list">
        {loading && entries.length === 0 ? (
          <div className="cloud-agent-empty">Loading...</div>
        ) : entries.length === 0 ? (
          <div className="cloud-agent-empty">
            <p>No checkpoints yet</p>
            <p className="hint">
              Agent runs create a git-commit checkpoint at each iteration that modifies files.
            </p>
          </div>
        ) : (
          entries.map((entry) => {
            const isExpanded =
              expanded?.sessionId === entry.sessionId && expanded.iteration === entry.iteration;
            return (
              <div key={`${entry.sessionId}-${entry.iteration}`} className="cloud-task-card completed">
                <div className="cloud-task-header">
                  <span className="status-dot connected" />
                  <span className="cloud-task-status">Iteration {entry.iteration}</span>
                  <span className="cloud-task-time">{formatTime(entry.timestamp)}</span>
                </div>
                <div className="cloud-task-message">
                  {entry.description}
                  {entry.files.length > 0 && (
                    <span className="ckpt-files"> ({entry.files.length} file{entry.files.length > 1 ? "s" : ""})</span>
                  )}
                </div>
                <div className="ckpt-meta">
                  {entry.sessionTitle} · {entry.commit_hash ? entry.commit_hash.slice(0, 7) : "no commit"}
                </div>
                <div className="ckpt-actions">
                  <button
                    className="file-explorer-action-btn"
                    onClick={() => handleToggleDiff(entry)}
                    title={isExpanded ? "Hide diff" : "View diff"}
                  >
                    {isExpanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                    <span style={{ marginLeft: 4 }}>Diff</span>
                  </button>
                  {entry.commit_hash && (
                    <button
                      className="file-explorer-action-btn"
                      onClick={() => handleRestore(entry)}
                      disabled={restoring !== null}
                      title="Restore files to this checkpoint"
                    >
                      <RotateCcw size={13} />
                      <span style={{ marginLeft: 4 }}>
                        {restoring?.sessionId === entry.sessionId && restoring.iteration === entry.iteration
                          ? "Restoring..."
                          : "Restore"}
                      </span>
                    </button>
                  )}
                </div>

                {isExpanded && (
                  <div className="ckpt-diff">
                    {diffLoading && <div className="ckpt-diff-loading">Loading diff...</div>}
                    {diffError && <div className="ckpt-diff-error">{diffError}</div>}
                    {!diffLoading && !diffError && diff && diff.length === 0 && (
                      <div className="ckpt-diff-error">No changes in this checkpoint.</div>
                    )}
                    {!diffLoading && !diffError && diff && diff.length > 0 && (
                      <div className="ckpt-diff-files">
                        {diff.map((file) => (
                          <details key={file.file_path} open>
                            <summary className="ckpt-diff-file">
                              <GitCompare size={12} /> {file.file_path}
                            </summary>
                            <div className="ckpt-diff-lines">
                              {file.hunks.map((hunk, i) => (
                                <div key={i} className={`ckpt-diff-line ${diffLineClass(hunk.type)}`}>
                                  {hunk.type === "hunk" ? (
                                    hunk.content
                                  ) : (
                                    <>
                                      <span className="ckpt-diff-ln">{hunk.old_start || ""}</span>
                                      <span className="ckpt-diff-ln">{hunk.new_start || ""}</span>
                                      <span className="ckpt-diff-content">{hunk.content}</span>
                                    </>
                                  )}
                                </div>
                              ))}
                            </div>
                          </details>
                        ))}
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })
        )}
      </div>
    </div>
  );
}
