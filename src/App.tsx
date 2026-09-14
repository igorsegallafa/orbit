import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useConfig } from "./hooks/useConfig";
import { DashboardPage } from "./pages/DashboardPage";
import { SettingsPage } from "./pages/SettingsPage";
import { WorkspaceDetailPage } from "./pages/WorkspaceDetailPage";
import { SidebarResizer } from "./components/SidebarResizer";
import { Workspace } from "./types/config";
import "./App.css";

type Page = { kind: "dashboard" } | { kind: "settings" } | { kind: "workspace"; name: string };

const SIDEBAR_KEY = "orbit.sidebar-width";
const DEFAULT_WIDTH = 220;
const COLLAPSED = 64;

function loadSidebarWidth(): number {
  const stored = localStorage.getItem(SIDEBAR_KEY);
  const n = stored ? Number(stored) : NaN;
  return Number.isFinite(n) && n >= 64 ? n : DEFAULT_WIDTH;
}

function App() {
  const { config, setConfig, loading, error, setError } = useConfig();
  const [page, setPage] = useState<Page>({ kind: "dashboard" });
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [sidebarWidth, setSidebarWidth] = useState(loadSidebarWidth);

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

  const onResize = useCallback((w: number) => {
    setSidebarWidth(w);
    localStorage.setItem(SIDEBAR_KEY, String(w));
  }, []);

  const collapsed = sidebarWidth <= COLLAPSED;
  const repoCount = config.services.length;
  const groupCount = Object.keys(config.groups).length;
  const activeWorkspace = page.kind === "workspace" ? workspaces.find((w) => w.name === page.name) : undefined;

  return (
    <div className="app">
      <aside className="sidebar" data-collapsed={collapsed} style={{ width: sidebarWidth }}>
        <div className="brand">
          🛰️ {collapsed ? "" : "Orbit"}
          {!collapsed && <span className="brand-sub">multi-repo workspace</span>}
        </div>

        <nav>
          {!collapsed && <div className="nav-section">Workspaces</div>}
          <button
            className={`nav-item ${page.kind === "dashboard" ? "active" : ""}`}
            onClick={() => setPage({ kind: "dashboard" })}
            title="Dashboard"
          >
            <span className="nav-icon">🏠</span> {!collapsed && "Dashboard"}
          </button>
          {!collapsed &&
            workspaces.map((ws) => (
              <button
                key={ws.name}
                className={`nav-item ${
                  page.kind === "workspace" && page.name === ws.name ? "active" : ""
                }`}
                onClick={() => setPage({ kind: "workspace", name: ws.name })}
                title={ws.name}
              >
                <span className="nav-icon">🛰️</span>
                <span className="nav-item-label">{ws.name}</span>
              </button>
            ))}

          {!collapsed && <div className="nav-section">General</div>}
          <button
            className={`nav-item ${page.kind === "settings" ? "active" : ""}`}
            onClick={() => setPage({ kind: "settings" })}
            title="Settings"
          >
            <span className="nav-icon">⚙️</span> {!collapsed && "Settings"}
            {!collapsed && repoCount + groupCount > 0 && (
              <span className="nav-count">{repoCount + groupCount}</span>
            )}
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
          <div className="page-loading">
            <span className="spinner" /> Loading…
          </div>
        ) : page.kind === "dashboard" ? (
          <DashboardPage
            config={config}
            workspaces={workspaces}
            onOpen={(ws) => setPage({ kind: "workspace", name: ws.name })}
            onChanged={loadWorkspaces}
            onError={setError}
          />
        ) : page.kind === "settings" ? (
          <SettingsPage config={config} onChange={setConfig} onError={setError} />
        ) : activeWorkspace ? (
          <WorkspaceDetailPage
            workspace={activeWorkspace}
            onBack={() => setPage({ kind: "dashboard" })}
            onRemoved={() => {
              loadWorkspaces();
              setPage({ kind: "dashboard" });
            }}
            onError={setError}
          />
        ) : (
          <div className="page-loading">
            <span className="spinner" /> Loading workspace…
          </div>
        )}
      </main>
    </div>
  );
}

export default App;