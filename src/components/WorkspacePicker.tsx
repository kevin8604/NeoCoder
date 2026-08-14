import { useState } from "react";
import { FolderOpen, Check, Pencil, Trash2, Plus } from "lucide-react";
import type { Workspace } from "../hooks/useTauri";

interface WorkspacePickerProps {
  workspaces: Workspace[];
  active: Workspace | null;
  onSelect: (ws: Workspace) => void;
  onOpenProject: () => void;
  onRename: (id: string, name: string) => void;
  onRemove: (id: string) => void;
  onClose: () => void;
}

/**
 * Dropdown picker for switching between registered workspaces.
 * Each workspace owns an independent index DB / watcher / project skills;
 * selecting one activates it (see `activate_workspace` on the backend).
 */
export default function WorkspacePicker({
  workspaces,
  active,
  onSelect,
  onOpenProject,
  onRename,
  onRemove,
  onClose,
}: WorkspacePickerProps) {
  const [renamingId, setRenamingId] = useState<string | null>(null);
  const [renameValue, setRenameValue] = useState("");

  const startRename = (ws: Workspace) => {
    setRenamingId(ws.id);
    setRenameValue(ws.name);
  };

  const commitRename = (id: string) => {
    const name = renameValue.trim();
    if (name) onRename(id, name);
    setRenamingId(null);
  };

  return (
    <>
      {/* Click-away overlay */}
      <div className="workspace-picker-overlay" onClick={onClose} />

      <div className="workspace-picker">
        <div className="workspace-picker-header">
          <FolderOpen size={13} />
          <span>Workspaces</span>
          <span className="workspace-picker-count">{workspaces.length}</span>
        </div>

        <div className="workspace-picker-list">
          {workspaces.length === 0 && (
            <div className="workspace-picker-empty">
              No workspaces yet — open a project to get started
            </div>
          )}

          {workspaces.map((ws) => {
            const isActive = active?.id === ws.id;
            return (
              <div
                key={ws.id}
                className={`workspace-picker-item${isActive ? " active" : ""}`}
                title={ws.path}
                onClick={() => !isActive && onSelect(ws)}
              >
                <span className="workspace-picker-name">
                  {isActive && <Check size={12} />}
                  {renamingId === ws.id ? (
                    <input
                      className="workspace-picker-input"
                      value={renameValue}
                      autoFocus
                      onChange={(e) => setRenameValue(e.target.value)}
                      onClick={(e) => e.stopPropagation()}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitRename(ws.id);
                        if (e.key === "Escape") setRenamingId(null);
                      }}
                      onBlur={() => commitRename(ws.id)}
                    />
                  ) : (
                    <span>{ws.name}</span>
                  )}
                </span>
                <span className="workspace-picker-path">{ws.path}</span>
                <span className="workspace-picker-actions">
                  {!renamingId && (
                    <button
                      className="workspace-picker-btn"
                      title="Rename"
                      onClick={(e) => {
                        e.stopPropagation();
                        startRename(ws);
                      }}
                    >
                      <Pencil size={12} />
                    </button>
                  )}
                  {!renamingId && !isActive && (
                    <button
                      className="workspace-picker-btn danger"
                      title="Remove"
                      onClick={(e) => {
                        e.stopPropagation();
                        onRemove(ws.id);
                      }}
                    >
                      <Trash2 size={12} />
                    </button>
                  )}
                </span>
              </div>
            );
          })}
        </div>

        <div className="workspace-picker-footer">
          <button className="workspace-picker-open" onClick={onOpenProject}>
            <Plus size={13} />
            <span>Open New Project…</span>
          </button>
        </div>
      </div>
    </>
  );
}
