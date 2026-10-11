import { useState, useEffect, useCallback } from "react";
import {
  getFailureLessons,
  deleteFailureLesson,
  clearFailureLessons,
  pruneFailureLessons,
  type FailureLesson,
} from "../hooks/useTauri";
import {
  Bug,
  RefreshCw,
  Trash2,
  Eraser,
  Wand2,
  ChevronDown,
  ChevronRight,
} from "lucide-react";

/**
 * Failure Lessons panel — the management side of the cross-session failure
 * loop. The agent already captures error signatures (FailureMemoryHook) and
 * injects them into future system prompts; this lets the human see what was
 * learned, dismiss false positives, prune stale patterns, or clear everything.
 */
export default function FailureLessonsPanel() {
  const [lessons, setLessons] = useState<FailureLesson[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set());

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const list = await getFailureLessons();
      if (list.length === 0 && !(await isDesktop())) {
        setError("失败教训仅在桌面应用中可用（非浏览器预览）。");
      }
      setLessons(list);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const dismiss = useCallback(async (l: FailureLesson) => {
    await deleteFailureLesson(l.project, l.signature);
    setLessons((prev) =>
      prev.filter((x) => !(x.project === l.project && x.signature === l.signature)),
    );
  }, []);

  const prune = useCallback(async () => {
    const removed = await pruneFailureLessons(90);
    if (removed !== null) await refresh();
  }, [refresh]);

  const clearAll = useCallback(async () => {
    const removed = await clearFailureLessons();
    if (removed !== null) setLessons([]);
  }, []);

  const toggleProject = (p: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev);
      if (next.has(p)) {
        next.delete(p);
      } else {
        next.add(p);
      }
      return next;
    });

  // Group lessons by project (backend already sorts most-recent-first overall).
  const byProject = new Map<string, FailureLesson[]>();
  for (const l of lessons) {
    const arr = byProject.get(l.project) ?? [];
    arr.push(l);
    byProject.set(l.project, arr);
  }

  return (
    <div className="lessons-panel">
      <div className="lessons-header">
        <span className="lessons-title">
          <Bug size={15} /> Failure Lessons
          <span className="lessons-count">{lessons.length}</span>
        </span>
        <div className="lessons-header-actions">
          <button className="eval-btn" onClick={refresh} disabled={loading} title="刷新">
            <RefreshCw size={13} className={loading ? "spinning" : ""} />
          </button>
          <button
            className="eval-btn"
            onClick={prune}
            disabled={loading || lessons.length === 0}
            title="清理 90 天未再出现的过时教训"
          >
            <Wand2 size={13} />
            <span>清理过时</span>
          </button>
          <button
            className="eval-btn eval-btn-danger"
            onClick={clearAll}
            disabled={loading || lessons.length === 0}
            title="清空全部教训"
          >
            <Eraser size={13} />
            <span>清空</span>
          </button>
        </div>
      </div>

      <p className="lessons-hint">
        Agent 会在 build/test/终端/诊断失败时记录错误签名，并在后续会话把这些教训注入
        system prompt。此处可审阅、逐条忽略误报，或清理过时/全部。
      </p>

      {error && <div className="eval-error">{error}</div>}

      <div className="lessons-list">
        {lessons.length === 0 && !loading && !error && (
          <div className="eval-empty">暂无记录的失败教训。</div>
        )}
        {[...byProject.entries()].map(([project, items]) => {
          const isCollapsed = collapsed.has(project);
          return (
            <div key={project} className="lessons-group">
              <div className="lessons-proj-row" onClick={() => toggleProject(project)}>
                <span className="eval-caret">
                  {isCollapsed ? (
                    <ChevronRight size={13} />
                  ) : (
                    <ChevronDown size={13} />
                  )}
                </span>
                <span className="lessons-proj" title={project}>
                  {basename(project)}
                </span>
                <span className="lessons-proj-count">{items.length}</span>
              </div>
              {!isCollapsed &&
                items.map((l) => (
                  <div key={`${l.project}::${l.signature}`} className="lessons-item">
                    <span className="lessons-sig" title={l.signature}>
                      {l.signature}
                    </span>
                    <span className="lessons-badge">×{l.count}</span>
                    <span className="lessons-tool">{l.tool}</span>
                    {l.file && (
                      <span className="lessons-loc" title={`${l.file}${l.line != null ? `:${l.line}` : ""}`}>
                        {l.file}
                        {l.line != null ? `:${l.line}` : ""}
                      </span>
                    )}
                    <span className="lessons-age">{relativeAge(l.last_seen)}</span>
                    <button
                      className="eval-btn eval-btn-sm"
                      onClick={() => dismiss(l)}
                      title="忽略此教训"
                    >
                      <Trash2 size={11} />
                    </button>
                    {l.snippet && <div className="lessons-snippet">{l.snippet}</div>}
                  </div>
                ))}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function basename(p: string): string {
  const parts = p.split(/[/\\]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : p;
}

function relativeAge(iso: string): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const days = Math.floor((Date.now() - t) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "1d ago";
  if (days < 30) return `${days}d ago`;
  return `${Math.floor(days / 30)}mo ago`;
}

async function isDesktop(): Promise<boolean> {
  return "__TAURI_INTERNALS__" in window;
}
