import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  PullRequest,
  RepoStatus,
  Workspace,
  WsPrStatus,
  WsCheck,
  PrCheck,
  CheckAnalysis,
} from "../types/config";
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
import { RebaseIcon, CommitIcon, PushIcon, PullRequestIcon, ChevronRightIcon, SparkIcon, CheckIcon, XIcon, CircleIcon, SpinnerIcon } from "../components/Icons";
import { GitHubIcon } from "../components/BrandIcons";

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
  const [wsChecks, setWsChecks] = useState<WsCheck[] | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null); // "repo/number"
  const [prsLoading, setPrsLoading] = useState(false);
  const [investigation, setInvestigation] = useState<null | {
    repo: string;
    check: PrCheck;
    analysis: CheckAnalysis | null;
    error?: string;
  }>(null);

  const loadChecks = useCallback(() => {
    return invoke<WsCheck[]>("ws_checks", { workspace: workspace.name })
      .then((rows) => setWsChecks(rows))
      .catch(() => setWsChecks([]));
  }, [workspace.name]);

  // Poll every 30s while the tracker is open and something is running.
  useEffect(() => {
    if (!prsOpen) return;
    const running = (wsChecks ?? []).some((w) => w.status === "running");
    if (!running) return;
    const t = window.setInterval(() => loadChecks(), 30_000);
    return () => window.clearInterval(t);
  }, [prsOpen, wsChecks, loadChecks]);

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
    loadChecks();
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
          await Promise.all([
            ...workspace.repos.map((repo) => invoke("refresh_repo", { name: repo }).catch(() => null)),
            // Tracker open? Refresh its PR statuses and checks along.
            prsOpen
              ? invoke<WsPrStatus[]>("ws_pr_status", { workspace: workspace.name })
                  .then((rows) => setPrStatuses(rows))
                  .catch(() => null)
              : null,
            prsOpen ? loadChecks() : null,
          ]);
        }
        const st = await invoke<RepoStatus[]>("workspace_status", { name: workspace.name });
        setStatuses(st);
      } catch (e) {
        onError(String(e));
      } finally {
        setRefreshing(false);
      }
    },
    [workspace.name, workspace.repos, prsOpen, loadChecks, onError]
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
            <span className="ws-pr-tracker-meta">
              {prStatuses?.length ?? 0} pull {prStatuses?.length === 1 ? "request" : "requests"} · statuses update with the workspace Refresh
            </span>
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
              {prStatuses.map((pr) => {
                const wsCheck = (wsChecks ?? []).find(
                  (c) => c.repo === pr.repo && c.prNumber === pr.number
                );
                const key = `${pr.repo}/${pr.number}`;
                const open = expanded === key;
                return (
                  <div key={key} className="ws-pr-item">
                    <div
                      className={`ws-pr-tracker-row ${open ? "ws-pr-row-open" : ""}`}
                      onClick={() => setExpanded(open ? null : key)}
                    >
                      <CheckDot status={wsCheck?.status ?? "none"} />
                      <span className={`tag ws-pr-state ws-pr-state-${pr.state.toLowerCase()}`}>
                        {pr.state === "MERGED" ? "merged" : pr.state === "CLOSED" ? "closed" : pr.isDraft ? "draft" : "open"}
                      </span>
                      <span className="mono ws-pr-tracker-repo">{pr.repo}</span>
                      <span className="ws-pr-tracker-title" title={pr.title}>#{pr.number} {pr.title}</span>
                      <span className="ws-pr-tracker-meta">
                        {pr.commits} {pr.commits === 1 ? "commit" : "commits"} · by {pr.author}
                      </span>
                      <ChevronRightIcon size={13} className={open ? "ws-chevron ws-chevron-open" : "ws-chevron"} />
                      <button
                        className="icon-button"
                        onClick={(e) => {
                          e.stopPropagation();
                          openUrl(pr.url).catch(() => null);
                        }}
                        onMouseEnter={(e) => tooltip.show("Open on GitHub ↗", e)}
                        onMouseLeave={() => tooltip.hide()}
                      >
                        <GitHubIcon size={14} />
                      </button>
                    </div>
                    {open && (
                      <ChecksList
                        wsCheck={wsCheck}
                        repo={pr.repo}
                        onRerun={(link) =>
                          invoke("check_rerun", { link })
                            .then(() => loadChecks())
                            .catch((e) => onError(String(e)))
                        }
                        onInvestigate={(repo, check) => setInvestigation({ repo, check, analysis: null })}
                      />
                    )}
                  </div>
                );
              })}
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

      {investigation && (
        <InvestigateModal
          workspace={workspace.name}
          repo={investigation.repo}
          check={investigation.check}
          analysis={investigation.analysis}
          onApplied={() => {
            setInvestigation(null);
            loadChecks();
            load(false);
          }}
          onClose={() => setInvestigation(null)}
          onError={onError}
        />
      )}
    </div>
  );
}

/** Aggregate CI dot for a PR row. */
function CheckDot({ status }: { status: string }) {
  const cls = `ws-check-dot ws-check-${status}`;
  return (
    <span
      className={cls}
      onMouseEnter={(e) =>
        tooltip.show(
          status === "pass"
            ? "All checks passed"
            : status === "fail"
              ? "Some checks failed"
              : status === "running"
                ? "Checks running…"
                : "No checks",
          e
        )
      }
      onMouseLeave={() => tooltip.hide()}
    >
      {status === "running" && <SpinnerIcon size={10} />}
    </span>
  );
}

/** Expanded checks of one PR: state icon, duration, Re-run / Investigate
 *  on failures, link on the name. */
function ChecksList({
  wsCheck,
  repo,
  onRerun,
  onInvestigate,
}: {
  wsCheck: WsCheck | undefined;
  repo: string;
  onRerun: (link: string) => void;
  onInvestigate: (repo: string, check: PrCheck) => void;
}) {
  const [rerunning, setRerunning] = useState<string | null>(null);
  if (!wsCheck || wsCheck.checks.length === 0) {
    return <div className="ws-checks"><p className="ws-commit-placeholder">No CI checks on this PR.</p></div>;
  }
  return (
    <div className="ws-checks">
      {wsCheck.checks.map((c) => {
        const dur = durationOf(c);
        const failed = c.bucket === "fail";
        const running = c.bucket === "pending" || c.bucket === "queued";
        return (
          <div key={c.name + c.link} className={`ws-check-row ${failed ? "ws-check-failed" : ""}`}>
            <span className={`ws-check-icon ws-check-${c.bucket}`}>
              {running ? (
                <SpinnerIcon size={12} />
              ) : failed ? (
                <XIcon size={11} />
              ) : c.bucket === "pass" ? (
                <CheckIcon size={11} />
              ) : (
                <CircleIcon size={10} />
              )}
            </span>
            <span
              className={`ws-check-name mono ${c.link ? "ws-check-link" : ""}`}
              title={c.link || c.workflow || c.name}
              onClick={
                c.link
                  ? () => {
                      openUrl(c.link).catch(() => null);
                    }
                  : undefined
              }
              onMouseEnter={
                c.link
                  ? (e) => tooltip.show("Open this check on GitHub ↗", e)
                  : undefined
              }
              onMouseLeave={() => tooltip.hide()}
            >
              {c.name}
            </span>
            <span className="ws-check-dur">{dur}</span>
            <span className="ws-check-actions">
              {failed && rerunning !== c.link && (
                <>
                  <button
                    className="btn-mini"
                    onClick={() => {
                      setRerunning(c.link);
                      onRerun(c.link);
                    }}
                    onMouseEnter={(e) => tooltip.show("Re-run the failed jobs (fixes intermittent failures)", e)}
                    onMouseLeave={() => tooltip.hide()}
                  >
                    ↻ Re-run
                  </button>
                  <button
                    className="btn-mini"
                    onClick={() => onInvestigate(repo, c)}
                    onMouseEnter={(e) => tooltip.show("AI reads the failure log and proposes a fix", e)}
                    onMouseLeave={() => tooltip.hide()}
                  >
                    <SparkIcon size={11} /> Investigate
                  </button>
                </>
              )}
              {rerunning === c.link && <span className="ws-check-meta">re-running…</span>}
            </span>
          </div>
        );
      })}
    </div>
  );
}

/** Human duration from a check's start/completed timestamps. */
function durationOf(c: PrCheck): string {
  const s = Date.parse(c.startedAt);
  const e = Date.parse(c.completedAt);
  if (Number.isNaN(s) || Number.isNaN(e) || s <= 0 || e <= 0 || e < s) return "";
  const sec = Math.round((e - s) / 1000);
  if (sec < 60) return `${sec}s`;
  const m = Math.floor(sec / 60);
  if (m < 60) return `${m}m${sec % 60 ? ` ${sec % 60}s` : ""}`;
  return `${Math.floor(m / 60)}h${m % 60}`;
}

/** AI investigation modal: fetches the failed log, asks the agent, shows
 *  problem + proposed fix; Apply runs the fix agent in the worktree. */
function InvestigateModal({
  workspace,
  repo,
  check,
  analysis,
  onApplied,
  onClose,
  onError,
}: {
  workspace: string;
  repo: string;
  check: PrCheck;
  analysis: CheckAnalysis | null;
  onApplied: () => void;
  onClose: () => void;
  onError: (msg: string) => void;
}) {
  const [state, setState] = useState<null | "analyzing" | "applying">(null);
  const [result, setResult] = useState<CheckAnalysis | null>(analysis);
  const [startedAt] = useState(Date.now());
  const [now, setNow] = useState(Date.now());

  // Fetch logs + analysis on mount.
  useEffect(() => {
    let cancelled = false;
    setState("analyzing");
    invoke<string>("check_logs", { link: check.link })
      .then((log) =>
        invoke<CheckAnalysis>("investigate_check", {
          workspace,
          repo,
          checkName: check.name,
          failedLog: log,
        })
      )
      .then((a) => {
        if (!cancelled) {
          setResult(a);
          setState(null);
        }
      })
      .catch((e) => {
        if (!cancelled) {
          setState(null);
          onError(String(e));
          onClose();
        }
      });
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => {
      cancelled = true;
      window.clearInterval(t);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const apply = () => {
    setState("applying");
    invoke("apply_check_fix", { workspace, repo, instruction: result?.fix })
      .then(() => {
        setState(null);
        onApplied();
      })
      .catch((e) => {
        setState(null);
        onError(String(e));
      });
  };

  const elapsed = Math.floor((now - startedAt) / 1000);

  return (
    <div className="modal-overlay" onMouseDown={state ? undefined : onClose}>
      <div className="modal ws-action-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Investigate: {check.name}</h3>
          {state === "analyzing" && (
            <div className="ws-pr-skeleton">
              <p className="ws-commit-placeholder">
                <span className="spinner" /> {elapsed}s — reading the failure log…
              </p>
              <Skeleton w="100%" h={12} />
              <Skeleton w="90%" h={12} />
              <Skeleton w="60%" h={12} />
            </div>
          )}
          {state === "applying" && (
            <p className="ws-commit-placeholder">
              <span className="spinner" /> Applying the fix in {repo}…
            </p>
          )}
          {!state && result && (
            <>
              <div className="ws-invest-block">
                <div className="ws-invest-label">Problem</div>
                <div className="ws-invest-text">{result.problem}</div>
              </div>
              {result.fix && (
                <div className="ws-invest-block">
                  <div className="ws-invest-label">Proposed fix</div>
                  <div className="ws-invest-text mono">{result.fix}</div>
                </div>
              )}
            </>
          )}
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose} disabled={!!state}>
            Close
          </button>
          {!state && result?.actionable && result.fix && (
            <button type="button" autoFocus onClick={apply}>
              Apply fix
            </button>
          )}
        </div>
      </div>
    </div>
  );
}