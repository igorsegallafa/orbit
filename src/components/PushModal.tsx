import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface RepoState {
  repo: string;
  status: "idle" | "pushing" | "pushed" | "skipped" | "error";
  detail?: string;
}

interface Props {
  workspace: string;
  repos: string[];
  /** After a successful push: continue to the PR-creation modal. */
  onCreatePrs: () => void;
  onClose: () => void;
  onSettled: () => void;
}

/** Push confirmation modal: nothing runs until "Push" is pressed. When
 *  done, offers the pull-request flow (the modal that drafts/edits
 *  title + description and creates the PRs). */
export function PushModal({ workspace, repos, onCreatePrs, onClose, onSettled }: Props) {
  const [states, setStates] = useState<RepoState[]>(() =>
    repos.map((r) => ({ repo: r, status: "idle" }))
  );
  const [running, setRunning] = useState(false);

  const set = (repo: string, patch: Partial<RepoState>) =>
    setStates((prev) => prev.map((s) => (s.repo === repo ? { ...s, ...patch } : s)));

  const push = async () => {
    setRunning(true);
    await Promise.all(
      repos.map(async (repo) => {
        set(repo, { status: "pushing" });
        try {
          await invoke("ws_push", { workspace, repo });
          set(repo, { status: "pushed", detail: "pushed" });
        } catch (e) {
          const msg = String(e);
          set(repo, {
            status: msg.includes("up to date") || msg.includes("Everything up-to-date") ? "skipped" : "error",
            detail: msg,
          });
        }
      })
    );
    setRunning(false);
    onSettled();
  };

  const done = !running && states.every((s) => s.status !== "idle" && s.status !== "pushing");
  const pushedAny = states.some((s) => s.status === "pushed");

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Push branches to origin</h3>
          <div className="ws-action-list">
            {states.map((s) => (
              <div key={s.repo} className={`ws-action-row ws-${s.status === "pushed" || s.status === "skipped" ? "ok" : s.status}`}>
                <span className="ws-action-repo mono">{s.repo}</span>
                <span className="ws-action-detail">
                  {s.status === "pushing"
                    ? "pushing…"
                    : s.status === "idle"
                      ? ""
                      : (s.detail ?? "nothing to push")}
                </span>
                <span className="ws-action-state">
                  {s.status === "pushing" && <span className="spinner" />}
                  {s.status === "pushed" && <span className="ws-dot ws-dot-ok">✓</span>}
                  {s.status === "error" && <span className="ws-dot ws-dot-err">✗</span>}
                </span>
              </div>
            ))}
          </div>
        </div>
        <div className="modal-footer">
          {!done ? (
            <>
              <button type="button" className="secondary" onClick={onClose} disabled={running}>
                Cancel
              </button>
              <button type="button" autoFocus onClick={push} disabled={running}>
                {running ? "Pushing…" : `Push ${repos.length} ${repos.length === 1 ? "repo" : "repos"}`}
              </button>
            </>
          ) : (
            <>
              <button type="button" className="secondary" onClick={onClose}>
                Close
              </button>
              {pushedAny && (
                <button type="button" autoFocus onClick={onCreatePrs}>
                  Create pull requests →
                </button>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
