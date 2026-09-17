import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PullRequest, RepoStatus, Workspace, WsPrStatus } from "../types/config";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { SkeletonTable, Skeleton } from "../components/Skeleton";
import { PlanModal } from "../components/PlanModal";
import { GrillModal } from "../components/GrillModal";
import { PlanProgress } from "../components/PlanProgress";
import { CommitModal } from "../components/CommitModal";
import { RebaseModal } from "../components/RebaseModal";
import { PushModal } from "../components/PushModal";
import { PrsModal } from "../components/PrsModal";
import { tooltip } from "../components/Tooltip";
import { RebaseIcon, CommitIcon, PushIcon, PullRequestIcon } from "../components/Icons";

interface Props {
  workspace: Workspace;
  onOpenEditor: (repo: string) => void;
  onOpenPlan: () => void;
  onRemoved: () => void;
  onError: (msg: string) => void;
  /** Opens a PR review tab for an explicit PR list. */
  onOpenPrList: (prs: PullRequest[]) => void;
}

export function WorkspaceDetailPage({ workspace, onOpenEditor, onOpenPlan, onRemoved, onError, onOpenPrList }: Props) {
  const [statuses, setStatuses] = useState<RepoStatus[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<null | "normal" | "force">(null);
  const [planOpen, setPlanOpen] = useState(false);
  const [grillOpen, setGrillOpen] = useState(false);
  const [planExists, setPlanExists] = useState<boolean | null>(null);
  const [pushOpen, setPushOpen] = useState(false);
  const [commitOpen, setCommitOpen] = useState(false);
  const [rebaseOpen, setRebaseOpen] = useState(false);
  const [prCreateOpen, setPrCreateOpen] = useState(false);
  const [prsOpen, setPrsOpen] = useState(false);
  const [prStatuses, setPrStatuses] = useState<WsPrStatus[] | null>(null);
  const [prsLoading, setPrsLoading] = useState(false);

  // "Pull requests" toggle: expands the tracker section below the
  // pipeline bar and (re)loads the branch's PR status per repo.
  const loadPrs = () => {
    if (prsOpen) {
      setPrsOpen(false);
      return;
    }
    setPrsLoading(true);
    setPrsOpen(true);
    invoke<WsPrStatus[]>("ws_pr_status", { workspace: workspace.name })
      .then((rows) => setPrStatuses(rows))
      .catch((e) => {
        setPrStatuses([]);
        onError(String(e));
      })
      .finally(() => setPrsLoading(false));
  };

  // Does a PLAN.md already exist for this workspace?
  useEffect(() => {
    let cancelled = false;
    invoke<boolean>("workspace_plan_exists", { name: workspace.name })
      .then((v) => !cancelled && setPlanExists(v))
      .catch(() => !cancelled && setPlanExists(false));
    return () => {
      cancelled = true;
    };
  }, [workspace.name]);

  const load = useCallback(
    async (fetch: boolean) => {
      setRefreshing(true);
      try {
        if (fetch) {
          await Promise.all(
            workspace.repos.map((repo) => invoke("refresh_repo", { name: repo }).catch(() => null))
          );
        }
        const st = await invoke<RepoStatus[]>("workspace_status", { name: workspace.name });
        setStatuses(st);
      } catch (e) {
        onError(String(e));
      } finally {
        setRefreshing(false);
      }
    },
    [workspace.name, workspace.repos, onError]
  );

  useEffect(() => {
    setStatuses(null);
    load(false);
  }, [load]);

  const dirtyCount = (statuses ?? []).filter((s) => s.dirty).length;
  const aheadCount = (statuses ?? []).reduce((a, s) => a + s.ahead, 0);
  // Branches on origin with PR-worthy content but no PR — the resume
  // point of an interrupted "Create pull requests" flow.
  const pushedNoPr =
    prStatuses !== null &&
    prStatuses.length === 0 &&
    (statuses ?? []).some((s) => s.prCommits > 0);

  const remove = async (force: boolean) => {
    try {
      await invoke("remove_workspace", { name: workspace.name, force });
      setConfirmRemove(null);
      onRemoved();
    } catch (e) {
      const msg = String(e);
      if (!force && msg.includes("uncommitted changes")) {
        setConfirmRemove("force");
      } else {
        onError(msg);
        setConfirmRemove(null);
      }
    }
  };

  return (
    <div className="page ws-detail">
      <header className="detail-header">
        <div className="detail-title-row">
          <h2>{workspace.name}</h2>
          <div className="detail-actions">
            {workspace.card && (
              <>
                <button
                  className="secondary"
                  title={planExists ? "Open the generated PLAN.md" : "Generate a PLAN.md from the linked card"}
                  onClick={() => (planExists ? onOpenPlan() : setPlanOpen(true))}
                >
                  {planExists ? "Open plan" : "✳ Plan"}
                </button>
                {planExists && (
                  <button
                    className="secondary"
                    title="Discard the current PLAN.md and generate a new one"
                    onClick={() => setPlanOpen(true)}
                  >
                    ↻ Replan
                  </button>
                )}
              </>
            )}
            <button className="secondary" disabled={refreshing} onClick={() => load(true)}>
              {refreshing ? "Refreshing…" : "↻ Refresh"}
            </button>
            <button className="secondary danger-outline" onClick={() => setConfirmRemove("normal")}>
              Remove
            </button>
          </div>
        </div>
        <div className="workspace-meta">
          <span className="tag">{workspace.branch}</span>
          <span className="tag tag-muted">base: {workspace.base}</span>
          <span className="tag tag-muted">{workspace.repos.length} repos</span>
          {workspace.card && (
            <a
              className="tag tag-info plan-card-tag"
              href={workspace.card.url}
              title={workspace.card.title}
              onClick={(e) => {
                e.preventDefault();
                import("@tauri-apps/plugin-opener")
                  .then(({ openUrl }) => openUrl(workspace.card!.url))
                  .catch(() => null);
              }}
            >
              {workspace.card.id}
            </a>
          )}
        </div>
      </header>

      <div className="ws-pipeline">
        <div className="segmented">
          <button
            className="pipe-btn"
            disabled={refreshing}
            onMouseEnter={(e) => tooltip.show(`Rebase every repo onto origin/${workspace.base}`, e)}
            onMouseLeave={() => tooltip.hide()}
            onClick={() => setRebaseOpen(true)}
          >
            <RebaseIcon size={13} /> Rebase
          </button>
          <button
            className="pipe-btn"
            disabled={dirtyCount === 0}
            onMouseEnter={(e) => tooltip.show("AI writes commit messages, you review and commit", e)}
            onMouseLeave={() => tooltip.hide()}
            onClick={() => setCommitOpen(true)}
          >
            <CommitIcon size={13} /> Commit
            {dirtyCount > 0 && <span className="pipe-badge pipe-badge-warn">{dirtyCount}</span>}
          </button>
          <button
            className="pipe-btn"
            disabled={aheadCount === 0}
            onMouseEnter={(e) => tooltip.show("Push branches to origin, then create the pull requests", e)}
            onMouseLeave={() => tooltip.hide()}
            onClick={() => setPushOpen(true)}
          >
            <PushIcon size={13} /> Push
            {aheadCount > 0 && <span className="pipe-badge">{aheadCount}</span>}
          </button>
          <button
            className={`pipe-btn ${prsOpen ? "pipe-btn-active" : ""}`}
            onMouseEnter={(e) => tooltip.show(`Track the PRs of branch ${workspace.branch} across the workspace repos`, e)}
            onMouseLeave={() => tooltip.hide()}
            onClick={loadPrs}
          >
            {prsLoading ? <span className="spinner" /> : <PullRequestIcon size={13} />} Pull requests
          </button>
        </div>
      </div>

      {prsOpen && (
        <div className="ws-pr-tracker">
          <div className="ws-pr-tracker-head">
            <span className="mono ws-pr-tracker-branch">{workspace.branch}</span>
            <button
              className="btn-mini"
              onClick={() => {
                setPrStatuses(null);
                setPrsLoading(true);
                invoke<WsPrStatus[]>("ws_pr_status", { workspace: workspace.name })
                  .then((rows) => setPrStatuses(rows))
                  .catch((e) => {
                    setPrStatuses([]);
                    onError(String(e));
                  })
                  .finally(() => setPrsLoading(false));
              }}
              onMouseEnter={(e) => tooltip.show("Reload PR statuses", e)}
              onMouseLeave={() => tooltip.hide()}
            >
              Refresh
            </button>
          </div>
          {prStatuses === null ? (
            <div className="ws-pr-skeleton">
              <Skeleton w="70%" h={16} />
              <Skeleton w="100%" h={12} />
              <Skeleton w="55%" h={12} />
            </div>
          ) : prStatuses.length === 0 ? (
            <div className="ws-pr-tracker-empty">
              <p className="ws-commit-placeholder">
                No pull requests for this branch yet.
              </p>
              {aheadCount > 0 || pushedNoPr ? (
                <button type="button" onClick={() => setPrCreateOpen(true)}>
                  Create pull requests
                </button>
              ) : (
                <p className="ws-commit-placeholder">
                  Commit and push your work first — the branches aren't on origin yet.
                </p>
              )}
            </div>
          ) : (
            <div className="ws-pr-tracker-list">
              {prStatuses.map((pr) => (
                <div key={pr.repo + pr.number} className="ws-pr-tracker-row">
                  <span className={`tag ws-pr-state ws-pr-state-${pr.state.toLowerCase()}`}>
                    {pr.state === "MERGED" ? "merged" : pr.state === "CLOSED" ? "closed" : pr.isDraft ? "draft" : "open"}
                  </span>
                  <span className="mono ws-pr-tracker-repo">{pr.repo}</span>
                  <span className="ws-pr-tracker-title" title={pr.title}>#{pr.number} {pr.title}</span>
                  <span className="ws-pr-tracker-meta">
                    {pr.commits} {pr.commits === 1 ? "commit" : "commits"} · by {pr.author}
                  </span>
                  <a
                    className="btn-mini"
                    href={pr.url}
                    onClick={(e) => {
                      e.preventDefault();
                      openUrl(pr.url).catch(() => null);
                    }}
                    onMouseEnter={(e) => tooltip.show("Open on GitHub ↗", e)}
                    onMouseLeave={() => tooltip.hide()}
                  >
                    View on GitHub ↗
                  </a>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      <PlanProgress
        workspace={workspace.name}
        refreshKey={statuses === null ? 0 : 1}
        onOpenPlan={onOpenPlan}
        onError={onError}
      />

      <div className="table-wrap">
        <table className="list">
          <thead>
            <tr>
              <th style={{ width: "28%" }}>Repository</th>
              <th style={{ width: "24%" }}>Branch</th>
              <th style={{ width: "14%" }}>Changes</th>
              <th style={{ width: "10%" }}>Ahead</th>
              <th style={{ width: "10%" }}>Behind</th>
            </tr>
          </thead>
          <tbody>
            {(statuses ?? []).map((st) => (
              <tr
                key={st.repo}
                className="ws-repo-row"
                onClick={() => onOpenEditor(st.repo)}
                title="Click to browse files"
              >
                <td className="cell-name">{st.repo}</td>
                <td>
                  <span className="mono">{st.branch ?? "—"}</span>
                </td>
                <td>
                  {st.dirty ? <span className="tag tag-warn">dirty</span> : <span className="tag tag-ok">clean</span>}
                </td>
                <td>{st.ahead > 0 ? <span className="tag tag-info">↑{st.ahead}</span> : <span className="tag tag-muted">—</span>}</td>
                <td>{st.behind > 0 ? <span className="tag tag-warn">↓{st.behind}</span> : <span className="tag tag-muted">—</span>}</td>
              </tr>
            ))}
            {statuses === null && (
              <tr>
                <td colSpan={5} className="table-skeleton-cell">
                  <SkeletonTable rows={Math.max(2, workspace.repos.length)} cols={5} />
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {rebaseOpen && (
        <RebaseModal
          workspace={workspace.name}
          base={workspace.base}
          repos={workspace.repos}
          onClose={() => setRebaseOpen(false)}
          onSettled={() => load(false)}
        />
      )}

      {commitOpen && dirtyCount > 0 && (
        <CommitModal
          workspace={workspace.name}
          repos={(statuses ?? []).filter((s) => s.dirty).map((s) => s.repo)}
          onClose={() => setCommitOpen(false)}
          onSettled={() => load(false)}
          onError={onError}
        />
      )}

      {pushOpen && (
        <PushModal
          workspace={workspace.name}
          repos={workspace.repos}
          onCreatePrs={() => {
            setPushOpen(false);
            setPrCreateOpen(true);
          }}
          onClose={() => setPushOpen(false)}
          onSettled={() => load(false)}
        />
      )}

      {prCreateOpen && (
        <PrsModal
          workspace={workspace.name}
          base={workspace.base}
          repos={(statuses ?? []).map((s) => ({ repo: s.repo, prCommits: s.prCommits }))}
          defaultTitle={workspace.card?.title ?? workspace.branch}
          onOpenPrs={(prs) => {
            setPrCreateOpen(false);
            onOpenPrList(prs);
          }}
          onClose={() => setPrCreateOpen(false)}
          onError={onError}
        />
      )}

      {confirmRemove && (
        <ConfirmDialog
          title={confirmRemove === "force" ? "Force remove workspace" : "Remove workspace"}
          message={
            confirmRemove === "force"
              ? "Some worktrees have uncommitted changes. Force remove will discard them permanently. Continue?"
              : `Remove workspace '${workspace.name}'? This deletes its worktrees and the branch '${workspace.branch}' in each repository.`
          }
          confirmLabel={confirmRemove === "force" ? "Force remove" : "Remove"}
          danger
          onConfirm={() => remove(confirmRemove === "force")}
          onClose={() => setConfirmRemove(null)}
        />
      )}

      {grillOpen && workspace.card && (
        <GrillModal
          workspace={workspace}
          card={workspace.card}
          onPlanReady={() => setPlanExists(true)}
          onClose={() => setGrillOpen(false)}
          onError={onError}
        />
      )}

      {planOpen && workspace.card && (
        <PlanModal
          workspace={workspace}
          card={workspace.card}
          onOpenPlan={() => {
            setPlanOpen(false);
            onOpenPlan();
          }}
          onStartInterview={() => setGrillOpen(true)}
          onClose={() => {
            setPlanOpen(false);
            setPlanExists(true);
          }}
          onError={onError}
        />
      )}
    </div>
  );
}