import { useState, useEffect, useCallback } from "react";
import { Brain, FolderTree, Ruler, Scale, Calendar, Search, Eraser, Package } from "lucide-react";
import {
  previewMemory,
  listNotes,
  readNote,
  getMemoryStats,
  getMemoryEntries,
  cleanupMemory,
  runDeepDreaming,
  exportTrainingData,
  searchMemory,
  listenToEvent,
  type MemSearchResult,
  type MemEntry,
} from "../hooks/useTauri";

interface NoteInfo {
  date: string;
  chars: number;
  preview: string;
}

interface ProgressEvent {
  phase: string;
  progress?: number;
  message?: string;
}

// ── 工具函数 ────────────────────────────────────────────────────────────────

function fmtStat(v: unknown): string {
  if (typeof v === "number") return v.toLocaleString();
  return String(v ?? 0);
}

/** 生成月份网格：返回 (day|null)[][]，day 为当月 1..31 */
function monthGrid(year: number, month: number): (number | null)[][] {
  const first = new Date(year, month, 1);
  const lead = first.getDay(); // 0=Sun
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const cells: (number | null)[] = [];
  for (let i = 0; i < lead; i++) cells.push(null);
  for (let d = 1; d <= daysInMonth; d++) cells.push(d);
  while (cells.length % 7 !== 0) cells.push(null);
  const weeks: (number | null)[][] = [];
  for (let i = 0; i < cells.length; i += 7) weeks.push(cells.slice(i, i + 7));
  return weeks;
}

/** 距上次召回的天数（基于 last_recalled 日期字符串） */
function daysSince(dateStr: string): number {
  const d = new Date(dateStr + "T00:00:00");
  if (isNaN(d.getTime())) return 0;
  return Math.max(0, Math.floor((Date.now() - d.getTime()) / 86400000));
}

/** Ebbinghaus 衰减曲线路径：R = e^(-t/S)，输出 SVG path d */
function decayCurvePath(
  stability: number,
  w: number,
  h: number,
  pad: number,
  maxDays: number
): string {
  const pts: string[] = [];
  for (let t = 0; t <= maxDays; t += 2) {
    const r = Math.exp(-t / stability);
    const x = pad + (t / maxDays) * (w - pad * 2);
    const y = h - pad - r * (h - pad * 2);
    pts.push(`${x.toFixed(1)},${y.toFixed(1)}`);
  }
  return pts.map((p, i) => (i === 0 ? `M${p}` : `L${p}`)).join(" ");
}

export default function MemoryPanel() {
  const [longTerm, setLongTerm] = useState<string>("");
  const [stats, setStats] = useState<Record<string, unknown> | null>(null);
  const [notes, setNotes] = useState<NoteInfo[]>([]);
  const [selectedDate, setSelectedDate] = useState<string | null>(null);
  const [noteContent, setNoteContent] = useState<string>("");
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState<"gc" | "dreaming" | "export" | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<ProgressEvent | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<MemSearchResult[] | null>(null);
  const [entries, setEntries] = useState<MemEntry[]>([]);
  // 日历状态：当前显示的月份（0-11）
  const now = new Date();
  const [calYear, setCalYear] = useState(now.getFullYear());
  const [calMonth, setCalMonth] = useState(now.getMonth());

  const noteDates = new Set(notes.map((n) => n.date));

  const loadAll = useCallback(async () => {
    setLoading(true);
    setError(null);
    const [pv, nt, me] = await Promise.all([previewMemory(), listNotes(), getMemoryEntries(200)]);
    if (pv) {
      setLongTerm(pv.long_term);
      setStats(pv.stats);
    }
    if (nt) setNotes(nt);
    if (me) setEntries(me);
    setLoading(false);
  }, []);

  useEffect(() => {
    loadAll();
  }, [loadAll]);

  // ── A2: 事件流监听（dreaming-progress / finetune-progress / memory-updated）──
  useEffect(() => {
    let unlisteners: (() => void)[] = [];
    (async () => {
      const onDreaming = (e: ProgressEvent) => {
        setProgress({ ...e, phase: "dreaming" });
        if (e.phase === "done") { setNotice("🧠 Deep Dreaming 完成 — 记忆已整合"); setTimeout(() => setNotice(null), 6000); }
        if (e.phase === "error") { setError(`Deep Dreaming 失败: ${e.message}`); setTimeout(() => setError(null), 8000); }
      };
      const onFinetune = (e: ProgressEvent) => {
        setProgress({ ...e, phase: "finetune" });
        if (e.phase === "exported") { setNotice("📦 训练数据集已导出"); setTimeout(() => setNotice(null), 6000); }
      };
      const onMemoryUpdated = () => { loadAll(); };
      const un1 = (await listenToEvent<any>("dreaming-progress", onDreaming)) ?? undefined;
      const un2 = (await listenToEvent<any>("finetune-progress", onFinetune)) ?? undefined;
      const un3 = (await listenToEvent<any>("memory-updated", onMemoryUpdated)) ?? undefined;
      unlisteners = [un1, un2, un3].filter(Boolean) as (() => void)[];
    })();
    return () => unlisteners.forEach((u) => u());
  }, [loadAll]);

  const handleCleanup = async () => {
    setBusy("gc");
    const report = await cleanupMemory();
    setBusy(null);
    if (report) setNotice(`🧹 GC 完成 — ${JSON.stringify(report)}`);
    setTimeout(() => setNotice(null), 6000);
    loadAll();
  };

  const handleDreaming = async () => {
    setBusy("dreaming");
    const report = await runDeepDreaming();
    setBusy(null);
    if (report) setNotice(`🧠 ${report}`);
    setTimeout(() => setNotice(null), 8000);
    loadAll();
  };

  const handleExport = async () => {
    setBusy("export");
    const path = await exportTrainingData();
    setBusy(null);
    if (path) setNotice(`📦 数据集: ${path}`);
    setTimeout(() => setNotice(null), 8000);
  };

  const handleSearch = async () => {
    if (!searchQuery.trim()) return;
    const results = await searchMemory(searchQuery, 8, true);
    setSearchResults(results ?? []);
  };

  const openNote = async (date: string) => {
    setSelectedDate(date);
    const content = await readNote(date);
    setNoteContent(content ?? "");
  };

  const statsCards = stats
    ? [
        { label: "长期记忆条目", value: fmtStat(stats.long_term_entries), icon: <Brain size={15} /> },
        { label: "分类数", value: fmtStat(stats.category_counts ? Object.keys(stats.category_counts as object).length : 0), icon: <FolderTree size={15} /> },
        { label: "总字符", value: fmtStat(stats.total_chars), icon: <Ruler size={15} /> },
        { label: "平均稳定度", value: fmtStat(stats.avg_stability), icon: <Scale size={15} /> },
        { label: "Daily Notes", value: fmtStat(stats.notes_count), icon: <Calendar size={15} /> },
        { label: "语义搜索", value: stats.semantic_search ? "开" : "关", icon: <Search size={15} /> },
      ]
    : [];

  const weeks = monthGrid(calYear, calMonth);

  // ── 记忆可视化数据：R 值分布分桶 + 衰减曲线散点 ──
  const buckets = Array.from({ length: 5 }, (_, i) => ({
    range: `${i * 20}-${(i + 1) * 20}%`,
    count: entries.filter((e) => e.retention >= i * 0.2 && e.retention < (i + 1) * 0.2).length,
  }));
  const maxBucket = Math.max(1, ...buckets.map((b) => b.count));
  const VIZ_MAX_DAYS = 60;
  const VIZ_W = 560;
  const VIZ_H = 190;
  const VIZ_PAD = 26;
  const refCurves = [
    { stability: 2, color: "#e05a5a", label: "S=2 (快速遗忘)" },
    { stability: 7, color: "#e0a05a", label: "S=7" },
    { stability: 30, color: "#5a9ae0", label: "S=30 (高稳定)" },
  ];
  const scatter = entries
    .map((e) => ({ e, days: daysSince(e.last_recalled) }))
    .filter(({ days }) => days <= VIZ_MAX_DAYS);

  return (
    <div className="memory-panel">
      <div className="memory-panel-header">
        <span className="memory-panel-title"><Brain size={15} /> Memory</span>
        <div className="memory-panel-actions">
          <button className="memory-btn" onClick={handleCleanup} disabled={busy !== null}>
            {busy === "gc" ? "清理中…" : <><Eraser size={13} /> GC</>}
          </button>
          <button className="memory-btn" onClick={handleDreaming} disabled={busy !== null}>
            {busy === "dreaming" ? "整合中…" : <><Brain size={13} /> Dream</>}
          </button>
          <button className="memory-btn" onClick={handleExport} disabled={busy !== null}>
            {busy === "export" ? "导出中…" : <><Package size={13} /> 导出数据集</>}
          </button>
        </div>
      </div>

      {notice && <div className="memory-notice">{notice}</div>}
      {error && <div className="memory-error">{error}</div>}
      {progress && progress.phase !== "done" && progress.phase !== "error" && (
        <div className="memory-progress">
          <div className="memory-progress-bar">
            <div
              className="memory-progress-fill"
              style={{ width: `${Math.round((progress.progress ?? 0) * 100)}%` }}
            />
          </div>
          <span>{progress.message ?? "处理中…"}</span>
        </div>
      )}

      {/* 统计卡片 */}
      <div className="memory-stats-grid">
        {statsCards.map((s) => (
          <div className="memory-stat-card" key={s.label}>
            <span className="memory-stat-icon">{s.icon}</span>
            <div>
              <div className="memory-stat-value">{s.value}</div>
              <div className="memory-stat-label">{s.label}</div>
            </div>
          </div>
        ))}
      </div>

      {/* 记忆可视化：R 值分布 + 衰减曲线 */}
      {entries.length > 0 && (
        <div className="memory-viz">
          <div className="memory-col-title">记忆可视化 — Ebbinghaus 保留率 (R)</div>
          <div className="memory-viz-grid">
            <div className="memory-viz-card">
              <div className="memory-viz-card-title">R 值分布（当前 {entries.length} 条）</div>
              <div className="memory-viz-histogram">
                {buckets.map((b) => (
                  <div className="memory-viz-bar-col" key={b.range}>
                    <div className="memory-viz-bar-track">
                      <div
                        className="memory-viz-bar-fill"
                        style={{ height: `${(b.count / maxBucket) * 100}%` }}
                        title={`${b.range}: ${b.count} 条`}
                      />
                    </div>
                    <div className="memory-viz-bar-label">{b.range}</div>
                    <div className="memory-viz-bar-count">{b.count}</div>
                  </div>
                ))}
              </div>
            </div>

            <div className="memory-viz-card">
              <div className="memory-viz-card-title">衰减曲线与条目散点（60 天内）</div>
              <svg viewBox={`0 0 ${VIZ_W} ${VIZ_H}`} className="memory-viz-svg">
                {/* Y 轴网格线 */}
                {[0, 0.25, 0.5, 0.75, 1].map((r) => {
                  const y = VIZ_H - VIZ_PAD - r * (VIZ_H - VIZ_PAD * 2);
                  return (
                    <g key={r}>
                      <line
                        x1={VIZ_PAD} x2={VIZ_W - VIZ_PAD} y1={y} y2={y}
                        stroke="#555" strokeWidth="0.5" strokeDasharray="2 3" opacity="0.35"
                      />
                      <text x={VIZ_PAD - 6} y={y + 3} textAnchor="end" fontSize="9" fill="#888">
                        {r.toFixed(2)}
                      </text>
                    </g>
                  );
                })}
                {/* X 轴刻度 */}
                {[0, 15, 30, 45, 60].map((d) => (
                  <text
                    key={d}
                    x={VIZ_PAD + (d / VIZ_MAX_DAYS) * (VIZ_W - VIZ_PAD * 2)}
                    y={VIZ_H - 8}
                    textAnchor="middle"
                    fontSize="9"
                    fill="#888"
                  >
                    {d}d
                  </text>
                ))}
                {/* 参考衰减曲线 */}
                {refCurves.map((c) => (
                  <path
                    key={c.stability}
                    d={decayCurvePath(c.stability, VIZ_W, VIZ_H, VIZ_PAD, VIZ_MAX_DAYS)}
                    stroke={c.color}
                    strokeWidth="1.2"
                    strokeDasharray="4 3"
                    fill="none"
                  />
                ))}
                {/* 图例 */}
                {refCurves.map((c, i) => (
                  <text key={c.stability} x={VIZ_W - VIZ_PAD - 95} y={VIZ_PAD + 4 + i * 12} fontSize="9" fill={c.color}>
                    {c.label}
                  </text>
                ))}
                {/* 条目散点 */}
                {scatter.map(({ e, days }) => {
                  const x = VIZ_PAD + (days / VIZ_MAX_DAYS) * (VIZ_W - VIZ_PAD * 2);
                  const y = VIZ_H - VIZ_PAD - e.retention * (VIZ_H - VIZ_PAD * 2);
                  const color =
                    e.retention < 0.2 ? "#e05a5a" : e.retention < 0.5 ? "#e0a05a" : "#5ab05a";
                  return (
                    <circle key={e.id} cx={x} cy={y} r="3" fill={color} opacity="0.85">
                      <title>{`${e.text}\nR=${(e.retention * 100).toFixed(0)}% · S=${e.stability} · ${days} 天前`}</title>
                    </circle>
                  );
                })}
                {/* 轴标签 */}
                <text x={VIZ_PAD} y={10} fontSize="9" fill="#888">R</text>
                <text x={VIZ_W - VIZ_PAD} y={VIZ_H - 2} fontSize="9" fill="#888" textAnchor="end">距上次召回</text>
              </svg>
            </div>
          </div>

          <div className="memory-viz-list-title">待复习条目（按 R 升序）</div>
          <div className="memory-viz-list">
            {entries.slice(0, 8).map((e) => (
              <div className="memory-viz-item" key={e.id}>
                <div className="memory-viz-item-top">
                  <span className="memory-viz-item-text" title={e.text}>{e.text}</span>
                  <span className="memory-viz-item-r">R={(e.retention * 100).toFixed(0)}%</span>
                </div>
                <div className="memory-viz-item-bar">
                  <div className="memory-viz-item-fill" style={{ width: `${e.retention * 100}%` }} />
                </div>
                <div className="memory-viz-item-meta">
                  <span className="memory-viz-item-tag">{e.category}</span>
                  <span>S={e.stability} · 召回 {e.recall_count} 次</span>
                  <span>{daysSince(e.last_recalled)} 天前</span>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* 记忆搜索 */}
      <div className="memory-search-row">
        <input
          className="memory-search-input"
          placeholder="搜索记忆（BM25 + 语义混合）…"
          value={searchQuery}
          onChange={(e) => setSearchQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && handleSearch()}
        />
        <button className="memory-btn" onClick={handleSearch}>搜索</button>
      </div>
      {searchResults && searchResults.length > 0 && (
        <div className="memory-search-results">
          {searchResults.map((r, i) => (
            <div className="memory-search-item" key={i}>
              <span className="memory-search-loc">{r.file_path}:{r.line_number}</span>
              <span className="memory-search-text">{r.line_content}</span>
              <span className="memory-search-score">{(r.relevance * 100).toFixed(0)}%</span>
            </div>
          ))}
        </div>
      )}

      {/* 主体：左 MEMORY.md 预览 / 右日历 */}
      <div className="memory-body">
        <div className="memory-col memory-md-col">
          <div className="memory-col-title">MEMORY.md 预览</div>
          <pre className="memory-md-preview">
            {loading ? "加载中…" : longTerm || "（暂无长期记忆，与 Agent 对话后会自动沉淀）"}
          </pre>
        </div>

        <div className="memory-col memory-cal-col">
          <div className="memory-col-title">Daily Notes 日历</div>
          <div className="memory-cal-header">
            <button className="memory-cal-nav" onClick={() => { if (calMonth === 0) { setCalMonth(11); setCalYear(calYear - 1); } else setCalMonth(calMonth - 1); }}>‹</button>
            <span>{calYear} 年 {calMonth + 1} 月</span>
            <button className="memory-cal-nav" onClick={() => { if (calMonth === 11) { setCalMonth(0); setCalYear(calYear + 1); } else setCalMonth(calMonth + 1); }}>›</button>
          </div>
          <div className="memory-cal-grid">
            {["日", "一", "二", "三", "四", "五", "六"].map((d) => (
              <div className="memory-cal-dow" key={d}>{d}</div>
            ))}
            {weeks.flat().map((day, i) => {
              if (day === null) return <div className="memory-cal-cell empty" key={i} />;
              const dateStr = `${calYear}-${String(calMonth + 1).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
              const hasNote = noteDates.has(dateStr);
              const isSelected = selectedDate === dateStr;
              const isToday = dateStr === now.toISOString().slice(0, 10);
              return (
                <div
                  className={`memory-cal-cell ${hasNote ? "has-note" : ""} ${isSelected ? "selected" : ""} ${isToday ? "today" : ""}`}
                  key={i}
                  title={hasNote ? `${dateStr} 有笔记` : dateStr}
                  onClick={() => hasNote && openNote(dateStr)}
                >
                  {day}
                  {hasNote && <span className="memory-cal-dot" />}
                </div>
              );
            })}
          </div>
          {selectedDate && (
            <div className="memory-note-view">
              <div className="memory-note-header">
                <span>{selectedDate} 笔记</span>
                <button className="memory-cal-nav" onClick={() => setSelectedDate(null)}>✕</button>
              </div>
              <pre className="memory-note-content">{noteContent || "（当日无内容）"}</pre>
            </div>
          )}
          {!selectedDate && (
            <div className="memory-notes-list">
              {notes.slice(0, 10).map((n) => (
                <div className="memory-notes-item" key={n.date} onClick={() => openNote(n.date)}>
                  <span className="memory-notes-date">{n.date}</span>
                  <span className="memory-notes-preview">{n.preview}</span>
                  <span className="memory-notes-chars">{n.chars} 字</span>
                </div>
              ))}
              {notes.length === 0 && <div className="memory-notes-empty">还没有每日笔记</div>}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
