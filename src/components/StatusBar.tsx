interface StatusBarProps {
  llmConnected: boolean;
  projectPath: string;
  fileCount?: number;
  activeView?: string;
  theme?: string;
  onToggleTheme?: () => void;
  onChatClick: () => void;
  onSettingsClick: () => void;
  onSearchClick?: () => void;
  onExplorerClick?: () => void;
  onCloudClick?: () => void;
}

export default function StatusBar({
  llmConnected,
  projectPath,
  fileCount,
  activeView,
  theme,
  onToggleTheme,
  onChatClick,
  onSettingsClick,
  onSearchClick,
  onExplorerClick,
  onCloudClick,
}: StatusBarProps) {
  const projectName = projectPath
    ? projectPath.split("\\").pop() || projectPath.split("/").pop()
    : "";

  return (
    <div className="status-bar">
      <div className="status-left">
        <div className="status-item" onClick={onExplorerClick} title="Toggle File Explorer (Ctrl+B)">
          <span>📁</span>
          <span>Explorer</span>
        </div>
        {fileCount !== undefined && (
          <div className="status-item" title="Open files">
            <span>📄</span>
            <span>{fileCount} files</span>
          </div>
        )}
      </div>

      <div className="status-center">
        {projectName && (
          <div className="status-item" title={projectPath}>
            <span>📂</span>
            <span>{projectName}</span>
          </div>
        )}
      </div>

      <div className="status-right">
        <div className="status-item" onClick={onSearchClick} title="Search Codebase (Ctrl+Shift+F)">
          <span>🔍</span>
          <span>Search</span>
        </div>

        <div className="status-item" onClick={onCloudClick} title="Cloud Agents">
          <span>☁️</span>
          <span>Cloud</span>
        </div>

        <div className="status-item" onClick={onChatClick} title="Toggle Chat (Ctrl+\\)">
          <span>💬</span>
          <span>Chat</span>
        </div>

        <div className="status-item">
          <span
            className={`status-dot ${llmConnected ? "connected" : "disconnected"}`}
          />
          <span>{llmConnected ? "LLM Connected" : "LLM Offline"}</span>
        </div>

        <div className="status-item" onClick={onSettingsClick} title="Settings">
          <span>⚙️</span>
        </div>

        <button
          className="theme-toggle-btn"
          onClick={onToggleTheme}
          title={theme === "light" ? "Switch to Dark" : "Switch to Light"}
        >
          {theme === "light" ? "☀️" : "🌙"}
        </button>
      </div>
    </div>
  );
}
