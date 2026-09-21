import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "./Toast";
import { StatusIcon, StatusKind } from "./StatusIcon";

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
    const results = await Promise.all(
      repos.map(async (repo) => {
        set(repo, { status: "pushing" });
        try {
          await invoke("ws_push", { workspace, repo });
          set(repo, { status: "pushed", detail: "Pushed" });
          return "pushed";
        } catch (e) {
          const msg = String(e);
          const skipped = msg.includes("up to date") || msg.includes("Everything up-to-date");
          set(repo, { status: skipped ? "skipped" : "error", detail: skipped ? "Already up to date" : msg });
          return skipped ? "skipped" : "error";
        }
      })
    );
    setRunning(false);
    onSettled();
    const pushed = results.filter((r) => r === "pushed").length;
    const failed = results.filter((r) => r === "error").length;
    if (failed) toast.error(`Push failed in ${failed} repo${failed === 1 ? "" : "s"}`, { description: "See the details in the dialog." });
    else if (pushed) toast.success(`Pushed ${pushed} repo${pushed === 1 ? "" : "s"}`);
  };

  const done = !running && states.every((s) => s.status !== "idle" && s.status !== "pushing");
  const pushedAny = states.some((s) => s.status === "pushed");

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Push to origin</h3>
          <p className="ws-commit-hint">Each repo pushes its feature branch and sets the upstream. Nothing is force-pushed.</p>
          <div className="ws-action-list">
            {states.map((s) => (
              <div key={s.repo} className={`status-row ${s.status === "error" ? "status-row-error" : ""}`}>
                <StatusIcon kind={PUSH_KIND[s.status]} />
                <span className="status-row-repo">{s.repo}</span>
                <span className="status-row-detail">
                  {s.status === "pushing" ? "Pushing…" : s.status === "idle" ? "Ready to push" : s.detail}
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

const PUSH_KIND: Record<"idle" | "pushing" | "pushed" | "skipped" | "error", StatusKind> = {
  idle: "pending",
  pushing: "working",
  pushed: "ok",
  skipped: "ok",
  error: "error",
};
