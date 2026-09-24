import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "./Toast";
import { BranchIcon } from "./Icons";
import { BranchSync, RepoStatus } from "../types/config";

interface Props {
  workspace: string;
  statuses: RepoStatus[] | null;
  /** After a switch: reload the statuses. */
  onSynced: () => void;
}

/** Warns about repos that aren't on the workspace's branch — a branch-only
 *  repo's clone is shared, so creating or opening another workspace
 *  switches it — and switches them back. Uncommitted work is stashed only
 *  after asking; Orbit re-applies it when that branch is switched to again.
 *  Repos set to auto-switch go back on their own when the page opens. */
export function BranchDriftBanner({ workspace, statuses, onSynced }: Props) {
  const [busy, setBusy] = useState(false);
  // Repos a switch left alone because of uncommitted work: ask to stash.
  const [dirty, setDirty] = useState<RepoStatus[]>([]);
  const autoDone = useRef<string | null>(null);

  const off = (statuses ?? []).filter((s) => s.offBranch);
  const switchable = off.filter((s) => !s.operation);

  const sync = async (repos: RepoStatus[], stash: boolean, auto = false) => {
    setBusy(true);
    const left: RepoStatus[] = [];
    const switched: string[] = [];
    const notes: string[] = [];
    for (const s of repos) {
      try {
        const r = await invoke<BranchSync>("ws_sync_branch", { workspace, repo: s.repo, stash });
        if (r.dirty) left.push(s);
        else {
          switched.push(r.stashed ? `${s.repo} (changes stashed)` : r.restored ? `${s.repo} (stashed changes restored)` : s.repo);
          if (r.note) notes.push(r.note);
        }
      } catch (e) {
        notes.push(String(e));
      }
    }
    setBusy(false);
    setDirty(auto ? [] : left);
    if (switched.length) {
      const what = auto ? "Switched automatically" : "Switched";
      toast.success(`${what}: ${switched.join(", ")}`, { description: notes.join("\n") || undefined });
    } else if (notes.length) {
      toast.error("Couldn't switch branches", { description: notes.join("\n") });
    }
    if (switched.length || notes.length) onSynced();
  };

  // Auto-switch once per workspace visit, only where nothing is in the way.
  useEffect(() => {
    if (!statuses || autoDone.current === workspace) return;
    autoDone.current = workspace;
    const auto = statuses.filter((s) => s.offBranch && s.autoSwitch && !s.dirty && !s.operation);
    if (auto.length) sync(auto, false, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [statuses, workspace]);

  useEffect(() => setDirty([]), [workspace]);

  if (!off.length) return null;

  const pending = dirty.filter((d) => off.some((s) => s.repo === d.repo));
  return (
    <div className="drift-banner">
      <div className="drift-head">
        <BranchIcon size={13} />
        <span>
          {off.length === 1 ? "A repository isn't" : `${off.length} repositories aren't`} on this workspace's branch. Commit, push, rebase
          and PRs are blocked for {off.length === 1 ? "it" : "them"} until switched back.
        </span>
        {switchable.length > 0 && pending.length === 0 && (
          <button className="btn-mini" disabled={busy} onClick={() => sync(switchable, false)}>
            {busy ? "Switching…" : switchable.length === 1 ? `Switch to ${switchable[0].expectedBranch}` : `Switch ${switchable.length} repos back`}
          </button>
        )}
      </div>
      <ul className="drift-rows">
        {off.map((s) => (
          <li key={s.repo}>
            <strong>{s.repo}</strong> is on <code>{s.branch ?? "a detached HEAD"}</code>
            {s.heldBy && <> (workspace {s.heldBy})</>}, expected <code>{s.expectedBranch}</code>
            {s.operation && <span className="drift-blocked"> · a {s.operation} is in progress; finish or abort it first</span>}
          </li>
        ))}
      </ul>
      {pending.length > 0 && (
        <div className="drift-confirm">
          <span>
            {pending.map((d) => d.repo).join(", ")} {pending.length === 1 ? "has" : "have"} uncommitted changes
            {pending.length === 1 && pending[0].branch ? <> on <code>{pending[0].branch}</code></> : null}. Stash them and switch? They come
            back when Orbit switches to that branch again.
          </span>
          <button className="btn-mini" disabled={busy} onClick={() => setDirty([])}>
            Cancel
          </button>
          <button className="btn-mini" disabled={busy} onClick={() => sync(pending, true)}>
            {busy ? "Switching…" : "Stash and switch"}
          </button>
        </div>
      )}
    </div>
  );
}
