import { useState, useEffect, useCallback } from "react";
import {
  listCloudTasks,
  cancelCloudTask,
  listenToEvent,
  type CloudTask,
} from "../hooks/useTauri";

export default function CloudAgentPanel() {
  const [tasks, setTasks] = useState<CloudTask[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadTasks = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await listCloudTasks();
      setTasks(result);
    } catch {
      setError("Failed to load tasks");
    }
    setLoading(false);
  }, []);

  useEffect(() => {
    loadTasks();
  }, [loadTasks]);

  // Listen for cloud agent events (task completed/failed)
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      unlisten = (await listenToEvent<any>("cloud-agent-event", () => {
        loadTasks();
      })) ?? undefined;
    })();
    return () => { if (unlisten) unlisten(); };
  }, [loadTasks]);

  // Auto-refresh for running tasks
  useEffect(() => {
    const hasRunning = tasks.some((t) => t.status === "pending" || t.status === "running");
    if (!hasRunning) return;
    const interval = setInterval(loadTasks, 3000);
    return () => clearInterval(interval);
  }, [tasks, loadTasks]);

  const handleCancel = async (taskId: string) => {
    await cancelCloudTask(taskId);
    loadTasks();
  };

  const formatTime = (ts: number): string => {
    const d = new Date(ts * 1000);
    return d.toLocaleTimeString();
  };

  const statusLabel = (status: string): string => {
    switch (status) {
      case "pending": return "Pending";
      case "running": return "Running";
      case "completed": return "Completed";
      case "failed": return "Failed";
      case "cancelled": return "Cancelled";
      default: return status;
    }
  };

  const statusClass = (status: string): string => {
    switch (status) {
      case "completed": return "status-dot connected";
      case "running": return "status-dot connecting";
      case "failed": return "status-dot disconnected";
      case "cancelled": return "status-dot disconnected";
      default: return "status-dot connecting";
    }
  };

  return (
    <div className="cloud-agent-panel">
      <div className="cloud-agent-header">
        <h3>☁️ Cloud Agents</h3>
        <button className="file-explorer-action-btn" onClick={loadTasks} title="Refresh">
          ↻
        </button>
      </div>

      {error && (
        <div className="file-explorer-error">
          {error}
          <button onClick={() => setError(null)}>✕</button>
        </div>
      )}

      <div className="cloud-agent-list">
        {loading && tasks.length === 0 ? (
          <div className="cloud-agent-empty">Loading...</div>
        ) : tasks.length === 0 ? (
          <div className="cloud-agent-empty">
            <p>No cloud tasks</p>
            <p className="hint">
              Start a cloud agent from the chat panel to run tasks in the background.
            </p>
          </div>
        ) : (
          tasks
            .slice()
            .sort((a, b) => b.created_at - a.created_at)
            .map((task) => (
              <div key={task.id} className={`cloud-task-card ${task.status}`}>
                <div className="cloud-task-header">
                  <span className={statusClass(task.status)} />
                  <span className="cloud-task-status">
                    {statusLabel(task.status)}
                  </span>
                  <span className="cloud-task-time">
                    {formatTime(task.created_at)}
                  </span>
                  {(task.status === "pending" || task.status === "running") && (
                    <button
                      className="cloud-task-cancel"
                      onClick={() => handleCancel(task.id)}
                      title="Cancel"
                    >
                      ✕
                    </button>
                  )}
                </div>
                <div className="cloud-task-message">{task.message}</div>
                {task.result && (
                  <div className="cloud-task-result">
                    <details>
                      <summary>View result</summary>
                      <pre>{task.result.slice(0, 1000)}</pre>
                    </details>
                  </div>
                )}
                {task.pr_url && (
                  <div className="cloud-task-pr">
                    🔗{" "}
                    <a href={task.pr_url} target="_blank" rel="noopener noreferrer">
                      Pull Request
                    </a>
                  </div>
                )}
                {task.completed_at && (
                  <div className="cloud-task-completed">
                    {task.status === "completed" ? "Completed" : task.status === "failed" ? "Failed" : "Ended"}{" "}
                    at {formatTime(task.completed_at)}
                  </div>
                )}
              </div>
            ))
        )}
      </div>

      {tasks.some((t) => t.status === "running") && (
        <div className="cloud-agent-footer">
          <span className="loading-dot" /> Tasks running...
        </div>
      )}
    </div>
  );
}
