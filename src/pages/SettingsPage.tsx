import { useState } from "react";
import { Config } from "../types/config";
import { ReposSection } from "../components/ReposSection";
import { GroupsSection } from "../components/GroupsSection";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

type Tab = "repos" | "groups";

export function SettingsPage({ config, onChange, onError }: Props) {
  const [tab, setTab] = useState<Tab>("repos");

  return (
    <div className="page">
      <div className="page-header">
        <h2>Settings</h2>
      </div>

      <div className="tabs">
        <button
          className={`tab ${tab === "repos" ? "tab-active" : ""}`}
          onClick={() => setTab("repos")}
        >
          Repositories
        </button>
        <button
          className={`tab ${tab === "groups" ? "tab-active" : ""}`}
          onClick={() => setTab("groups")}
        >
          Groups
        </button>
      </div>

      {tab === "repos" ? (
        <ReposSection config={config} onChange={onChange} onError={onError} />
      ) : (
        <GroupsSection config={config} onChange={onChange} onError={onError} />
      )}
    </div>
  );
}