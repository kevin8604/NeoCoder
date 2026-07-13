import { useState, useEffect, useCallback, useRef } from "react";
import {
  getFileTree,
  createFile,
  createDirectory,
  deleteFileOrDir,
  renameFileOrDir,
  type FileTreeItem,
} from "../hooks/useTauri";

interface FileExplorerProps {
  projectPath: string;
  onFileSelect: (path: string) => void;
  onClose: () => void;
}

interface ContextMenuState {
  x: number;
  y: number;
  path: string;
  isDir: boolean;
}

interface TreeNodeProps {
  item: FileTreeItem;
  depth: number;
  onFileSelect: (path: string) => void;
  onContextMenu: (e: React.MouseEvent, path: string, isDir: boolean) => void;
  onRename: (path: string, isDir: boolean) => void;
  refreshKey: number;
}

function TreeNode({ item, depth, onFileSelect, onContextMenu, onRename, refreshKey }: TreeNodeProps) {
  const [expanded, setExpanded] = useState(false);
  const [children, setChildren] = useState<FileTreeItem[] | null>(null);
  const [loading, setLoading] = useState(false);

  // Reload children when refreshKey changes
  useEffect(() => {
    if (expanded) {
      setLoading(true);
      getFileTree(item.path, 1).then((result) => {
        setChildren(result);
        setLoading(false);
      });
    }
  }, [refreshKey]);

  const handleToggle = useCallback(async () => {
    if (!item.is_dir) {
      onFileSelect(item.path);
      return;
    }

    if (expanded) {
      setExpanded(false);
      return;
    }

    if (!children) {
      setLoading(true);
      const result = await getFileTree(item.path, 1);
      setChildren(result);
      setLoading(false);
    }
    setExpanded(true);
  }, [item, expanded, children, onFileSelect]);

  const handleContextMenu = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      onContextMenu(e, item.path, item.is_dir);
    },
    [item.path, item.is_dir, onContextMenu]
  );

  const handleDoubleClick = useCallback(
    (e: React.MouseEvent) => {
      if (item.is_dir) {
        handleToggle();
      } else {
        onRename(item.path, item.is_dir);
      }
    },
    [item, handleToggle, onRename]
  );

  const icon = item.is_dir
    ? expanded
      ? "📂"
      : "📁"
    : getFileIcon(item.name);

  return (
    <div>
      <div
        className="file-tree-node"
        style={{ paddingLeft: depth * 16 + 8 }}
        onClick={handleToggle}
        onContextMenu={handleContextMenu}
        onDoubleClick={handleDoubleClick}
        title={`${item.path}\nRight-click for actions · Double-click to rename`}
      >
        <span className="file-tree-icon">{icon}</span>
        <span className="file-tree-name">{item.name}</span>
        {loading && <span className="file-tree-loading">⋯</span>}
      </div>
      {expanded && children && (
        <div className="file-tree-children">
          {children.map((child) => (
            <TreeNode
              key={child.path}
              item={child}
              depth={depth + 1}
              onFileSelect={onFileSelect}
              onContextMenu={onContextMenu}
              onRename={onRename}
              refreshKey={refreshKey}
            />
          ))}
          {children.length === 0 && (
            <div
              className="file-tree-node file-tree-empty"
              style={{ paddingLeft: (depth + 1) * 16 + 8 }}
            >
              (empty)
            </div>
          )}
        </div>
      )}
    </div>
  );
}

function getFileIcon(name: string): string {
  const ext = name.split(".").pop()?.toLowerCase();
  switch (ext) {
    case "rs": return "🦀";
    case "ts":
    case "tsx": return "🔷";
    case "js":
    case "jsx": return "🟨";
    case "py": return "🐍";
    case "go": return "🔵";
    case "java": return "☕";
    case "css":
    case "scss": return "🎨";
    case "json": return "📋";
    case "toml": return "⚙️";
    case "md": return "📝";
    case "yml":
    case "yaml": return "📐";
    case "html": return "🌐";
    case "sql": return "🗃️";
    default: return "📄";
  }
}

export default function FileExplorer({ projectPath, onFileSelect, onClose }: FileExplorerProps) {
  const [rootItems, setRootItems] = useState<FileTreeItem[]>([]);
  const [loading, setLoading] = useState(true);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [renameTarget, setRenameTarget] = useState<{ path: string; isDir: boolean } | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [newItemPrompt, setNewItemPrompt] = useState<{ parentPath: string; type: "file" | "folder" } | null>(null);
  const [newItemName, setNewItemName] = useState("");
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [refreshKey, setRefreshKey] = useState(0);
  const renameInputRef = useRef<HTMLInputElement>(null);
  const newItemInputRef = useRef<HTMLInputElement>(null);

  const reloadTree = useCallback(() => {
    setRefreshKey((k) => k + 1);
    setLoading(true);
    getFileTree(projectPath, 1).then((items) => {
      setRootItems(items);
      setLoading(false);
    });
  }, [projectPath]);

  useEffect(() => {
    reloadTree();
  }, [projectPath]);

  // Close context menu on outside click
  useEffect(() => {
    if (!contextMenu) return;
    const handler = () => setContextMenu(null);
    window.addEventListener("click", handler);
    return () => window.removeEventListener("click", handler);
  }, [contextMenu]);

  // Focus input on rename/new-item prompt
  useEffect(() => {
    if (renameTarget) {
      const name = renameTarget.path.split("\\").pop() || renameTarget.path.split("/").pop() || "";
      setRenameValue(renameTarget.isDir ? name : name);
      setTimeout(() => renameInputRef.current?.focus(), 10);
    }
  }, [renameTarget]);

  useEffect(() => {
    if (newItemPrompt) {
      setNewItemName("");
      setTimeout(() => newItemInputRef.current?.focus(), 10);
    }
  }, [newItemPrompt]);

  const handleContextMenu = useCallback(
    (e: React.MouseEvent, path: string, isDir: boolean) => {
      setContextMenu({ x: e.clientX, y: e.clientY, path, isDir });
    },
    []
  );

  const handleNewFile = useCallback(() => {
    const parentPath = contextMenu?.isDir ? contextMenu.path : contextMenu?.path.split("\\").slice(0, -1).join("\\") || projectPath;
    setContextMenu(null);
    setNewItemPrompt({ parentPath: parentPath || projectPath, type: "file" });
  }, [contextMenu, projectPath]);

  const handleNewFolder = useCallback(() => {
    const parentPath = contextMenu?.isDir ? contextMenu.path : contextMenu?.path.split("\\").slice(0, -1).join("\\") || projectPath;
    setContextMenu(null);
    setNewItemPrompt({ parentPath: parentPath || projectPath, type: "folder" });
  }, [contextMenu, projectPath]);

  const handleRenameStart = useCallback(
    (path: string, isDir: boolean) => {
      setRenameTarget({ path, isDir });
    },
    []
  );

  const handleRenameConfirm = useCallback(async () => {
    if (!renameTarget || !renameValue.trim()) {
      setRenameTarget(null);
      return;
    }
    const parentDir = renameTarget.path.split("\\").slice(0, -1).join("\\") + "\\";
    const newPath = parentDir + renameValue.trim();
    if (newPath === renameTarget.path) {
      setRenameTarget(null);
      return;
    }
    const ok = await renameFileOrDir(renameTarget.path, newPath);
    setRenameTarget(null);
    if (ok) {
      reloadTree();
      setErrorMsg(null);
    } else {
      setErrorMsg(`Failed to rename "${renameTarget.path.split("\\").pop()}"`);
    }
  }, [renameTarget, renameValue, reloadTree]);

  const handleDelete = useCallback(async () => {
    if (!contextMenu) return;
    const name = contextMenu.path.split("\\").pop() || "";
    const type = contextMenu.isDir ? "folder" : "file";
    if (!window.confirm(`Delete ${type} "${name}"? This cannot be undone.`)) {
      setContextMenu(null);
      return;
    }
    const ok = await deleteFileOrDir(contextMenu.path);
    setContextMenu(null);
    if (ok) {
      reloadTree();
      setErrorMsg(null);
    } else {
      setErrorMsg(`Failed to delete "${name}"`);
    }
  }, [contextMenu, reloadTree]);

  const handleNewItemConfirm = useCallback(async () => {
    if (!newItemPrompt || !newItemName.trim()) {
      setNewItemPrompt(null);
      return;
    }
    const fullPath = newItemPrompt.parentPath + "\\" + newItemName.trim();
    let ok: boolean;
    if (newItemPrompt.type === "file") {
      ok = await createFile(fullPath);
    } else {
      ok = await createDirectory(fullPath);
    }
    setNewItemPrompt(null);
    if (ok) {
      reloadTree();
      setErrorMsg(null);
    } else {
      setErrorMsg(`Failed to create "${newItemName.trim()}"`);
    }
  }, [newItemPrompt, newItemName, reloadTree]);

  return (
    <div className="file-explorer">
      <div className="file-explorer-header">
        <span className="file-explorer-title">Explorer</span>
        <span className="file-explorer-project">
          {projectPath.split("\\").pop() || projectPath.split("/").pop()}
        </span>
        <div className="file-explorer-actions">
          <button
            className="file-explorer-action-btn"
            onClick={() => setNewItemPrompt({ parentPath: projectPath, type: "file" })}
            title="New File"
          >
            📄+
          </button>
          <button
            className="file-explorer-action-btn"
            onClick={() => setNewItemPrompt({ parentPath: projectPath, type: "folder" })}
            title="New Folder"
          >
            📁+
          </button>
          <button className="file-explorer-action-btn" onClick={reloadTree} title="Refresh">
            ↻
          </button>
          <button className="file-explorer-close" onClick={onClose}>
            ✕
          </button>
        </div>
      </div>

      {errorMsg && (
        <div className="file-explorer-error">
          <span>{errorMsg}</span>
          <button onClick={() => setErrorMsg(null)}>✕</button>
        </div>
      )}

      <div className="file-explorer-tree">
        {loading ? (
          <div className="file-tree-loading-text">Loading...</div>
        ) : (
          rootItems.map((item) => (
            <TreeNode
              key={item.path}
              item={item}
              depth={0}
              onFileSelect={onFileSelect}
              onContextMenu={handleContextMenu}
              onRename={handleRenameStart}
              refreshKey={refreshKey}
            />
          ))
        )}
      </div>

      {/* Context Menu */}
      {contextMenu && (
        <div
          className="explorer-context-menu"
          style={{ left: contextMenu.x, top: contextMenu.y }}
          onClick={(e) => e.stopPropagation()}
        >
          <div className="context-menu-item" onClick={handleNewFile}>
            <span className="context-menu-icon">📄</span>
            <span className="context-menu-label">New File</span>
          </div>
          <div className="context-menu-item" onClick={handleNewFolder}>
            <span className="context-menu-icon">📁</span>
            <span className="context-menu-label">New Folder</span>
          </div>
          <div className="context-menu-separator" />
          <div
            className="context-menu-item"
            onClick={() => {
              setContextMenu(null);
              handleRenameStart(contextMenu.path, contextMenu.isDir);
            }}
          >
            <span className="context-menu-icon">✏️</span>
            <span className="context-menu-label">Rename</span>
            <span className="context-menu-shortcut">F2</span>
          </div>
          <div className="context-menu-item danger" onClick={handleDelete}>
            <span className="context-menu-icon">🗑️</span>
            <span className="context-menu-label">Delete</span>
            <span className="context-menu-shortcut">Del</span>
          </div>
        </div>
      )}

      {/* Rename Modal */}
      {renameTarget && (
        <div className="explorer-modal-overlay" onClick={() => setRenameTarget(null)}>
          <div className="explorer-modal" onClick={(e) => e.stopPropagation()}>
            <div className="explorer-modal-header">
              Rename {renameTarget.isDir ? "Folder" : "File"}
            </div>
            <input
              ref={renameInputRef}
              className="settings-input"
              value={renameValue}
              onChange={(e) => setRenameValue(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleRenameConfirm();
                if (e.key === "Escape") setRenameTarget(null);
              }}
            />
            <div className="explorer-modal-footer">
              <button className="ask-question-cancel" onClick={() => setRenameTarget(null)}>Cancel</button>
              <button className="ask-question-submit" onClick={handleRenameConfirm}>Rename</button>
            </div>
          </div>
        </div>
      )}

      {/* New Item Modal */}
      {newItemPrompt && (
        <div className="explorer-modal-overlay" onClick={() => setNewItemPrompt(null)}>
          <div className="explorer-modal" onClick={(e) => e.stopPropagation()}>
            <div className="explorer-modal-header">
              New {newItemPrompt.type === "file" ? "File" : "Folder"}
            </div>
            <div className="explorer-modal-path">
              {newItemPrompt.parentPath}
            </div>
            <input
              ref={newItemInputRef}
              className="settings-input"
              placeholder={newItemPrompt.type === "file" ? "filename.ts" : "new-folder"}
              value={newItemName}
              onChange={(e) => setNewItemName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") handleNewItemConfirm();
                if (e.key === "Escape") setNewItemPrompt(null);
              }}
            />
            <div className="explorer-modal-footer">
              <button className="ask-question-cancel" onClick={() => setNewItemPrompt(null)}>Cancel</button>
              <button className="ask-question-submit" onClick={handleNewItemConfirm}>Create</button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
