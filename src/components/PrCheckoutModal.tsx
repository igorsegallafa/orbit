import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PullRequest, Workspace } from "../types/config";
import { StatusIcon } from "./StatusIcon";
import { toast } from "./Toast";

interface Plan {
  repo: string;
  known: boolean;
  cloned: boolean;
  onOrigin: boolean;
  branchOnly: boolean;
  dirty: boolean;
}

interface Props {
  /** The PRs of one feature (same branch, one per repo). */
  prs: PullRequest[];
  workspaces: Workspace[];
  onCheckedOut: (ws: Workspace) => void;
  onClose: () => void;
}

/** "feat/refactor-x" → "refactor-x", made unique among existing workspaces. */
function suggestName(branch: string, taken: Set<string>): string {
  const base =
    (branch.split("/").pop() ?? branch)
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "") || "review";
  if (!taken.has(base)) return base;
  for (let i = 2; ; i++) if (!taken.has(`${base}-${i}`)) return `${base}-${i}`;
}

/** The workspace that already has these PRs checked out, if any. */
export function workspaceForPrs(prs: PullRequest[], workspaces: Workspace[]): Workspace | null {
  return (
    workspaces.find((w) =>
      prs.some(
        (p) => w.repos.includes(p.repo) && (w.branch === p.branch || w.prRefs?.some((r) => r.repo === p.repo && r.number === p.number)),
      ),
    ) ?? null
  );
}

/**
 * Check out a PR (or a multi-repo feature) as a workspace, to run it
 * locally: one worktree per repo on the PR's branch, tracking origin so
 * fixes can be pushed back to the PR.
 */
export function PrCheckoutModal({ prs, workspaces, onCheckedOut, onClose }: Props) {
  const first = prs[0];
  const [name, setName] = useState(() => suggestName(first.branch, new Set(workspaces.map((w) => w.name))));
  const [plan, setPlan] = useState<Plan[] | null>(null);
  const [running, setRunning] = useState(false);
  const specs = prs.map((p) => ({ repo: p.repo, number: p.number, branch: p.branch, url: p.url }));

  useEffect(() => {
    invoke<Plan[]>("pr_checkout_plan", { prs: specs })
      .then(setPlan)
      .catch(() => setPlan([]));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const blocked = (p: Plan) => !p.known || (p.branchOnly && p.dirty);
  const usable = plan ? plan.filter((p) => !blocked(p)).length : 0;
  const nameTaken = workspaces.some((w) => w.name === name.trim());
  const canCheckout = !!plan && usable > 0 && !!name.trim() && !nameTaken && !running;

  const checkout = async () => {
    setRunning(true);
    const t = toast.loading(`Checking out ${first.branch}…`, { description: `Workspace ${name.trim()}` });
    onClose();
    try {
      const res = await invoke<{ workspace: Workspace; failures: string[] }>("pr_checkout", { name: name.trim(), base: first.base, prs: specs });
      if (res.failures.length) toast.update(t, "info", `Workspace ${res.workspace.name} is ready`, { description: res.failures.join("\n") });
      else toast.update(t, "success", `Workspace ${res.workspace.name} is ready`, { description: `${first.branch} in ${res.workspace.repos.join(", ")}` });
      onCheckedOut(res.workspace);
    } catch (e) {
      toast.update(t, "error", "Check out failed", { description: String(e) });
    }
  };

  const rowState = (p: Plan | undefined) => {
    if (!p) return { kind: "pending" as const, text: "Checking…" };
    if (!p.known) return { kind: "error" as const, text: "Not a repository in Orbit (add it in Settings)" };
    if (p.branchOnly && p.dirty) return { kind: "error" as const, text: "Branch-only repo with uncommitted changes in its clone" };
    if (!p.onOrigin) return { kind: "warn" as const, text: "From a fork: read-only branch, pushes go nowhere" };
    if (!p.cloned) return { kind: "pending" as const, text: "Will be cloned first" };
    return { kind: "ok" as const, text: p.branchOnly ? "Checked out in its clone (branch-only)" : "New worktree, tracking the PR branch" };
  };

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Check out {prs.length > 1 ? `${prs.length} pull requests` : `#${first.number}`}</h3>
          <p className="ws-commit-hint">
            A workspace on <strong className="mono">{first.branch}</strong> (based on {first.base}) to run and try the changes.
            Commits you push go to the pull request.
          </p>
          <label className="field">
            <span className="field-label">Workspace name</span>
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && canCheckout && checkout()}
            />
            {nameTaken && <span className="field-hint pr-checkout-taken">A workspace with this name already exists.</span>}
          </label>
          <div className="pr-checkout-rows">
            {prs.map((pr) => {
              const s = rowState(plan?.find((p) => p.repo === pr.repo));
              return (
                <div key={pr.repo + pr.number} className={`status-row ${s.kind === "error" ? "status-row-error" : ""}`}>
                  <StatusIcon kind={s.kind} />
                  <span className="status-row-repo">
                    {pr.repo} #{pr.number}
                  </span>
                  <span className="status-row-detail">{s.text}</span>
                </div>
              );
            })}
          </div>
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button type="button" disabled={!canCheckout} onClick={checkout}>
            Check out
          </button>
        </div>
      </div>
    </div>
  );
}
