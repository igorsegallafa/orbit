import { useState } from "react";
import { useConfig } from "./hooks/useConfig";
import { ReposPage } from "./pages/ReposPage";
import { GroupsPage } from "./pages/GroupsPage";
import "./App.css";

type Tab = "repos" | "groups";

function App() {
  const { config, setConfig, loading, error, setError } = useConfig();
  const [tab, setTab] = useState<Tab>("repos");

  return (
    <div className="app">
      <aside className="sidebar">
        <div className="brand">🛰️ Orbit</div>
        <nav>
          <button className={tab === "repos" ? "active" : ""} onClick={() => setTab("repos")}>
            Repositories
          </button>
          <button className={tab === "groups" ? "active" : ""} onClick={() => setTab("groups")}>
            Groups
          </button>
        </nav>
      </aside>

      <main className="content">
        {error && (
          <div className="banner error" onClick={() => setError(null)}>
            {error}
          </div>
        )}
        {loading ? (
          <p className="empty">Loading…</p>
        ) : tab === "repos" ? (
          <ReposPage config={config} onChange={setConfig} onError={setError} />
        ) : (
          <GroupsPage config={config} onChange={setConfig} onError={setError} />
        )}
      </main>
    </div>
  );
}

export default App;
