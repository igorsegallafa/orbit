import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FolderIcon, GitIcon, SearchIcon } from "./Icons";
import { FileTypeIcon, FolderTreeIcon } from "./FileIcons";
import { Workspace } from "../types/config";

interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
}

type DockView = "files" | "git" | "search";

interface Props {
  workspace: Workspace;
  initialRepo?: string;
  onOpenFile: (repo: string, path: string) => void;
  onError: (msg: string) => void;
}

/**
 * Fixed right dock (Orca/VS Code style): activity bar on the far edge with
 * Files/Git/Search, and a compact file panel for the active workspace's
 * selected repo. Always visible while a workspace is open.
 */
export function FileTreePanel({ workspace, initialRepo, onOpenFile, onError }: Props) {
  const [view, setView] = useState<DockView>("files");
  const [repo, setRepo] = useState(initialRepo ?? workspace.repos[0] ?? "");
  const [tree, setTree] = useState<Record<string, FileNode[]>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const loadDir = async (dir: string) => {
    try {
      const nodes = await invoke<FileNode[]>("list_files", {
        workspace: workspace.name,
        repo,
        path: dir,
      });
      setTree((t) => ({ ...t, [dir]: nodes }));
    } catch (e) {
      onError(String(e));
    }
  };

  // Reload when workspace or repo changes
  useEffect(() => {
    if (!repo) return;
    setTree({});
    setExpanded({ "": true });
    loadDir("");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace.name, repo]);

  const toggleDir = (dir: string) => {
    setExpanded((x) => {
      const next = { ...x, [dir]: !x[dir] };
      if (next[dir]) loadDir(dir);
      return next;
    });
  };

  const renderDir = (dir: string, depth: number): React.ReactNode => {
    const children = tree[dir] ?? [];
    return (
      <>
        {children
          .filter((n) => n.is_dir)
          .map((n) => (
            <div key={n.path}>
              <button className={`tree-item depth-${Math.min(depth, 6)}`} onClick={() => toggleDir(n.path)}>
                <span className="tree-chevron">{expanded[n.path] ? "▾" : "▸"}</span>
                <FolderTreeIcon open={!!expanded[n.path]} />
                <span className="tree-name">{n.name}</span>
              </button>
              {expanded[n.path] && renderDir(n.path, depth + 1)}
            </div>
          ))}
        {children
          .filter((n) => !n.is_dir)
          .map((n) => (
            <button
              key={n.path}
              className={`tree-item tree-file depth-${Math.min(depth, 6)}`}
              onClick={() => onOpenFile(repo, n.path)}
            >
              <span className="tree-chevron invisible" />
              <FileTypeIcon name={n.name} />
              <span className="tree-name">{n.name}</span>
            </button>
          ))}
      </>
    );
  };

  const dockItems: { id: DockView; label: string; icon: React.ReactNode }[] = [
    { id: "files", label: "Files", icon: <FolderIcon /> },
    { id: "search", label: "Find", icon: <SearchIcon /> },
    { id: "git", label: "Git", icon: <GitIcon /> },
  ];

  return (
    <div className="dock-panel">
      {/* Horizontal icon toolbar above the FILES header, Orca style */}
      <div className="dock-icons">
        {dockItems.map((item) => (
          <button
            key={item.id}
            className={`dock-icon ${view === item.id ? "dock-icon-active" : ""}`}
            title={item.label}
            onClick={() => setView(item.id)}
          >
            {item.icon}
          </button>
        ))}
      </div>
      <div className="dock-panel-header">
        {dockItems.find((i) => i.id === view)?.label}
      </div>
      {view === "files" && (
        <>
          <div className="dock-toolbar">
            <select
              className="dock-select"
              value={repo}
              onChange={(e) => setRepo(e.target.value)}
            >
              {workspace.repos.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </div>
          <div className="dock-panel-body">{renderDir("", 0)}</div>
        </>
      )}
      {view === "git" && <div className="dock-placeholder">Git status coming soon</div>}
      {view === "search" && <div className="dock-placeholder">Search coming soon</div>}
    </div>
  );
}