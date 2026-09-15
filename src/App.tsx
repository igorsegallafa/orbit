import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useConfig } from "./hooks/useConfig";
import { DashboardPage } from "./pages/DashboardPage";
import { SettingsPage } from "./pages/SettingsPage";
import { IntegrationsPage } from "./pages/IntegrationsPage";
import { WorkspaceDetailPage } from "./pages/WorkspaceDetailPage";
import { SidebarResizer } from "./components/SidebarResizer";
import { ContextMenu, MenuItem, useContextMenu } from "./components/ContextMenu";
import { EditorPane } from "./components/EditorPane";
import { TerminalPane, TerminalTab } from "./components/TerminalPane";
import { FileTreePanel } from "./components/FileTreePanel";
import { SearchEverywhereModal } from "./components/SearchEverywhereModal";
import { HomeIcon, SettingsIcon, SatelliteIcon, DocIcon, TerminalIcon, PlusIcon, ChevronRightIcon, PlugIcon } from "./components/Icons";
import { UsageBar } from "./components/UsageBar";
import { SkeletonCards, SkeletonTable } from "./components/Skeleton";
import { randomSessionName } from "./lib/names";
import { AgentStatus } from "./lib/agentStatus";
import { StatusIndicator } from "./components/StatusIndicator";
import { Workspace } from "./types/config";
import "./App.css";

type NavPage = { kind: "dashboard" } | { kind: "settings" } | { kind: "integrations" };

type Tab =
  | { kind: "workspace"; workspace: Workspace }
  | { kind: "editor"; workspace: string; repo: string; path: string }
  | { kind: "terminal"; terminal: TerminalTab };

const SIDEBAR_KEY = "orbit.sidebar-width";
const DOCK_KEY = "orbit.dock-width";
const DEFAULT_WIDTH = 220;
const COLLAPSED = 64;

function loadStored(key: string, def: number, min: number, max: number): number {
  const n = Number(localStorage.getItem(key));
  return Number.isFinite(n) && n >= min && n <= max ? n : def;
}

function tabId(tab: Tab): string {
  const t = tab;
  if (t.kind === "workspace") return `ws:${t.workspace.name}`;
  if (t.kind === "editor") return `ed:${t.workspace}/${t.repo}/${t.path}`;
  // Stable id: must NOT embed the display name, or renaming re-keys the pane
  // and remounts the terminal (killing the PTY session).
  return t.terminal.id;
}

function App() {
  const { config, setConfig, loading, error, setError } = useConfig();
  const [navPage, setNavPage] = useState<NavPage>({ kind: "dashboard" });
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeTab, setActiveTab] = useState<string | null>(null);
  const [sessionStatuses, setSessionStatuses] = useState<Record<string, AgentStatus>>({});
  const [renamingTab, setRenamingTab] = useState<string | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(() => loadStored(SIDEBAR_KEY, DEFAULT_WIDTH, 64, 400));
  const [dockWidth, setDockWidth] = useState(() => loadStored(DOCK_KEY, 240, 180, 460));
  const [searchOpen, setSearchOpen] = useState(false);
  const [plusMenu, setPlusMenu] = useState<{ x: number; y: number } | null>(null);
  const [wsExpanded, setWsExpanded] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem("orbit.ws-expanded") ?? "{}");
    } catch {
      return {};
    }
  });
  const { menu, setMenu, openFromEvent } = useContextMenu<Workspace>();
  const tabMenu = useContextMenu<Tab>();

  const loadWorkspaces = useCallback(async () => {
    try {
      setWorkspaces(await invoke<Workspace[]>("list_workspaces"));
    } catch (e) {
      setError(String(e));
    }
  }, [setError]);

  useEffect(() => {
    loadWorkspaces();
  }, [loadWorkspaces]);

  // JetBrains-style double-shift: two bare Shift presses within 350ms open
  // Search Everywhere. Guarded against Shift+key combos (typing capitals).
  useEffect(() => {
    let lastShift = 0;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Shift" && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const now = Date.now();
        if (now - lastShift < 350) {
          lastShift = 0;
          setSearchOpen((open) => !open);
        } else {
          lastShift = now;
        }
      } else if (e.key !== "Meta" && e.key !== "Control" && e.key !== "Alt") {
        lastShift = 0;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onResize = useCallback((w: number) => {
    setSidebarWidth(w);
    localStorage.setItem(SIDEBAR_KEY, String(w));
  }, []);

  const onDockResize = useCallback(
    (w: number) => {
      setDockWidth(w);
      localStorage.setItem(DOCK_KEY, String(w));
    },
    []
  );

  const collapsed = sidebarWidth <= COLLAPSED;
  const repoCount = config.services.length;
  const groupCount = Object.keys(config.groups).length;

  const openTab = (tab: Tab) => {
    const id = tabId(tab);
    setTabs((ts) => (ts.some((t) => tabId(t) === id) ? ts : [...ts, tab]));
    setActiveTab(id);
  };

  const closeTab = (id: string) => {
    setTabs((ts) => {
      const idx = ts.findIndex((t) => tabId(t) === id);
      const next = ts.filter((t) => tabId(t) !== id);
      if (activeTab === id) {
        const nextTab = next[Math.min(idx, next.length - 1)];
        // Never leave a null active tab while tabs remain — the tab body
        // would render an empty viewport.
        setActiveTab(nextTab ? tabId(nextTab) : null);
      }
      return next;
    });
    setSessionStatuses((s) => {
      const { [id]: _drop, ...rest } = s;
      return rest;
    });
  };

  // Batch closes for the tab context menu. All remount their panes, so PTY
  // sessions in closed tabs end naturally via cleanup.
  const closeOtherTabs = (keepId: string) => {
    const keep = tabs.find((t) => tabId(t) === keepId);
    if (!keep) return;
    setTabs((ts) => {
      for (const t of ts) {
        if (tabId(t) !== keepId) {
          setSessionStatuses((s) => {
            const { [tabId(t)]: _drop, ...rest } = s;
            return rest;
          });
        }
      }
      return [keep];
    });
    setActiveTab(keepId);
  };

  const closeAllTabs = () => {
    setTabs([]);
    setActiveTab(null);
    setSessionStatuses({});
  };

  const tabMenuItems = (t: Tab): MenuItem[] => [
    {
      label: "Close",
      onSelect: () => closeTab(tabId(t)),
    },
    {
      label: "Close Other Tabs",
      onSelect: () => closeOtherTabs(tabId(t)),
    },
    {
      label: "Close All Tabs",
      danger: true,
      onSelect: () => closeAllTabs(),
    },
  ];

  const openWorkspaceTab = (ws: Workspace) => {
    setNavPage({ kind: "dashboard" });
    openTab({ kind: "workspace", workspace: ws });
  };

  const openFileTab = (wsName: string, repo: string, path: string) => {
    setNavPage({ kind: "dashboard" });
    openTab({ kind: "editor", workspace: wsName, repo, path });
  };

  const newTerminal = (wsName: string, label: string, cmd: string | null) => {
    const taken = tabs
      .filter((t): t is Extract<Tab, { kind: "terminal" }> => t.kind === "terminal")
      .map((t) => t.terminal.sessionName);
    openTab({
      kind: "terminal",
      terminal: {
        id: `tm:${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        workspace: wsName,
        label,
        sessionName: randomSessionName(taken),
        cmd,
      },
    });
  };

  const openShellTerminal = (wsName: string) => newTerminal(wsName, "shell", null);

  // Collapse/expand of workspace sessions in the sidebar; persisted so the
  // tree reopens as the user left it.
  const toggleWsExpanded = (wsName: string) => {
    setWsExpanded((prev) => {
      const next = { ...prev, [wsName]: !(prev[wsName] ?? true) };
      localStorage.setItem("orbit.ws-expanded", JSON.stringify(next));
      return next;
    });
  };

  // Rename: display-only. The tab id is stable so the pane never remounts.
  const renameTab = (id: string, newName: string) => {
    const name = newName.trim();
    if (!name) return;
    const target = tabs.find((t) => tabId(t) === id);
    if (!target || target.kind !== "terminal") return;
    if (
      tabs.some(
        (t) =>
          t.kind === "terminal" &&
          t.terminal.workspace === target.terminal.workspace &&
          t.terminal.sessionName === name &&
          tabId(t) !== id
      )
    ) {
      setError(`a session named '${name}' already exists in this workspace`);
      return;
    }
    setTabs((ts) =>
      ts.map((t) =>
        tabId(t) === id && t.kind === "terminal"
          ? { ...t, terminal: { ...t.terminal, sessionName: name } }
          : t
      )
    );
  };

  const setSessionStatus = (id: string, status: AgentStatus) => {
    setSessionStatuses((s) => (s[id] === status ? s : { ...s, [id]: status }));
  };

  const sidebarMenuItems = (ws: Workspace): MenuItem[] => [
    { label: "Open workspace", onSelect: () => openWorkspaceTab(ws) },
    { label: "New terminal", onSelect: () => openShellTerminal(ws.name) },
    {
      label: "Open in editor",
      onSelect: () =>
        invoke("open_workspace_in_editor", { name: ws.name }).catch((e) => setError(String(e))),
    },
    {
      label: "Open folder in Finder",
      onSelect: () =>
        invoke("reveal_workspace_folder", { name: ws.name }).catch((e) => setError(String(e))),
    },
  ];

  const active = tabs.find((t) => tabId(t) === activeTab) ?? null;

  // The right dock shows for whichever workspace is "in focus": the active
  // workspace tab, else a workspace owning the active editor/terminal tab.
  const focusWorkspace: Workspace | null =
    active?.kind === "workspace"
      ? active.workspace
      : active
        ? (workspaces.find(
            (w) => w.name === (active.kind === "editor" ? active.workspace : active.terminal.workspace)
          ) ?? null)
        : null;

  const renderPane = (tab: Tab) => {
    if (tab.kind === "workspace") {
      return (
        <WorkspaceDetailPage
          workspace={tab.workspace}
          onOpenEditor={(repo) => openFileTab(tab.workspace.name, repo, "")}
          onRemoved={() => {
            loadWorkspaces();
            closeTab(tabId(tab));
          }}
          onError={setError}
        />
      );
    }
    if (tab.kind === "editor") {
      return (
        <EditorPane
          workspace={tab.workspace}
          repo={tab.repo}
          path={tab.path}
          onError={setError}
        />
      );
    }
    return (
      <TerminalPane
        tab={tab.terminal}
        onError={setError}
        onStatusChange={(s) => setSessionStatus(tabId(tab), s)}
      />
    );
  };

  const renderMain = () => {
    if (navPage.kind === "settings") {
      return <SettingsPage config={config} onChange={setConfig} onError={setError} />;
    }
    if (navPage.kind === "integrations") {
      return <IntegrationsPage onError={setError} />;
    }
    return (
      <DashboardPage
        config={config}
        workspaces={workspaces}
        onOpen={openWorkspaceTab}
        onChanged={loadWorkspaces}
        onError={setError}
      />
    );
  };

  return (
    <div className="app">
      <div className="app-row">
      <aside className="sidebar" data-collapsed={collapsed} style={{ width: sidebarWidth }}>
        <div className="brand">
          <span className="brand-logo"><SatelliteIcon size={18} /></span> {collapsed ? "" : "Orbit"}
          {!collapsed && <span className="brand-sub">multi-repo workspace</span>}
        </div>

        <nav>
          {!collapsed && <div className="nav-section">Workspaces</div>}
          <button
            className={`nav-item ${navPage.kind === "dashboard" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "dashboard" });
            }}
            title="Dashboard"
          >
            <span className="nav-icon"><HomeIcon size={15} /></span> {!collapsed && "Dashboard"}
          </button>
          {!collapsed &&
            workspaces.map((ws) => {
              const wsId = `ws:${ws.name}`;
              // Sessions under this workspace: terminals (agents/shells) and editors
              const wsSessions = tabs.filter(
                (t): t is Extract<Tab, { kind: "terminal" } | { kind: "editor" }> =>
                  (t.kind === "terminal" && t.terminal.workspace === ws.name) ||
                  (t.kind === "editor" && t.workspace === ws.name)
              );
              return (
                <div key={ws.name} className="nav-workspace">
                  <button
                    className={`nav-item nav-ws ${activeTab === wsId ? "active" : ""}`}
                    onClick={() => openWorkspaceTab(ws)}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setMenu({ x: e.clientX, y: e.clientY, payload: ws });
                    }}
                    onMouseDown={(e) => openFromEvent(e, ws)}
                    onPointerDown={(e) => openFromEvent(e, ws)}
                    title={ws.name}
                  >
                    <span className="nav-icon"><SatelliteIcon size={15} /></span>
                    <span className="nav-item-label">{ws.name}</span>
                    {wsSessions.length > 0 && (
                      <span
                        role="button"
                        tabIndex={0}
                        className={`nav-chevron ${wsExpanded[ws.name] === false ? "" : "open"}`}
                        title={wsExpanded[ws.name] === false ? "Expand sessions" : "Collapse sessions"}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleWsExpanded(ws.name);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.stopPropagation();
                            toggleWsExpanded(ws.name);
                          }
                        }}
                      >
                        <ChevronRightIcon size={8} />
                      </span>
                    )}
                  </button>
                  {(wsExpanded[ws.name] ?? true) &&
                    wsSessions.map((t) => {
                      const id = tabId(t);
                      const label =
                        t.kind === "terminal" ? t.terminal.sessionName : `${t.repo}/${t.path.split("/").pop()}`;
                      return (
                        <button
                          key={id}
                          className={`nav-item nav-sub ${activeTab === id ? "active" : ""}`}
                          onClick={() => setActiveTab(id)}
                          title={label}
                        >
                          {t.kind === "terminal" ? (
                            <StatusIndicator status={sessionStatuses[id] ?? "idle"} />
                          ) : (
                            <span className="nav-icon"><DocIcon size={13} /></span>
                          )}
                          <span className="nav-item-label">{label}</span>
                        </button>
                      );
                    })}
                </div>
              );
            })}

          {!collapsed && <div className="nav-section">General</div>}
          <button
            className={`nav-item ${navPage.kind === "settings" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "settings" });
            }}
            title="Settings"
          >
            <span className="nav-icon"><SettingsIcon size={15} /></span> {!collapsed && "Settings"}
            {!collapsed && repoCount + groupCount > 0 && (
              <span className="nav-count">{repoCount + groupCount}</span>
            )}
          </button>
          <button
            className={`nav-item ${navPage.kind === "integrations" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "integrations" });
            }}
            title="Integrations"
          >
            <span className="nav-icon"><PlugIcon size={15} /></span> {!collapsed && "Integrations"}
          </button>
        </nav>

        {!collapsed && (
          <div className="sidebar-footer">
            <div className="sidebar-stat">
              <span className="sidebar-stat-value">{repoCount}</span> repos
            </div>
            <div className="sidebar-stat">
              <span className="sidebar-stat-value">{groupCount}</span> groups
            </div>
          </div>
        )}
      </aside>

      <SidebarResizer width={sidebarWidth} onResize={onResize} />

      <main className="content">
        {error && (
          <div className="banner error" onClick={() => setError(null)}>
            {error}
          </div>
        )}
        {loading ? (
          <div className="nav-page">
            <SkeletonCards n={4} />
            <div style={{ marginTop: 20 }} />
            <SkeletonTable rows={4} cols={3} />
          </div>
        ) : tabs.length > 0 ? (
          <>
            <div
              className="tab-strip"
              onWheel={(e) => {
                // WKWebView: vertical wheel/two-finger gestures don't scroll
                // overflow-x containers — translate deltaY into scrollLeft.
                if (e.deltaY !== 0 && e.currentTarget.scrollWidth > e.currentTarget.clientWidth) {
                  e.currentTarget.scrollLeft += e.deltaY;
                  e.preventDefault();
                }
              }}
            >
              {tabs.map((t) => {
                const id = tabId(t);
                const label =
                  t.kind === "workspace"
                    ? t.workspace.name
                    : t.kind === "editor"
                      ? `${t.repo}/${t.path.split("/").pop()}`
                      : t.terminal.sessionName;
                const icon =
                  t.kind === "workspace" ? (
                    <SatelliteIcon size={13} />
                  ) : t.kind === "editor" ? (
                    <DocIcon size={13} />
                  ) : (
                    <TerminalIcon size={13} />
                  );
                const status = t.kind === "terminal" ? sessionStatuses[id] : undefined;
                return (
                  <div
                    key={id}
                    className={`tab ${activeTab === id ? "tab-active" : ""}`}
                    onClick={() => setActiveTab(id)}
                    onDoubleClick={() => {
                      if (t.kind === "terminal") setRenamingTab(id);
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      tabMenu.setMenu({ x: e.clientX, y: e.clientY, payload: t });
                    }}
                    onMouseDown={(e) => {
                      if (e.button === 1) {
                        e.preventDefault();
                        closeTab(id);
                      } else {
                        tabMenu.openFromEvent(e, t);
                      }
                    }}
                    onPointerDown={(e) => tabMenu.openFromEvent(e, t)}
                  >
                    <span className="tab-icon">{icon}</span>
                    {renamingTab === id ? (
                      <input
                        className="tab-rename"
                        autoFocus
                        defaultValue={label}
                        onBlur={(e) => {
                          renameTab(id, e.target.value);
                          setRenamingTab(null);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            renameTab(id, e.currentTarget.value);
                            setRenamingTab(null);
                          } else if (e.key === "Escape") {
                            setRenamingTab(null);
                          }
                          e.stopPropagation();
                        }}
                        onMouseDown={(e) => e.stopPropagation()}
                        onClick={(e) => e.stopPropagation()}
                        onDoubleClick={(e) => e.stopPropagation()}
                      />
                    ) : (
                      <span className="tab-label">{label}</span>
                    )}
                    {status && <StatusIndicator status={status} />}
                    <button
                      className="tab-close"
                      onClick={(e) => {
                        e.stopPropagation();
                        closeTab(id);
                      }}
                    >
                      ×
                    </button>
                  </div>
                );
              })}
              <button
                className="tab-plus"
                title="New…"
                onClick={(e) => {
                  const r = e.currentTarget.getBoundingClientRect();
                  setPlusMenu({ x: r.left, y: r.bottom + 4 });
                }}
              >
                <PlusIcon size={14} />
              </button>
            </div>
            {tabs.map((t) => (
              <div
                key={tabId(t)}
                className={`tab-body ${activeTab === tabId(t) ? "tab-body-active" : ""}`}
              >
                {renderPane(t)}
              </div>
            ))}
            {/* No active tab = user navigated to Dashboard/Settings via the
                sidebar: render the nav page alongside open (hidden) tabs. */}
            {!activeTab && <div className="nav-page">{renderMain()}</div>}
          </>
        ) : (
          <div className="nav-page">{renderMain()}</div>
        )}

        {/* Search Everywhere (double-shift) for the focused workspace */}
      {searchOpen && focusWorkspace && (
        <SearchEverywhereModal
          workspace={focusWorkspace}
          onOpenFile={(repo, path) => {
            setNavPage({ kind: "dashboard" });
            openTab({ kind: "editor", workspace: focusWorkspace.name, repo, path });
          }}
          onClose={() => setSearchOpen(false)}
        />
      )}

      {menu && (
          <ContextMenu
            x={menu.x}
            y={menu.y}
            items={sidebarMenuItems(menu.payload)}
            onClose={() => setMenu(null)}
          />
        )}
      {tabMenu.menu && (
        <ContextMenu
          x={tabMenu.menu.x}
          y={tabMenu.menu.y}
          items={tabMenuItems(tabMenu.menu.payload)}
          onClose={() => tabMenu.setMenu(null)}
        />
      )}
      {plusMenu && focusWorkspace && (
        <ContextMenu
          x={plusMenu.x}
          y={plusMenu.y}
          items={[
            { label: "Terminal", onSelect: () => openShellTerminal(focusWorkspace.name) },
            { label: "Claude Code", onSelect: () => newTerminal(focusWorkspace.name, "claude", "claude") },
            { label: "Open Code", onSelect: () => newTerminal(focusWorkspace.name, "opencode", "opencode") },
          ]}
          onClose={() => setPlusMenu(null)}
        />
      )}
      </main>

      {/* Fixed right dock, full height, visible while a workspace is in focus */}
      {focusWorkspace && (
        <>
          <div
            className="dock-resizer"
            onMouseDown={() => {
              const onMove = (e: MouseEvent) => {
                onDockResize(Math.min(460, Math.max(180, window.innerWidth - e.clientX)));
              };
              const onUp = () => {
                window.removeEventListener("mousemove", onMove);
                window.removeEventListener("mouseup", onUp);
                document.body.style.cursor = "";
              };
              document.body.style.cursor = "col-resize";
              window.addEventListener("mousemove", onMove);
              window.addEventListener("mouseup", onUp);
            }}
          />
          <aside className="dock" style={{ width: dockWidth }}>
            <FileTreePanel
              workspace={focusWorkspace}
              onOpenFile={(repo, path) => openFileTab(focusWorkspace.name, repo, path)}
              onError={setError}
            />
          </aside>
        </>
      )}
      </div>

      {/* AI usage bottom bar for the focused workspace */}
      {focusWorkspace && <UsageBar workspace={focusWorkspace.name} />}
    </div>
  );
}

export default App;