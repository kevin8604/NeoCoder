import { useEffect, useRef, useState, useCallback } from "react";
import { BarChart3, RefreshCw, Loader2, AlertTriangle } from "lucide-react";
import mermaid from "mermaid";

interface DependencyGraphProps {
  projectPath: string;
  onFileSelect?: (path: string) => void;
}

interface GraphData {
  mermaid: string;
  node_count: number;
  edge_count: number;
}

// Initialize mermaid with dark theme
mermaid.initialize({
  startOnLoad: false,
  theme: "dark",
  securityLevel: "loose",
  flowchart: {
    useMaxWidth: true,
    htmlLabels: true,
    curve: "basis",
  },
});

export default function DependencyGraph({ projectPath, onFileSelect }: DependencyGraphProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  const [graphData, setGraphData] = useState<GraphData | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [depth, setDepth] = useState(3);
  const [svgContent, setSvgContent] = useState<string>("");

  const loadGraph = useCallback(async () => {
    if (!projectPath) return;
    setLoading(true);
    setError(null);

    try {
      const { invoke } = await import("@tauri-apps/api/core");
      const result = await invoke<GraphData>("get_dependency_graph", {
        projectPath,
        depth,
      });
      setGraphData(result);

      // Render mermaid diagram
      if (result.mermaid) {
        const { svg } = await mermaid.render("dep-graph", result.mermaid);
        setSvgContent(svg);
      }
    } catch (e: any) {
      setError(typeof e === "string" ? e : e?.message || "Failed to load dependency graph");
    } finally {
      setLoading(false);
    }
  }, [projectPath, depth]);

  useEffect(() => {
    loadGraph();
  }, [loadGraph]);

  // Handle node click to navigate to file
  const handleSvgClick = useCallback(
    (e: React.MouseEvent<HTMLDivElement>) => {
      const target = e.target as HTMLElement;
      // Mermaid nodes have data-id attributes or are within .node elements
      const node = target.closest(".node");
      if (node) {
        const label = node.querySelector(".nodeLabel")?.textContent;
        if (label && onFileSelect) {
          // Try to find the full path from the graph data
          // The label is just the filename, we need to match it
          onFileSelect(label);
        }
      }
    },
    [onFileSelect]
  );

  return (
    <div className="dependency-graph-panel" style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      {/* Header */}
      <div style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        padding: "8px 12px",
        borderBottom: "1px solid var(--border, #313244)",
      }}>
        <span style={{ fontWeight: 600, fontSize: 13, display: "inline-flex", alignItems: "center", gap: 6 }}><BarChart3 size={14} /> Dependency Graph</span>
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <label style={{ fontSize: 11, color: "var(--text-muted, #a6adc8)" }}>
            Depth:
            <select
              value={depth}
              onChange={(e) => setDepth(Number(e.target.value))}
              style={{
                marginLeft: 4,
                background: "var(--surface-1, #313244)",
                color: "var(--text, #cdd6f4)",
                border: "1px solid var(--border, #45475a)",
                borderRadius: 4,
                padding: "2px 4px",
                fontSize: 11,
              }}
            >
              <option value={2}>2</option>
              <option value={3}>3</option>
              <option value={5}>5</option>
              <option value={8}>8</option>
            </select>
          </label>
          <button
            onClick={loadGraph}
            disabled={loading}
            style={{
              padding: "4px 8px",
              fontSize: 11,
              background: "var(--surface-1, #313244)",
              color: "var(--text, #cdd6f4)",
              border: "1px solid var(--border, #45475a)",
              borderRadius: 4,
              cursor: loading ? "wait" : "pointer",
            }}
          >
            {loading ? <Loader2 size={12} className="spin" /> : <><RefreshCw size={12} /> Refresh</>}
          </button>
        </div>
      </div>

      {/* Stats */}
      {graphData && (
        <div style={{
          padding: "4px 12px",
          fontSize: 11,
          color: "var(--text-muted, #a6adc8)",
          borderBottom: "1px solid var(--border, #313244)",
        }}>
          {graphData.node_count} modules · {graphData.edge_count} dependencies
        </div>
      )}

      {/* Graph container */}
      <div
        ref={containerRef}
        onClick={handleSvgClick}
        style={{
          flex: 1,
          overflow: "auto",
          padding: 16,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        {loading && <div style={{ color: "var(--text-muted)" }}>Loading graph...</div>}
        {error && (
          <div style={{ color: "var(--error, #f38ba8)", fontSize: 13, textAlign: "center" }}>
            <p><AlertTriangle size={14} style={{ verticalAlign: -2 }} /> {error}</p>
            <button
              onClick={loadGraph}
              style={{
                marginTop: 8,
                padding: "4px 12px",
                background: "var(--surface-1, #313244)",
                color: "var(--text, #cdd6f4)",
                border: "1px solid var(--border, #45475a)",
                borderRadius: 4,
                cursor: "pointer",
              }}
            >
              Retry
            </button>
          </div>
        )}
        {!loading && !error && svgContent && (
          <div
            className="mermaid-svg-container"
            dangerouslySetInnerHTML={{ __html: svgContent }}
            style={{ maxWidth: "100%", overflow: "auto" }}
          />
        )}
        {!loading && !error && !svgContent && (
          <div style={{ color: "var(--text-muted)", fontSize: 13 }}>
            No dependency graph available. Make sure a project is open.
          </div>
        )}
      </div>
    </div>
  );
}
