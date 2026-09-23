import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderIcon, GitIcon, SearchIcon } from "./Icons";
import { FileTypeIcon, FolderTreeIcon } from "./FileIcons";
import { ContextMenu, MenuItem, useContextMenu } from "./ContextMenu";
import { GitPanel } from "./GitPanel";
import { GitCommit, Workspace } from "../types/config";

interface FileNode {
  name: string;
  path: string;
  is_dir: boolean;
}

type DockView = "files" | "git";

interface TreeTarget {
  repo: string;
  path: string;
  isDir: boolean;
}

interface Props {
  workspace: Workspace;
  onOpenFile: (repo: string, path: string) => void;
  /** Opens the diff review for a changed file (Git tab). */
  onReviewFile: (repo: string, path: string) => void;
  /** Opens commit inspection (Git tab). */
  onReviewCommit: (repo: string, commit: GitCommit) => void;
  /** Opens the Find in Files popup. */
  onOpenFind: () => void;
  onError: (msg: string) => void;
}

/**
 * Fixed right dock (Orca/VS Code style): Files/Git/Search icon toolbar,
 * compact file tree for the workspace's selected repo.
 *
 * Drag & drop is implemented with raw mouse events because WKWebView
 * (Tauri's webview) does not fire HTML5 drag events for in-page drags —
 * `draggable`/onDragStart simply never trigger there. We track mousedown →
 * move past a 4px threshold → hit-test folders with elementFromPoint →
 * move on mouseup, with a floating ghost and target highlight.
 */
export function FileTreePanel({ workspace, onOpenFile, onReviewFile, onReviewCommit, onOpenFind, onError }: Props) {
  const [view, setView] = useState<DockView>("files");
  // The picked repo only counts while it belongs to the focused workspace:
  // switching focus must never pair the new workspace with the old repo.
  const [picked, setRepo] = useState(workspace.repos[0] ?? "");
  const repo = workspace.repos.includes(picked) ? picked : (workspace.repos[0] ?? "");
  const [tree, setTree] = useState<Record<string, FileNode[]>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [renameTarget, setRenameTarget] = useState<TreeTarget | null>(null);
  const [renameValue, setRenameValue] = useState("");
  const [createPrompt, setCreatePrompt] = useState<{ dir: string; isDir: boolean } | null>(null);
  const [createValue, setCreateValue] = useState("");
  const menu = useContextMenu<TreeTarget>();

  // --- manual drag state ---
  const dragRef = useRef<{ startX: number; startY: number; src: TreeTarget | null; active: boolean }>({
    startX: 0,
    startY: 0,
    src: null,
    active: false,
  });
  const justDraggedRef = useRef(false);
  const [dragging, setDragging] = useState<TreeTarget | null>(null);
  const [dragHover, setDragHover] = useState<string | null>(null);
  const [dragOverTerminal, setDragOverTerminal] = useState(false);
  const [ghostPos, setGhostPos] = useState<{ x: number; y: number } | null>(null);

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
      if (next[dir] && !tree[dir]) loadDir(dir);
      return next;
    });
  };

  // ---- operations ----

  const act = async (fn: () => Promise<unknown>, refresh: () => void) => {
    try {
      await fn();
      refresh();
    } catch (e) {
      onError(String(e));
    }
  };

  const parentDir = (p: string) => {
    const i = p.lastIndexOf("/");
    return i === -1 ? "" : p.slice(0, i);
  };

  const move = (src: TreeTarget, destDir: string) =>
    act(
      () =>
        invoke("move_file", {
          workspace: workspace.name,
          repo: src.repo,
          src: src.path,
          destDir,
        }),
      () => {
        loadDir(destDir || "");
        const srcParent = parentDir(src.path) || "";
        if (srcParent !== destDir) loadDir(srcParent);
      }
    );

  // Keep move reachable from the global drag listeners without re-binding
  const moveRef = useRef(move);
  moveRef.current = move;

  const del = (target: TreeTarget) =>
    act(
      () => invoke("delete_node", { workspace: workspace.name, repo: target.repo, path: target.path }),
      () => {
        loadDir(parentDir(target.path));
        loadDir("");
      }
    );

  const create = (dir: string, name: string, isDir: boolean) =>
    act(
      () => invoke("create_node", { workspace: workspace.name, repo, dir, name, isDir }),
      () => {
        if (dir && !expanded[dir]) toggleDir(dir);
        loadDir(dir || "");
      }
    );

  const rename = (target: TreeTarget, newName: string) =>
    act(
      () => invoke("rename_node", { workspace: workspace.name, repo: target.repo, path: target.path, newName }),
      () => {
        loadDir(parentDir(target.path));
        loadDir("");
      }
    );

  const copyPath = async (target: TreeTarget) => {
    try {
      const p = await invoke<string>("node_abs_path", {
        workspace: workspace.name,
        repo: target.repo,
        path: target.path,
      });
      await navigator.clipboard.writeText(p);
    } catch (e) {
      onError(String(e));
    }
  };

  const addFiles = async (dir: string) => {
    try {
      const picked = await openDialog({
        multiple: true,
        title: "Add files to the workspace",
      });
      if (!picked) return;
      const sources = Array.isArray(picked) ? picked : [picked];
      if (sources.length === 0) return;
      const copied = await invoke<number>("import_files", {
        workspace: workspace.name,
        repo,
        destDir: dir,
        sources,
      });
      if (copied > 0) {
        if (dir && !expanded[dir]) toggleDir(dir);
        loadDir(dir || "");
      }
    } catch (e) {
      onError(String(e));
    }
  };

  const contextItems = (t: TreeTarget): MenuItem[] => [
    ...(t.isDir
      ? [
          { label: "New File…", onSelect: () => { setCreatePrompt({ dir: t.path, isDir: false }); setCreateValue(""); } },
          { label: "New Folder…", onSelect: () => { setCreatePrompt({ dir: t.path, isDir: true }); setCreateValue(""); } },
          { label: "Add Files…", onSelect: () => addFiles(t.path) },
        ]
      : [
          { label: "Open", onSelect: () => onOpenFile(t.repo, t.path) },
        ]),
    { label: "Rename…", onSelect: () => { setRenameTarget(t); setRenameValue(t.path.split("/").pop() ?? ""); } },
    { label: "Copy Path", onSelect: () => copyPath(t) },
    { label: "Reveal in Finder", onSelect: () => act(() => invoke("reveal_node", { workspace: workspace.name, repo: t.repo, path: t.path }), () => {}) },
    { label: "Delete", danger: true, onSelect: () => del(t) },
  ];

  const confirmCreate = () => {
    if (!createPrompt || !createValue.trim()) {
      setCreatePrompt(null);
      return;
    }
    const { dir, isDir } = createPrompt;
    setCreatePrompt(null);
    create(dir, createValue.trim(), isDir);
  };

  // ---- manual drag: global listeners ----

  useEffect(() => {
    const dropDirAt = (x: number, y: number): string | null => {
      const el = document.elementFromPoint(x, y) as HTMLElement | null;
      const dropEl = el?.closest("[data-drop-dir]") as HTMLElement | null;
      return dropEl ? dropEl.dataset.dropDir ?? "" : null;
    };

    const onMouseMove = (e: MouseEvent) => {
      const st = dragRef.current;
      if (!st.src) return;
      if (!st.active) {
        const dx = e.clientX - st.startX;
        const dy = e.clientY - st.startY;
        if (dx * dx + dy * dy < 16) return; // still within click threshold
        st.active = true;
        setDragging(st.src);
      }
      setGhostPos({ x: e.clientX, y: e.clientY });
      const overEl = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null;
      const overTerminal = !!overEl?.closest(".terminal-host");
      setDragOverTerminal(overTerminal);
      if (overTerminal) {
        setDragHover(null);
        return;
      }
      const dir = dropDirAt(e.clientX, e.clientY);
      if (dir === null) {
        setDragHover(null);
        return;
      }
      const srcPath = st.src.path;
      const isSelf = dir === srcPath;
      const isOwnSubtree = dir.startsWith(srcPath + "/");
      setDragHover(isSelf || isOwnSubtree ? null : dir);
    };

    const endDrag = (e: MouseEvent | null) => {
      const st = dragRef.current;
      if (st.src && st.active) {
        justDraggedRef.current = true;
        setTimeout(() => (justDraggedRef.current = false), 200);
        if (e) {
          const src = st.src;
          // 1) Dropped over an agent terminal? Inject the path into its PTY.
          const el = document.elementFromPoint(e.clientX, e.clientY) as HTMLElement | null;
          const termHost = el?.closest(".terminal-host") as HTMLElement | null;
          if (termHost) {
            termHost.dispatchEvent(
              new CustomEvent("orbit-drop-file", { detail: { path: src.path } })
            );
          } else {
            // 2) Otherwise: folder move, as before.
            const dir = dropDirAt(e.clientX, e.clientY);
            if (dir !== null && dir !== src.path && !dir.startsWith(src.path + "/")) {
              moveRef.current(src, dir);
            }
          }
        }
      }
      dragRef.current = { startX: 0, startY: 0, src: null, active: false };
      setDragging(null);
      setDragHover(null);
      setDragOverTerminal(false);
      setGhostPos(null);
    };

    const onMouseUp = (e: MouseEvent) => endDrag(e);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && dragRef.current.src) endDrag(null);
    };

    window.addEventListener("mousemove", onMouseMove);
    window.addEventListener("mouseup", onMouseUp);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousemove", onMouseMove);
      window.removeEventListener("mouseup", onMouseUp);
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  const startPotentialDrag = (e: React.MouseEvent, target: TreeTarget) => {
    if (e.button !== 0 || renameTarget) return;
    dragRef.current = { startX: e.clientX, startY: e.clientY, src: target, active: false };
  };

  // ---- render ----

  const renameInput = (currentName: string) => (
    <input
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
      className="tree-rename"
      autoFocus
      value={renameValue}
      onChange={(e) => setRenameValue(e.target.value)}
      onBlur={() => {
        if (renameValue.trim() && renameValue !== currentName && renameTarget) {
          rename(renameTarget, renameValue.trim());
        }
        setRenameTarget(null);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          if (renameValue.trim() && renameValue !== currentName && renameTarget) {
            rename(renameTarget, renameValue.trim());
          }
          setRenameTarget(null);
        } else if (e.key === "Escape") setRenameTarget(null);
        e.stopPropagation();
      }}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
    />
  );

  const renderDir = (dir: string, depth: number): React.ReactNode => {
    const children = tree[dir] ?? [];
    return (
      <>
        {children
          .filter((n) => n.is_dir)
          .map((n) => (
            <div key={n.path}>
              <button
                className={`tree-item tree-dir depth-${Math.min(depth, 6)} ${dragHover === n.path ? "tree-drop-target" : ""}`}
                data-drop-dir={n.path}
                onClick={() => {
                  if (justDraggedRef.current) return;
                  toggleDir(n.path);
                }}
                onContextMenu={(e) => {
                  e.preventDefault();
                  menu.setMenu({ x: e.clientX, y: e.clientY, payload: { repo, path: n.path, isDir: true } });
                }}
                onMouseDown={(e) => {
                  menu.openFromEvent(e, { repo, path: n.path, isDir: true });
                  startPotentialDrag(e, { repo, path: n.path, isDir: true });
                }}
                onPointerDown={(e) => menu.openFromEvent(e, { repo, path: n.path, isDir: true })}
              >
                <span className="tree-chevron-small">{expanded[n.path] ? "▾" : "▸"}</span>
                <FolderTreeIcon open={!!expanded[n.path]} />
                {renameTarget?.path === n.path ? (
                  renameInput(n.name)
                ) : (
                  <span className="tree-name">{n.name}</span>
                )}
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
              onClick={() => {
                if (justDraggedRef.current) return;
                onOpenFile(repo, n.path);
              }}
              onContextMenu={(e) => {
                e.preventDefault();
                menu.setMenu({ x: e.clientX, y: e.clientY, payload: { repo, path: n.path, isDir: false } });
              }}
              onMouseDown={(e) => {
                menu.openFromEvent(e, { repo, path: n.path, isDir: false });
                startPotentialDrag(e, { repo, path: n.path, isDir: false });
              }}
              onPointerDown={(e) => menu.openFromEvent(e, { repo, path: n.path, isDir: false })}
            >
              <span className="tree-chevron-small invisible" />
              <FileTypeIcon name={n.name} />
              {renameTarget?.path === n.path ? (
                renameInput(n.name)
              ) : (
                <span className="tree-name">{n.name}</span>
              )}
            </button>
          ))}
      </>
    );
  };

  const dockItems: { id: DockView | "find"; label: string; icon: React.ReactNode }[] = [
    { id: "files", label: "Files", icon: <FolderIcon /> },
    { id: "find", label: "Find in Files (Ctrl+Shift+F)", icon: <SearchIcon /> },
    { id: "git", label: "Git", icon: <GitIcon /> },
  ];

  return (
    <div className="dock-panel">
      <div className="dock-icons">
        {dockItems.map((item) => (
          <button
            key={item.id}
            className={`dock-icon ${view === item.id ? "dock-icon-active" : ""}`}
            title={item.label}
            onClick={() => (item.id === "find" ? onOpenFind() : setView(item.id))}
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
            <select className="dock-select" value={repo} onChange={(e) => setRepo(e.target.value)}>
              {workspace.repos.map((r) => (
                <option key={r} value={r}>
                  {r}
                </option>
              ))}
            </select>
          </div>
          <div
            className={`dock-panel-body ${dragHover === "" ? "tree-drop-target-body" : ""}`}
            data-drop-dir=""
          >
            {renderDir("", 0)}
          </div>
        </>
      )}
      {view === "git" && (
        <GitPanel
          workspace={workspace}
          onReviewFile={onReviewFile}
          onReviewCommit={onReviewCommit}
          onError={onError}
        />
      )}

      {/* Floating ghost with the dragged file name */}
      {dragging && ghostPos && (
        <div className="tree-drag-ghost" style={{ left: ghostPos.x + 12, top: ghostPos.y + 14 }}>
          {dragging.path.split("/").pop()}
          {dragOverTerminal ? (
            <span className="tree-drag-ghost-target"> → agent</span>
          ) : dragHover !== null ? (
            <span className="tree-drag-ghost-target"> → {dragHover || "root"}</span>
          ) : null}
        </div>
      )}

      {menu.menu && (
        <ContextMenu
          x={menu.menu.x}
          y={menu.menu.y}
          items={contextItems(menu.menu.payload)}
          onClose={() => menu.setMenu(null)}
        />
      )}

      {createPrompt && (
        <div className="modal-overlay" onMouseDown={() => setCreatePrompt(null)}>
          <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
            <div className="modal-body">
              <h3>{createPrompt.isDir ? "New folder" : "New file"}</h3>
              <p className="dock-create-hint">
                {repo}{createPrompt.dir ? `/${createPrompt.dir}` : ""}
              </p>
              <input
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                className="dock-create-input"
                autoFocus
                value={createValue}
                placeholder={createPrompt.isDir ? "folder-name" : "file-name.ts"}
                onChange={(e) => setCreateValue(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") confirmCreate();
                  if (e.key === "Escape") setCreatePrompt(null);
                }}
              />
            </div>
            <div className="modal-footer">
              <button className="secondary" onClick={() => setCreatePrompt(null)}>
                Cancel
              </button>
              <button onClick={confirmCreate} disabled={!createValue.trim()}>
                Create
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}