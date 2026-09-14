import { useCallback, useState } from "react";
import { useConfig } from "./hooks/useConfig";
import { DashboardPage } from "./pages/DashboardPage";
import { SettingsPage } from "./pages/SettingsPage";
import { SidebarResizer } from "./components/SidebarResizer";
import "./App.css";

type Page = "dashboard" | "settings";

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
  const [page, setPage] = useState<Page>("dashboard");
  const [sidebarWidth, setSidebarWidth] = useState(loadSidebarWidth);

  const onResize = useCallback((w: number) => {
    setSidebarWidth(w);
    localStorage.setItem(SIDEBAR_KEY, String(w));
  }, []);

  const collapsed = sidebarWidth <= COLLAPSED;
  const repoCount = config.services.length;
  const groupCount = Object.keys(config.groups).length;

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
            className={`nav-item ${page === "dashboard" ? "active" : ""}`}
            onClick={() => setPage("dashboard")}
            title="Dashboard"
          >
            <span className="nav-icon">🏠</span> {!collapsed && "Dashboard"}
          </button>

          {!collapsed && <div className="nav-section">General</div>}
          <button
            className={`nav-item ${page === "settings" ? "active" : ""}`}
            onClick={() => setPage("settings")}
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
        ) : page === "dashboard" ? (
          <DashboardPage config={config} />
        ) : (
          <SettingsPage config={config} onChange={setConfig} onError={setError} />
        )}
      </main>
    </div>
  );
}

export default App;