import { useState, useCallback, useRef, useEffect } from "react";
import { searchCodebase, reindexProject, type SearchResult } from "../hooks/useTauri";

interface SearchPanelProps {
  projectPath: string;
  onFileSelect: (path: string, line?: number) => void;
}

export default function SearchPanel({ projectPath, onFileSelect }: SearchPanelProps) {
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [searching, setSearching] = useState(false);
  const [reindexing, setReindexing] = useState(false);
  const [statusMessage, setStatusMessage] = useState("");
  const [hasSearched, setHasSearched] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    // Auto-focus input on mount
    setTimeout(() => inputRef.current?.focus(), 100);
  }, []);

  const handleSearch = useCallback(async () => {
    const q = query.trim();
    if (!q) return;

    setSearching(true);
    setStatusMessage("");
    try {
      const res = await searchCodebase(q, 20);
      setResults(res || []);
      setHasSearched(true);
      if (res?.length === 0) {
        setStatusMessage("No results found. Try reindexing the project.");
      }
    } catch {
      setStatusMessage("Search failed. Is the backend running?");
    }
    setSearching(false);
  }, [query]);

  const handleReindex = useCallback(async () => {
    setReindexing(true);
    setStatusMessage("Reindexing project...");
    try {
      const msg = await reindexProject(projectPath);
      setStatusMessage(msg || "Reindex complete!");
    } catch {
      setStatusMessage("Reindex failed.");
    }
    setReindexing(false);
  }, [projectPath]);

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      handleSearch();
    }
  };

  const handleResultClick = (result: SearchResult) => {
    onFileSelect(result.chunk.file_path, result.chunk.start_line);
  };

  const scorePercent = (score: number) => Math.round(score * 100);

  return (
    <div className="search-panel">
      <div className="search-header">
        <h3>Search Codebase</h3>
        <button
          className="search-reindex-btn"
          onClick={handleReindex}
          disabled={reindexing}
          title="Reindex project"
        >
          {reindexing ? "⟳" : "↻"}
        </button>
      </div>

      <div className="search-input-area">
        <input
          ref={inputRef}
          className="search-input"
          type="text"
          placeholder="Search codebase... (e.g. function name, error pattern)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={handleKeyDown}
        />
        <button
          className="search-submit-btn"
          onClick={handleSearch}
          disabled={!query.trim() || searching}
        >
          {searching ? "..." : "Search"}
        </button>
      </div>

      {statusMessage && (
        <div className="search-status">{statusMessage}</div>
      )}

      <div className="search-results">
        {results.map((result, i) => (
          <div
            key={`${result.chunk.file_path}-${result.chunk.start_line}-${i}`}
            className="search-result-item"
            onClick={() => handleResultClick(result)}
          >
            <div className="search-result-header">
              <span className="search-result-file" title={result.chunk.file_path}>
                📄 {result.chunk.file_path.split("\\").pop() || result.chunk.file_path.split("/").pop()}
              </span>
              <span className={`search-result-score ${result.score > 0.5 ? "high" : "low"}`}>
                {scorePercent(result.score)}%
              </span>
            </div>
            {result.chunk.summary && (
              <div className="search-result-summary">{result.chunk.summary}</div>
            )}
            <div className="search-result-snippet">
              <code>{result.chunk.content.substring(0, 200)}</code>
            </div>
            <div className="search-result-meta">
              <span>Line {result.chunk.start_line}</span>
              <span className="search-result-type">{result.chunk.chunk_type}</span>
              <span>{result.chunk.language}</span>
            </div>
          </div>
        ))}
        {hasSearched && results.length === 0 && !searching && (
          <div className="search-empty">
            <p>No results for "{query}"</p>
          </div>
        )}
      </div>
    </div>
  );
}
