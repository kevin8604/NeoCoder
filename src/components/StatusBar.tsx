import { Folder, FileText, FolderOpen, Search, Cloud, BarChart3, Brain, TrendingUp, MessageSquare, Settings, Sun, Moon, History } from "lucide-react";

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
  onGraphClick?: () => void;
  onMemoryClick?: () => void;
  onInsightsClick?: () => void;
  onCheckpointsClick?: () => void;
  /** null = local models disabled; true/false = Ollama health probe result */
  localModelRunning?: boolean | null;
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
  onGraphClick,
  onMemoryClick,
  onInsightsClick,
  onCheckpointsClick,
  localModelRunning,
}: StatusBarProps) {
  const projectName = projectPath
    ? projectPath.split("\\").pop() || projectPath.split("/").pop()
    : "";

  return (
    <div className="status-bar">
      <div className="status-left">
        <div className="status-item" onClick={onExplorerClick} title="Toggle File Explorer (Ctrl+B)">
          <Folder size={13} />
          <span>Explorer</span>
        </div>
        {fileCount !== undefined && (
          <div className="status-item" title="Open files">
            <FileText size={13} />
            <span>{fileCount} files</span>
          </div>
        )}
      </div>

      <div className="status-center">
        {projectName && (
          <div className="status-item" title={projectPath}>
            <FolderOpen size={13} />
            <span>{projectName}</span>
          </div>
        )}
      </div>

      <div className="status-right">
        <div className="status-item" onClick={onSearchClick} title="Search Codebase (Ctrl+Shift+F)">
          <Search size={13} />
          <span>Search</span>
        </div>

        <div className="status-item" onClick={onCloudClick} title="Cloud Agents">
          <Cloud size={13} />
          <span>Cloud</span>
        </div>

        <div className="status-item" onClick={onGraphClick} title="Dependency Graph">
          <BarChart3 size={13} />
          <span>Graph</span>
        </div>

        <div className="status-item" onClick={onCheckpointsClick} title="Checkpoints (iteration snapshots & diff)">
          <History size={13} />
          <span>Checkpoints</span>
        </div>

        <div className="status-item" onClick={onMemoryClick} title="Memory Panel">
          <Brain size={13} />
          <span>Memory</span>
        </div>

        <div className="status-item" onClick={onInsightsClick} title="Insights (Telemetry & Agent Logs)">
          <TrendingUp size={13} />
          <span>Insights</span>
        </div>

        <div className="status-item" onClick={onChatClick} title="Toggle Chat (Ctrl+\\)">
          <MessageSquare size={13} />
          <span>Chat</span>
        </div>

        <div className="status-item">
          <span
            className={`status-dot ${llmConnected ? "connected" : "disconnected"}`}
          />
          <span>{llmConnected ? "LLM Connected" : "LLM Offline"}</span>
        </div>

        {localModelRunning !== undefined && localModelRunning !== null && (
          <div className="status-item" title="Local model (Ollama) status — auto-degrades to remote when offline">
            <span
              className={`status-dot ${localModelRunning ? "connected" : "disconnected"}`}
            />
            <span>{localModelRunning ? "Ollama" : "Ollama Offline"}</span>
          </div>
        )}

        <div className="status-item" onClick={onSettingsClick} title="Settings">
          <Settings size={13} />
        </div>

        <button
          className="theme-toggle-btn"
          onClick={onToggleTheme}
          title={theme === "light" ? "Switch to Dark" : "Switch to Light"}
        >
          {theme === "light" ? <Sun size={14} /> : <Moon size={14} />}
        </button>
      </div>
    </div>
  );
}
