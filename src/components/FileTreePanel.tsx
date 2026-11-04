import { memo, ReactNode, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { FolderIcon, GitIcon, SearchIcon } from "./Icons";
import { FileTypeIcon, FolderTreeIcon } from "./FileIcons";
import { ContextMenu, MenuItem, useContextMenu } from "./ContextMenu";
import { GitPanel } from "./GitPanel";
import { useFsChanges } from "../lib/useFsChanges";
import { GitCommit, Workspace } from "../types/config";
import { useStableHandlers } from "../lib/useStableHandlers";

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
  /** `preview`: a single click, which reuses the preview tab. */
  onOpenFile: (repo: string, path: string, opts?: { preview?: boolean }) => void;
  /** Opens the diff review for a changed file (Git tab). */
  onReviewFile: (repo: string, path: string) => void;
  /** Opens the Review tab with every change of the workspace. */
  onReviewAll: () => void;
  /** Opens commit inspection (Git tab). */
  onReviewCommit: (repo: string, commit: GitCommit) => void;
  /** Opens the Find in Files popup. */
  onOpenFind: () => void;
  onError: (msg: string) => void;
  /** Repo of the active editor tab: the tree follows it. */
  activeRepo?: string;
  /** The active tab browses a repo (no file open): show its files. */
  browsing?: boolean;
  /** File of the active editor tab (in `activeRepo`): highlighted and revealed. */
  activePath?: string;
}

type RowHandlers = {
  click: (n: FileNode) => void;
  doubleClick: (n: FileNode) => void;
  contextMenu: (e: React.MouseEvent, n: FileNode) => void;
  mouseDown: (e: React.MouseEvent, n: FileNode) => void;
  pointerDown: (e: React.PointerEvent, n: FileNode) => void;
};

/** One tree row. Memoized: hovering or re-rendering the panel only touches
 *  rows whose own state changed. */
const TreeRow = memo(function TreeRow({ node, depth, open, active, dropTarget, rename, h }: {
  node: FileNode;
  depth: number;
  open: boolean;
  active: boolean;
  dropTarget: boolean;
  rename: ReactNode;
  h: RowHandlers;
}) {
  return (
    <button
      className={`tree-item ${node.is_dir ? "tree-dir" : "tree-file"}${active ? " tree-active" : ""}${dropTarget ? " tree-drop-target" : ""}`}
      style={{ "--depth": depth } as React.CSSProperties}
      data-path={node.path}
      data-drop-dir={node.is_dir ? node.path : undefined}
      onClick={() => h.click(node)}
      onDoubleClick={() => h.doubleClick(node)}
      onContextMenu={(e) => h.contextMenu(e, node)}
      onMouseDown={(e) => h.mouseDown(e, node)}
      onPointerDown={(e) => h.pointerDown(e, node)}
    >
      {node.is_dir ? (
        <>
          <span className="tree-chevron-small">{open ? "▾" : "▸"}</span>
          <FolderTreeIcon open={open} />
        </>
      ) : (
        <>
          <span className="tree-chevron-small invisible" />
          <FileTypeIcon name={node.name} />
        </>
      )}
      {rename ?? <span className="tree-name">{node.name}</span>}
    </button>
  );
});

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
export const FileTreePanel = memo(function FileTreePanel({
  workspace,
  onOpenFile,
  onReviewFile,
  onReviewAll,
  onReviewCommit,
  onOpenFind,
  onError,
  activeRepo,
  browsing,
  activePath,
}: Props) {
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

  // Opening a repo's files (Browse files) or switching editor tabs: the
  // tree shows that repo.
  useEffect(() => {
    if (activeRepo && workspace.repos.includes(activeRepo)) setRepo(activeRepo);
  }, [activeRepo, workspace.repos]);
  useEffect(() => {
    if (browsing) setView("files");
  }, [browsing, activeRepo]);

  // Files created or deleted elsewhere (another IDE, an agent): the watcher
  // reports them and the open folders they touched are reloaded.
  const expandedRef = useRef(expanded);
  expandedRef.current = expanded;
  const hostRef = useRef<HTMLDivElement>(null);
  // Re-list the open folders a change touched (any of them for git/focus).
  useFsChanges(repo && view === "files" ? workspace.name : null, repo, (c) => {
    if (!hostRef.current?.offsetParent) return;
    const parents = new Set(c.paths.map((p) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "")));
    const all = c.git || c.paths.length === 0;
    const dirs = Object.keys(expandedRef.current).filter((d) => expandedRef.current[d] && (all || parents.has(d)));
    for (const dir of dirs) {
        invoke<FileNode[]>("list_files", { workspace: workspace.name, repo, path: dir })
          .then((nodes) =>
            setTree((t) => (JSON.stringify(t[dir]) === JSON.stringify(nodes) ? t : { ...t, [dir]: nodes }))
          )
          .catch(() => null); // gone (deleted folder) or transient: the next change settles it
    }
  });

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

  const target = (n: FileNode): TreeTarget => ({ repo, path: n.path, isDir: n.is_dir });
  const rowHandlers = useStableHandlers<RowHandlers>({
    click: (n) => {
      if (justDraggedRef.current) return;
      if (n.is_dir) toggleDir(n.path);
      else onOpenFile(repo, n.path, { preview: true });
    },
    doubleClick: (n) => {
      if (!n.is_dir) onOpenFile(repo, n.path);
    },
    contextMenu: (e, n) => {
      e.preventDefault();
      menu.setMenu({ x: e.clientX, y: e.clientY, payload: target(n) });
    },
    mouseDown: (e, n) => {
      menu.openFromEvent(e, target(n));
      startPotentialDrag(e, target(n));
    },
    pointerDown: (e, n) => menu.openFromEvent(e, target(n)),
  });

  // Follow the active editor: open the folders down to its file and scroll
  // the row into view.
  const shownPath = activeRepo === repo ? activePath : undefined;
  useEffect(() => {
    if (!shownPath || view !== "files") return;
    const parts = shownPath.split("/").slice(0, -1);
    const dirs = parts.map((_, i) => parts.slice(0, i + 1).join("/"));
    setExpanded((x) => (dirs.every((d) => x[d]) ? x : { ...x, ...Object.fromEntries(dirs.map((d) => [d, true])) }));
    for (const d of dirs) if (!tree[d]) loadDir(d);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shownPath, repo, view]);
  const shownLoaded = !!shownPath && !!tree[shownPath.includes("/") ? shownPath.slice(0, shownPath.lastIndexOf("/")) : ""];
  useEffect(() => {
    if (!shownPath || !shownLoaded) return;
    const row = hostRef.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(shownPath)}"]`);
    row?.scrollIntoView({ block: "nearest" });
  }, [shownPath, shownLoaded]);

  /** Arrow keys move between rows, ←/→ fold and unfold folders, Enter
   *  opens a file in a tab of its own. */
  const onTreeKey = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (renameTarget || e.altKey || e.ctrlKey || e.metaKey) return;
    const rows = Array.from(e.currentTarget.querySelectorAll<HTMLElement>("button.tree-item"));
    const row = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>("button.tree-item");
    const at = row ? rows.indexOf(row) : -1;
    const path = row?.dataset.path ?? "";
    const isDir = !!row?.classList.contains("tree-dir");
    const focus = (el: HTMLElement | undefined | null) => {
      if (!el) return;
      e.preventDefault();
      el.focus();
      el.scrollIntoView({ block: "nearest" });
    };
    switch (e.key) {
      case "ArrowDown":
        return focus(rows[at + 1] ?? rows[at]);
      case "ArrowUp":
        return focus(rows[Math.max(at - 1, 0)]);
      case "ArrowRight":
        if (!row || !isDir) return;
        e.preventDefault();
        if (!expanded[path]) toggleDir(path);
        else focus(rows[at + 1]);
        return;
      case "ArrowLeft": {
        if (!row) return;
        e.preventDefault();
        if (isDir && expanded[path]) return toggleDir(path);
        const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : null;
        if (parent) focus(rows.find((r) => r.dataset.path === parent));
        return;
      }
      case "Enter":
        if (!row || isDir) return;
        e.preventDefault();
        onOpenFile(repo, path);
        return;
    }
  };

  // The editor's breadcrumb: show a folder (open it and its parents, select it).
  const [revealDir, setRevealDir] = useState<string | null>(null);
  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<{ repo: string; path: string }>).detail;
      if (!workspace.repos.includes(d.repo)) return;
      setRepo(d.repo);
      setView("files");
      setRevealDir(d.path);
    };
    window.addEventListener("orbit-reveal-in-tree", on);
    return () => window.removeEventListener("orbit-reveal-in-tree", on);
  }, [workspace.repos]);
  useEffect(() => {
    if (revealDir === null) return;
    const parts = revealDir ? revealDir.split("/") : [];
    const dirs = ["", ...parts.map((_, i) => parts.slice(0, i + 1).join("/"))];
    // Here rather than in the event: switching repos resets the tree first.
    setExpanded((x) => (dirs.every((d) => x[d]) ? x : { ...x, ...Object.fromEntries(dirs.map((d) => [d, true])) }));
    const missing = dirs.filter((d) => !tree[d]);
    if (missing.length) {
      missing.forEach((d) => loadDir(d));
      return;
    }
    const row = revealDir
      ? hostRef.current?.querySelector<HTMLElement>(`[data-path="${CSS.escape(revealDir)}"]`)
      : hostRef.current?.querySelector<HTMLElement>("button.tree-item");
    row?.scrollIntoView({ block: "nearest" });
    row?.focus();
    setRevealDir(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealDir, tree]);

  const renderDir = (dir: string, depth: number): React.ReactNode => {
    const children = tree[dir] ?? [];
    const row = (n: FileNode) => (
      <TreeRow
        key={n.path}
        node={n}
        depth={depth}
        open={!!expanded[n.path]}
        active={n.path === shownPath}
        dropTarget={dragHover === n.path}
        rename={renameTarget?.path === n.path ? renameInput(n.name) : null}
        h={rowHandlers}
      />
    );
    return (
      <>
        {children
          .filter((n) => n.is_dir)
          .map((n) => (
            <div key={n.path}>
              {row(n)}
              {expanded[n.path] && renderDir(n.path, depth + 1)}
            </div>
          ))}
        {children.filter((n) => !n.is_dir).map(row)}
      </>
    );
  };

  const dockItems: { id: DockView | "find"; label: string; icon: React.ReactNode }[] = [
    { id: "files", label: "Files", icon: <FolderIcon /> },
    { id: "find", label: "Find in Files (Ctrl+Shift+F)", icon: <SearchIcon /> },
    { id: "git", label: "Git", icon: <GitIcon /> },
  ];

  return (
    <div className="dock-panel" ref={hostRef}>
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
            onKeyDown={onTreeKey}
          >
            {renderDir("", 0)}
          </div>
        </>
      )}
      {view === "git" && (
        <GitPanel
          workspace={workspace}
          onReviewFile={onReviewFile}
          onReviewAll={onReviewAll}
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
});
