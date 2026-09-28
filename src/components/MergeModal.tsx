import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "./Toast";
import { StatusIcon, StatusKind } from "./StatusIcon";

/** A PR to merge: a workspace's (found through its worktree) or, with
 *  `ownerRepo`, one addressed on GitHub directly (not checked out). */
export interface MergeItem {
  repo: string;
  number: number;
  title: string;
  ownerRepo?: string;
}

type MergeStatus = "idle" | "merging" | "merged" | "error";

interface Props {
  /** Omitted for PRs that aren't checked out (each carries `ownerRepo`). */
  workspace?: string;
  base: string;
  prs: MergeItem[];
  onClose: () => void;
  /** After the run, with the PRs that actually merged (it stops at the first failure). */
  onSettled: (merged: MergeItem[]) => void;
}

/** Squash-and-merge confirmation: nothing runs until "Merge" is pressed.
 *  PRs merge one at a time so a failure (review required, conflicts) stops
 *  the rest instead of leaving a feature half-merged. */
export function MergeModal({ workspace, base, prs, onClose, onSettled }: Props) {
  const [states, setStates] = useState<Record<string, { status: MergeStatus; detail?: string }>>({});
  const [running, setRunning] = useState(false);
  const key = (pr: MergeItem) => `${pr.repo}/${pr.number}`;
  const set = (pr: MergeItem, status: MergeStatus, detail?: string) =>
    setStates((prev) => ({ ...prev, [key(pr)]: { status, detail } }));

  const merge = async () => {
    setRunning(true);
    const landed: MergeItem[] = [];
    for (const pr of prs) {
      set(pr, "merging");
      try {
        await (pr.ownerRepo
          ? invoke("pr_merge_remote", { ownerRepo: pr.ownerRepo, number: pr.number })
          : invoke("ws_pr_merge", { workspace, repo: pr.repo, number: pr.number }));
        set(pr, "merged", `Squashed into ${base}`);
        landed.push(pr);
      } catch (e) {
        set(pr, "error", String(e));
        toast.error(`Merge stopped at ${pr.repo} #${pr.number}`, { description: "See the details in the dialog." });
        break;
      }
    }
    setRunning(false);
    onSettled(landed);
    if (landed.length === prs.length) toast.success(`Merged ${landed.length} pull request${landed.length === 1 ? "" : "s"} into ${base}`);
  };

  const started = Object.keys(states).length > 0;
  const done = started && !running;

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Squash and merge</h3>
          <p className="ws-commit-hint">
            Each PR lands in <strong>{base}</strong> as a single commit titled after the PR, and its branch is deleted on GitHub.
            {workspace ? " The workspace stays until you remove it." : ""}
          </p>
          <div className="ws-action-list">
            {prs.map((pr) => {
              const s = states[key(pr)] ?? { status: "idle" as MergeStatus };
              return (
                <div key={key(pr)} className={`status-row ${s.status === "error" ? "status-row-error" : ""}`}>
                  <StatusIcon kind={MERGE_KIND[s.status]} />
                  <span className="status-row-repo">
                    {pr.repo} #{pr.number}
                  </span>
                  <span className="status-row-detail">
                    {s.status === "merging" ? "Merging…" : s.status === "idle" ? pr.title : s.detail}
                  </span>
                </div>
              );
            })}
          </div>
        </div>
        <div className="modal-footer">
          {done ? (
            <button type="button" className="secondary" onClick={onClose}>
              Close
            </button>
          ) : (
            <>
              <button type="button" className="secondary" onClick={onClose} disabled={running}>
                Cancel
              </button>
              <button type="button" autoFocus onClick={merge} disabled={running}>
                {running ? "Merging…" : `Merge ${prs.length} PR${prs.length === 1 ? "" : "s"}`}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

const MERGE_KIND: Record<MergeStatus, StatusKind> = {
  idle: "pending",
  merging: "working",
  merged: "ok",
  error: "error",
};
