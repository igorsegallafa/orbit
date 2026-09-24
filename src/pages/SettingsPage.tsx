import { useState } from "react";
import { Config } from "../types/config";
import { ReposSection } from "../components/ReposSection";
import { GroupsSection } from "../components/GroupsSection";
import { AiSection } from "../components/AiSection";
import { HealthSection } from "../components/HealthSection";
import { UpdatesSection } from "../components/UpdatesSection";
import { LanguagesSection } from "../components/LanguagesSection";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

type Tab = "repos" | "groups" | "ai" | "languages" | "health" | "updates";

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
        <button
          className={`tab ${tab === "ai" ? "tab-active" : ""}`}
          onClick={() => setTab("ai")}
        >
          AI
        </button>
        <button
          className={`tab ${tab === "languages" ? "tab-active" : ""}`}
          onClick={() => setTab("languages")}
        >
          Languages
        </button>
        <button
          className={`tab ${tab === "health" ? "tab-active" : ""}`}
          onClick={() => setTab("health")}
        >
          Health
        </button>
        <button
          className={`tab ${tab === "updates" ? "tab-active" : ""}`}
          onClick={() => setTab("updates")}
        >
          Updates
        </button>
      </div>

      {tab === "repos" && <ReposSection config={config} onChange={onChange} onError={onError} />}
      {tab === "groups" && <GroupsSection config={config} onChange={onChange} onError={onError} />}
      {tab === "ai" && <AiSection onError={onError} />}
      {tab === "languages" && <LanguagesSection onError={onError} />}
      {tab === "health" && <HealthSection onError={onError} />}
      {tab === "updates" && <UpdatesSection />}
    </div>
  );
}