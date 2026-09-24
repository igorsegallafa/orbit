import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SkeletonTable } from "./Skeleton";

interface Check {
  name: string;
  group: "orbit" | "agent" | "build" | "language";
  required: boolean;
  ok: boolean;
  detail: string;
  usedBy: string[];
}

const GROUPS: { id: Check["group"]; title: string; empty?: string }[] = [
  { id: "orbit", title: "Orbit" },
  { id: "agent", title: "AI agent" },
  {
    id: "build",
    title: "Build tools",
    empty: "Detected from your repositories' build commands. None configured yet.",
  },
  {
    id: "language",
    title: "Language servers",
    empty: "Detected from the languages your cloned repositories use. None found yet.",
  },
];

export function HealthSection({ onError }: { onError: (msg: string) => void }) {
  const [checks, setChecks] = useState<Check[] | null>(null);

  const load = async () => {
    setChecks(null);
    try {
      setChecks(await invoke<Check[]>("health_check"));
    } catch (e) {
      onError(String(e));
      setChecks([]);
    }
  };

  useEffect(() => {
    load();
  }, []);

  const missingRequired = checks?.filter((c) => c.required && !c.ok).length ?? 0;

  return (
    <div className="section">
      <div className="section-header">
        <div>
          <h3>Environment</h3>
          {checks && (
            <p className={`health-summary ${missingRequired ? "health-summary-bad" : ""}`}>
              {missingRequired === 0
                ? "Everything Orbit needs is installed."
                : `${missingRequired} required tool${missingRequired > 1 ? "s" : ""} missing.`}
            </p>
          )}
        </div>
        <button className="secondary" onClick={load} disabled={!checks}>
          Re-check
        </button>
      </div>

      {!checks ? (
        <SkeletonTable rows={6} cols={3} />
      ) : (
        GROUPS.map((g) => {
          const rows = checks.filter((c) => c.group === g.id);
          return (
            <div key={g.id} className="health-group">
              <div className="health-group-title">{g.title}</div>
              {rows.length === 0 ? (
                <p className="field-hint">{g.empty}</p>
              ) : (
                <div className="health-list">
                  {rows.map((c) => (
                    <div key={c.name} className="health-row">
                      <span className={`health-dot ${c.ok ? "ok" : c.required ? "bad" : "warn"}`} />
                      <span className="health-name">{c.name}</span>
                      <span className={`health-detail ${c.ok ? "" : "health-detail-missing"}`} title={c.detail}>
                        {c.detail}
                      </span>
                      {c.usedBy.length > 0 && <span className="health-used">used by {c.usedBy.join(", ")}</span>}
                    </div>
                  ))}
                </div>
              )}
            </div>
          );
        })
      )}
    </div>
  );
}
