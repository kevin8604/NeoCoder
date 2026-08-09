import React, { type ReactNode } from "react";
import { FileText, Zap, Wrench, FolderTree, Folder } from "lucide-react";

export interface MentionItem {
  id: string;
  label: string;
  type: "file" | "symbol" | "command" | "codebase" | "folder";
  description?: string;
  path?: string;
  /** Optional line number for `@file:line` references */
  line?: number;
}

interface MentionMenuProps {
  visible: boolean;
  items: MentionItem[];
  selectedIndex: number;
  position: { top: number; left: number };
  onSelect: (item: MentionItem) => void;
}

const typeIcons: Record<string, ReactNode> = {
  file: <FileText size={13} />,
  symbol: <Zap size={13} />,
  command: <Wrench size={13} />,
  codebase: <FolderTree size={13} />,
  folder: <Folder size={13} />,
};

const typeLabels: Record<string, string> = {
  file: "File",
  symbol: "Symbol",
  command: "Command",
  codebase: "Codebase",
  folder: "Folder",
};

export default function MentionMenu({ visible, items, selectedIndex, position, onSelect }: MentionMenuProps) {
  if (!visible || items.length === 0) return null;

  return (
    <div
      className="mention-menu"
      style={{ bottom: position.top, left: position.left }}
    >
      {items.map((item, idx) => (
        <div
          key={item.id}
          className={`mention-item ${idx === selectedIndex ? "selected" : ""}`}
          onMouseDown={(e) => {
            e.preventDefault();
            onSelect(item);
          }}
          onMouseEnter={() => {
            // visual only via CSS :hover
          }}
        >
          <span className="mention-icon">{typeIcons[item.type] || <FileText size={13} />}</span>
          <div className="mention-info">
            <span className="mention-label">{item.label}</span>
            {item.description && (
              <span className="mention-desc">{item.description}</span>
            )}
          </div>
          <span className="mention-type-tag">{typeLabels[item.type]}</span>
        </div>
      ))}
    </div>
  );
}
