import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { checkForUpdates, installUpdate, useUpdater } from "../lib/updater";

/** Settings → Updates: the installed version, the latest check, and the
 *  buttons to check now or install a pending update. */
export function UpdatesSection() {
  const state = useUpdater();
  const [version, setVersion] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(null));
  }, []);

  const busy = state.status === "checking" || state.status === "downloading";

  return (
    <div className="section">
      <div className="settings-card">
        <div className="settings-row">
          <div className="settings-row-text">
            <strong>Version</strong>
            <span>Orbit checks GitHub for a new release at startup and every hour while it's open.</span>
          </div>
          <div className="settings-row-control settings-row-inline">
            <code>{version ?? "…"}</code>
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-text">
            <strong>Updates</strong>
            <span>{describe(state)}</span>
            {state.status === "error" && <code className="settings-error">{state.message}</code>}
          </div>
          <div className="settings-row-control settings-row-inline">
            {state.status === "available" || state.status === "downloading" ? (
              <button onClick={() => void installUpdate()} disabled={busy}>
                {state.status === "downloading" ? (
                  <>
                    <span className="spinner" /> Installing…
                  </>
                ) : (
                  "Install and restart"
                )}
              </button>
            ) : (
              <button className="secondary" onClick={() => void checkForUpdates({ manual: true })} disabled={busy}>
                {state.status === "checking" ? (
                  <>
                    <span className="spinner" /> Checking…
                  </>
                ) : (
                  "Check for updates"
                )}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

function describe(s: ReturnType<typeof useUpdater>): string {
  switch (s.status) {
    case "idle":
      return "Not checked yet.";
    case "checking":
      return "Checking for a new release…";
    case "up-to-date":
      return `You're on the latest version (checked ${new Date(s.checkedAt).toLocaleTimeString()}).`;
    case "available":
      return `Orbit ${s.version} is available.`;
    case "downloading":
      return `Downloading Orbit ${s.version}${s.percent !== null ? ` · ${s.percent}%` : ""}… Orbit restarts when it's done.`;
    case "error":
      return "The last check failed.";
  }
}
