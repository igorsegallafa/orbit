import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "./Toast";
import { tooltip } from "./Tooltip";
import { StatusIcon, StatusKind } from "./StatusIcon";

type PushStatus = "idle" | "pushing" | "pushed" | "skipped" | "rejected" | "error";

interface RepoState {
  repo: string;
  status: PushStatus;
  detail?: string;
  /** Left out on purpose (its PR was closed): pushed only on request. */
  dropped?: boolean;
}

interface Props {
  workspace: string;
  repos: string[];
  /** Repos not pushed unless asked, with why (e.g. their PR was closed). */
  skipped?: { repo: string; reason: string }[];
  /** After a successful push: continue to the PR-creation modal. */
  onCreatePrs: () => void;
  onClose: () => void;
  onSettled: () => void;
}

/** Push confirmation modal: nothing runs until "Push" is pressed. When
 *  done, offers the pull-request flow (the modal that drafts/edits
 *  title + description and creates the PRs). A branch rewritten by a
 *  rebase is rejected as non-fast-forward; that row then offers a
 *  force push with lease — never done without the click. */
export function PushModal({ workspace, repos, skipped = [], onCreatePrs, onClose, onSettled }: Props) {
  const [states, setStates] = useState<RepoState[]>(() => [
    ...repos.map((r): RepoState => ({ repo: r, status: "idle" })),
    ...skipped.map((s): RepoState => ({ repo: s.repo, status: "skipped", detail: s.reason, dropped: true })),
  ]);
  const [running, setRunning] = useState(false);

  const set = (repo: string, patch: Partial<RepoState>) =>
    setStates((prev) => prev.map((s) => (s.repo === repo ? { ...s, ...patch } : s)));

  const pushOne = async (repo: string, force: boolean): Promise<PushStatus> => {
    set(repo, { status: "pushing", dropped: false });
    try {
      await invoke("ws_push", { workspace, repo, force });
      set(repo, { status: "pushed", detail: force ? "Force-pushed" : "Pushed" });
      return "pushed";
    } catch (e) {
      const msg = String(e);
      if (msg.includes("up to date") || msg.includes("Everything up-to-date")) {
        set(repo, { status: "skipped", detail: "Already up to date" });
        return "skipped";
      }
      if (!force && (msg.includes("non-fast-forward") || msg.includes("fetch first"))) {
        set(repo, { status: "rejected", detail: "Branch diverged from origin (rebased?)" });
        return "rejected";
      }
      set(repo, { status: "error", detail: msg });
      return "error";
    }
  };

  const push = async () => {
    setRunning(true);
    const results = await Promise.all(repos.map((repo) => pushOne(repo, false)));
    setRunning(false);
    onSettled();
    const pushed = results.filter((r) => r === "pushed").length;
    const failed = results.filter((r) => r === "error").length;
    const rejected = results.filter((r) => r === "rejected").length;
    if (failed) toast.error(`Push failed in ${failed} repo${failed === 1 ? "" : "s"}`, { description: "See the details in the dialog." });
    else if (rejected) toast.error(`Push rejected in ${rejected} repo${rejected === 1 ? "" : "s"}`, { description: "The branch was rewritten; force push it from the dialog." });
    else if (pushed) toast.success(`Pushed ${pushed} repo${pushed === 1 ? "" : "s"}`);
  };

  // A dropped repo, on explicit request: recreates its branch on origin.
  const pushAnyway = async (repo: string) => {
    tooltip.hide();
    setRunning(true);
    const r = await pushOne(repo, false);
    setRunning(false);
    onSettled();
    if (r === "pushed") toast.success(`Pushed ${repo}`);
  };

  const forcePush = async (repo: string) => {
    tooltip.hide();
    setRunning(true);
    const r = await pushOne(repo, true);
    setRunning(false);
    onSettled();
    if (r === "pushed") toast.success(`Force-pushed ${repo}`);
    else if (r === "error") toast.error(`Force push failed in ${repo}`, { description: "See the details in the dialog." });
  };

  const done = !running && states.every((s) => s.status !== "idle" && s.status !== "pushing");
  const toPush = states.filter((s) => s.status === "idle").length;
  const pushedAny = states.some((s) => s.status === "pushed");

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Push to origin</h3>
          <p className="ws-commit-hint">Each repo pushes its feature branch and sets the upstream. Nothing is force-pushed unless you ask for it.</p>
          <div className="ws-action-list">
            {states.map((s) => (
              <div key={s.repo} className={`status-row ${s.status === "error" ? "status-row-error" : ""}`}>
                <StatusIcon kind={PUSH_KIND[s.status]} />
                <span className="status-row-repo">{s.repo}</span>
                <span className="status-row-detail">
                  {s.status === "pushing" ? "Pushing…" : s.status === "idle" ? "Ready to push" : s.detail}
                </span>
                {s.dropped && (
                  <span className="status-row-actions">
                    <button
                      className="btn-mini"
                      disabled={running}
                      onClick={() => pushAnyway(s.repo)}
                      onMouseEnter={(e) => tooltip.show("Pushes the branch again, recreating it on origin if it was deleted", e)}
                      onMouseLeave={() => tooltip.hide()}
                    >
                      Push anyway
                    </button>
                  </span>
                )}
                {s.status === "rejected" && (
                  <span className="status-row-actions">
                    <button
                      className="btn-mini"
                      disabled={running}
                      onClick={() => forcePush(s.repo)}
                      onMouseEnter={(e) =>
                        tooltip.show("git push --force-with-lease: overwrites origin's branch, but refuses if someone else pushed to it since your last fetch", e)
                      }
                      onMouseLeave={() => tooltip.hide()}
                    >
                      Force push
                    </button>
                  </span>
                )}
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
              <button type="button" autoFocus onClick={push} disabled={running || toPush === 0}>
                {running ? "Pushing…" : `Push ${toPush} ${toPush === 1 ? "repo" : "repos"}`}
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

const PUSH_KIND: Record<PushStatus, StatusKind> = {
  idle: "pending",
  pushing: "working",
  pushed: "ok",
  skipped: "ok",
  rejected: "warn",
  error: "error",
};
