import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { tooltip } from "./Tooltip";
import { toast } from "./Toast";
import { StatusIcon, StatusKind } from "./StatusIcon";

interface RepoState {
  repo: string;
  status:
    | "idle"
    | "rebasing"
    | "clean"
    | "conflicts"
    | "resolving"
    | "continuing"
    | "done"
    | "error";
  conflicts: string[];
  detail?: string;
  askAi?: boolean;
}

interface Props {
  workspace: string;
  base: string;
  repos: string[];
  onClose: () => void;
  onSettled: () => void;
}

/** Rebases every repo onto origin/<base> — after the user confirms. On
 *  conflicts the row asks before letting the AI agent resolve (then
 *  `rebase --continue` runs ourselves — a half-resolution never lands).
 *  Abort per row rolls that repo back to its pre-rebase state. */
export function RebaseModal({ workspace, base, repos, onClose, onSettled }: Props) {
  const [states, setStates] = useState<RepoState[]>(() =>
    repos.map((r) => ({ repo: r, status: "idle", conflicts: [] }))
  );
  const [started, setStarted] = useState(false);

  const set = (repo: string, patch: Partial<RepoState>) =>
    setStates((prev) => prev.map((s) => (s.repo === repo ? { ...s, ...patch } : s)));

  const start = () => {
    setStarted(true);
    repos.forEach((repo) => {
      set(repo, { status: "rebasing" });
      invoke<{ clean?: unknown; conflicts?: string[] }>("ws_rebase", {
        workspace,
        repo,
        base,
      })
        .then((r) => {
          if (r.conflicts && r.conflicts.length > 0) {
            set(repo, { status: "conflicts", conflicts: r.conflicts, askAi: true });
          } else {
            set(repo, { status: "clean", detail: "rebased" });
          }
        })
        .catch((e) => set(repo, { status: "error", detail: String(e) }));
    });
  };

  const allSettled =
    started &&
    states.every((s) => ["clean", "done", "error", "conflicts"].includes(s.status));
  const anyWorking = states.some((s) =>
    ["rebasing", "resolving", "continuing"].includes(s.status)
  );

  const resolveWithAi = (repo: string) => {
    set(repo, { status: "resolving", askAi: false });
    invoke<string>("ws_resolve_conflicts", { workspace, repo })
      .then((summary) => {
        // Agent staged the resolutions — we run the continue.
        set(repo, { status: "continuing", detail: summary });
        return invoke("ws_rebase_continue", { workspace, repo });
      })
      .then(() => set(repo, { status: "done", detail: "conflicts resolved by AI" }))
      .catch((e) => {
        // Agent failed or continue refused: roll back so the repo is never
        // left mid-rebase.
        invoke("ws_rebase_abort", { workspace, repo })
          .catch(() => null)
          .finally(() => set(repo, { status: "error", detail: String(e) }));
      });
  };

  const abort = (repo: string) => {
    invoke("ws_rebase_abort", { workspace, repo })
      .then(() => set(repo, { status: "error", detail: "aborted — back to pre-rebase state" }))
      .catch((e) => set(repo, { status: "error", detail: String(e) }));
  };

  // When everything settles clean/done, tell the parent (refresh statuses).
  useEffect(() => {
    if (!allSettled || states.length === 0) return;
    onSettled();
    const conflicts = states.filter((s) => s.status === "conflicts").length;
    const errors = states.filter((s) => s.status === "error").length;
    if (conflicts) toast.info(`${conflicts} repo${conflicts === 1 ? "" : "s"} paused with conflicts`, { description: "Resolve them with AI or abort in the dialog." });
    else if (errors) toast.error(`Rebase failed in ${errors} repo${errors === 1 ? "" : "s"}`);
    else toast.success(`Rebased onto origin/${base}`);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [allSettled]);

  return (
    <div className="modal-overlay" onMouseDown={anyWorking ? undefined : onClose}>
      <div className="modal ws-action-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Rebase onto origin/{base}</h3>
          {!started && (
            <p className="ws-commit-hint">
              Every repo's feature branch will be rebased onto{" "}
              <span className="mono">origin/{base}</span> (fetch first). Conflicts pause that
              repo for review — nothing is force-pushed.
            </p>
          )}
          <div className="ws-action-list">
            {states.map((s) => (
              <div key={s.repo} className={`status-row ${s.status === "error" ? "status-row-error" : ""}`}>
                <StatusIcon kind={!started ? "pending" : REBASE_KIND[s.status]} />
                <span className="status-row-repo">{s.repo}</span>
                <span className="status-row-detail">
                  {!started
                    ? `onto origin/${base}`
                    : s.status === "idle" || s.status === "rebasing"
                      ? "rebasing…"
                      : s.status === "resolving"
                        ? "AI is resolving conflicts…"
                        : s.status === "continuing"
                          ? "finishing rebase…"
                          : s.status === "conflicts"
                            ? `${s.conflicts.length} conflicted file(s): ${s.conflicts.join(", ")}`
                            : (s.detail ?? "done")}
                </span>
                <span className="status-row-actions">
                  {s.status === "conflicts" && s.askAi && (
                    <>
                      <button
                        className="btn-mini"
                        onClick={() => resolveWithAi(s.repo)}
                        onMouseEnter={(e) =>
                          tooltip.show("AI resolves and stages; continue runs here", e)
                        }
                        onMouseLeave={() => tooltip.hide()}
                      >
                        Resolve with AI
                      </button>
                      <button className="btn-mini" onClick={() => abort(s.repo)}>
                        Abort
                      </button>
                    </>
                  )}
                </span>
              </div>
            ))}
          </div>
        </div>
        <div className="modal-footer">
          {!started ? (
            <>
              <button type="button" className="secondary" onClick={onClose}>
                Cancel
              </button>
              <button type="button" autoFocus onClick={start}>
                Rebase {repos.length} {repos.length === 1 ? "repo" : "repos"}
              </button>
            </>
          ) : (
            <button type="button" className="secondary" onClick={onClose} disabled={anyWorking}>
              Close
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

const REBASE_KIND: Record<string, StatusKind> = {
  idle: "working",
  rebasing: "working",
  resolving: "working",
  continuing: "working",
  clean: "ok",
  done: "ok",
  conflicts: "warn",
  error: "error",
};
