import { useCallback, useEffect, useState } from "react";
import { AddressReviewModal } from "../components/AddressReview";
import { RaceModal, RaceSection, StartSession } from "../components/Race";
import { AgentStatus } from "../lib/agentStatus";
import { toast } from "../components/Toast";
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
import { Skeleton } from "../components/Skeleton";
import { PlanModal } from "../components/PlanModal";
import { GrillModal } from "../components/GrillModal";
import { PlanProgress } from "../components/PlanProgress";
import { CommitModal } from "../components/CommitModal";
import { RebaseModal } from "../components/RebaseModal";
import { PushModal } from "../components/PushModal";
import { BranchDriftBanner } from "../components/BranchDriftBanner";
import { MergeModal } from "../components/MergeModal";
import { PrsModal } from "../components/PrsModal";
import { BuildModal } from "../components/BuildModal";
import { tooltip } from "../components/Tooltip";
import { RebaseIcon, CommitIcon, PushIcon, PullRequestIcon, ChevronRightIcon, SparkIcon, CheckIcon, XIcon, CircleIcon, SpinnerIcon, GitIcon, DocIcon, TrashIcon, RefreshIcon, PlayIcon, RaceIcon, BranchIcon, DiffIcon } from "../components/Icons";
import { GitHubIcon } from "../components/BrandIcons";

interface Props {
  workspace: Workspace;
  onOpenEditor: (repo: string) => void;
  onOpenPlan: () => void;
  onRemoved: () => void;
  onError: (msg: string) => void;
  /** Opens a PR review tab for an explicit PR list. */
  onOpenPrList: (prs: PullRequest[]) => void;
  /** Opens the workspace's Ralph tab. */
  onOpenRalph: () => void;
  /** Race support: start an agent session, open another workspace, live status, list refresh. */
  onStartSession: StartSession;
  onOpenWorkspace: (name: string) => void;
  statusOf: (ws: string) => AgentStatus | null;
  onWorkspacesChanged: () => void;
  /** Closes the sessions running in these workspaces (before deleting them). */
  onBeforeRemove: (names: string[]) => void;
}

export function WorkspaceDetailPage({
  workspace,
  onOpenEditor,
  onOpenPlan,
  onRemoved,
  onError,
  onOpenPrList,
  onOpenRalph,
  onStartSession,
  onOpenWorkspace,
  statusOf,
  onWorkspacesChanged,
  onBeforeRemove,
}: Props) {
  const [raceOpen, setRaceOpen] = useState(false);
  const [addressing, setAddressing] = useState<WsPrStatus | null>(null);
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
  const [merging, setMerging] = useState<WsPrStatus[] | null>(null);
  const [buildOpen, setBuildOpen] = useState(false);
  const [prStatuses, setPrStatuses] = useState<WsPrStatus[] | null>(null);
  const [wsChecks, setWsChecks] = useState<WsCheck[] | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null); // "repo/number"
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

  const loadPrs = useCallback(() => {
    return invoke<WsPrStatus[]>("ws_pr_status", { workspace: workspace.name })
      .then((rows) => setPrStatuses(rows))
      .catch(() => setPrStatuses([]));
  }, [workspace.name]);

  // PRs and their checks are part of the page, loaded with it.
  useEffect(() => {
    setPrStatuses(null);
    setWsChecks(null);
    loadPrs();
    loadChecks();
  }, [loadPrs, loadChecks]);

  // Keep checks fresh while any is running.
  useEffect(() => {
    const running = (wsChecks ?? []).some((w) => w.status === "running");
    if (!running) return;
    const t = window.setInterval(() => loadChecks(), 30_000);
    return () => window.clearInterval(t);
  }, [wsChecks, loadChecks]);

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
            loadPrs(),
            loadChecks(),
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
    [workspace.name, workspace.repos, loadPrs, loadChecks, onError]
  );

  // New commits on the remote branch (e.g. a checked-out PR got updated).
  const [pulling, setPulling] = useState<string | null>(null);
  const pull = async (repo: string) => {
    setPulling(repo);
    try {
      await invoke("ws_pull", { workspace: workspace.name, repo });
      toast.success(`Pulled ${repo}`);
      await load(false);
    } catch (e) {
      toast.error(`Pull failed in ${repo}`, { description: String(e) });
    } finally {
      setPulling(null);
    }
  };

  useEffect(() => {
    setStatuses(null);
    load(false);
  }, [load]);

  // A tracked PR in the code review tab (diff, threads, verdict).
  const openReview = async (pr: WsPrStatus) => {
    try {
      const ownerRepo = await invoke<string>("ws_owner_repo", { workspace: workspace.name, repo: pr.repo });
      const branch = statuses?.find((s) => s.repo === pr.repo)?.expectedBranch ?? workspace.branch;
      onOpenPrList([
        {
          repo: pr.repo,
          ownerRepo,
          number: pr.number,
          title: pr.title,
          branch,
          base: workspace.base,
          author: pr.author,
          isDraft: pr.isDraft,
          url: pr.url,
          updatedAt: pr.updatedAt,
        },
      ]);
    } catch (e) {
      onError(String(e));
    }
  };

  const dirtyCount = (statuses ?? []).filter((s) => s.dirty).length;
  // Squash-merged repos keep local commits no remote has; they need no push
  // (pushing would even recreate the deleted branch).
  const pushRepos = (statuses ?? []).filter((s) => !s.integrated);
  const aheadCount = pushRepos.reduce((a, s) => a + s.ahead, 0);
  const pushedContent = (statuses ?? []).some((s) => s.prCommits > 0);
  const openPrs = (prStatuses ?? []).filter((p) => p.state === "OPEN");
  const mergedPrs = (prStatuses ?? []).filter((p) => p.state === "MERGED");
  const landed = mergedPrs.length > 0 || (statuses ?? []).some((s) => s.integrated);
  const canCreatePrs = prStatuses !== null && prStatuses.length === 0 && (aheadCount > 0 || pushedContent);
  const checkStates = (wsChecks ?? []).map((c) => c.status);
  const checksFailing = checkStates.includes("fail");
  const checksRunning = checkStates.includes("running");
  const checksPassing = checkStates.length > 0 && checkStates.every((s) => s === "pass");
  const mergeablePrs = openPrs.filter((p) => !p.isDraft);
  const canMerge = mergeablePrs.length > 0 && !checksFailing && !checksRunning;
  // Every PR landed and nothing new is pending: the feature is done.
  const featureDone =
    statuses !== null && mergedPrs.length > 0 && openPrs.length === 0 && dirtyCount === 0 && aheadCount === 0;

  const hint = (text: string) => ({
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  });

  const remove = async (force: boolean) => {
    try {
      onBeforeRemove([workspace.name]);
      const kept = await invoke<string[]>("remove_workspace", { name: workspace.name, force });
      setConfirmRemove(null);
      if (kept.length) toast.info(`Workspace ${workspace.name} removed; branches with unpushed work kept`, { description: kept.join("\n") });
      else toast.success(`Workspace ${workspace.name} removed`);
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

  const scrollToPrs = () => document.getElementById(`prs-${workspace.name}`)?.scrollIntoView({ behavior: "smooth" });

  // Delivery flow: each step shows its state and, when it is the next thing
  // to do, the action that moves it forward.
  const loadingStatus = statuses === null;
  const steps: FlowStep[] = [
    {
      key: "changes",
      icon: <CommitIcon size={15} />,
      title: "Changes",
      state: loadingStatus ? "loading" : dirtyCount > 0 ? "action" : "done",
      detail: loadingStatus
        ? "Checking…"
        : dirtyCount > 0
          ? `${dirtyCount} repo${dirtyCount === 1 ? "" : "s"} with uncommitted changes`
          : "Working tree clean",
      action: dirtyCount > 0 ? { label: "Commit", onClick: () => setCommitOpen(true) } : undefined,
    },
    {
      key: "push",
      icon: <PushIcon size={15} />,
      title: "Push",
      state: loadingStatus ? "loading" : aheadCount > 0 ? "action" : pushedContent || landed ? "done" : "idle",
      detail: loadingStatus
        ? "Checking…"
        : aheadCount > 0
          ? `${aheadCount} commit${aheadCount === 1 ? "" : "s"} to push`
          : pushedContent
            ? "Everything is on origin"
            : landed
              ? `Merged into ${workspace.base}`
              : "No commits yet",
      action: aheadCount > 0 ? { label: "Push", onClick: () => setPushOpen(true) } : undefined,
    },
    {
      key: "prs",
      icon: <PullRequestIcon size={15} />,
      title: "Pull requests",
      state: prStatuses === null ? "loading" : canCreatePrs ? "action" : openPrs.length > 0 || mergedPrs.length > 0 ? "done" : "idle",
      detail:
        prStatuses === null
          ? "Checking GitHub…"
          : prStatuses.length > 0
            ? [openPrs.length && `${openPrs.length} open`, mergedPrs.length && `${mergedPrs.length} merged`].filter(Boolean).join(" · ") ||
              `${prStatuses.length} closed`
            : canCreatePrs
              ? "Ready to open"
              : "Push your work first",
      action: canCreatePrs
        ? { label: "Create", onClick: () => setPrCreateOpen(true) }
        : canMerge
          ? { label: "Squash and merge", onClick: () => setMerging(mergeablePrs), secondary: true }
          : prStatuses && prStatuses.length > 0
          ? { label: "View", onClick: scrollToPrs, secondary: true }
          : undefined,
    },
    {
      key: "checks",
      icon: <CheckIcon size={14} />,
      title: "Checks",
      state:
        wsChecks === null ? "loading" : checksFailing ? "fail" : checksRunning ? "running" : checksPassing ? "done" : "idle",
      detail:
        wsChecks === null
          ? "Checking CI…"
          : checksFailing
            ? "Some checks failed"
            : checksRunning
              ? "Running…"
              : checksPassing
                ? "All checks passed"
                : "No checks yet",
      action: checksFailing ? { label: "Review", onClick: scrollToPrs } : undefined,
    },
  ];

  // Only the first step that needs something is highlighted: the next move.
  const nextStep = steps.findIndex((s) => s.state === "action" || s.state === "fail");

  return (
    <div className="page ws-detail">
      {workspace.variant_of && (
        <div className="race-banner">
          <RaceIcon size={13} />
          <span>
            Race variant of <strong>{workspace.variant_of}</strong>
            {workspace.agent && <> · {workspace.agent}</>}
          </span>
          <button className="btn-link" onClick={() => onOpenWorkspace(workspace.variant_of!)}>
            Compare in {workspace.variant_of} →
          </button>
        </div>
      )}
      <header className="ws-head">
        <div className="ws-head-main">
          <h2>{workspace.name}</h2>
          <div className="ws-head-meta">
            <span className="ws-branch">
              <GitIcon size={12} /> {workspace.branch}
            </span>
            <span className="ws-meta-sep">from</span>
            <span className="ws-meta-muted">{workspace.base}</span>
            <span className="ws-meta-sep">·</span>
            <span className="ws-meta-muted">
              {workspace.repos.length} repo{workspace.repos.length === 1 ? "" : "s"}
            </span>
            {workspace.card && (
              <button className="dash-card-link" onClick={() => openUrl(workspace.card!.url).catch(() => null)} {...hint(workspace.card.title)}>
                {workspace.card.id}
              </button>
            )}
          </div>
        </div>
        <div className="ws-head-tools">
          {workspace.card && (
            <button
              className="secondary ws-tool"
              {...hint(planExists ? "Open the generated PLAN.md" : "Generate a PLAN.md from the linked card")}
              onClick={() => (planExists ? onOpenPlan() : setPlanOpen(true))}
            >
              <DocIcon size={14} /> {planExists ? "Plan" : "Create plan"}
            </button>
          )}
          <button className="secondary ws-tool" onClick={onOpenRalph} {...hint("Write a PRD and let the agent implement it story by story")}>
            <SparkIcon size={14} /> Ralph
          </button>
          {!workspace.variant_of && (
            <button className="secondary ws-tool" onClick={() => setRaceOpen(true)} {...hint("Run the same task with several agents and keep the best result")}>
              <RaceIcon size={14} /> Race
            </button>
          )}
          <button className="secondary ws-tool" onClick={() => setBuildOpen(true)} {...hint("Run each repo's build (skips unchanged ones)")}>
            <PlayIcon size={13} /> Build
          </button>
          <button className="secondary ws-tool" onClick={() => setRebaseOpen(true)} {...hint(`Rebase every repo onto origin/${workspace.base}`)}>
            <RebaseIcon size={14} /> Rebase
          </button>
          <span className="ws-tool-sep" />
          <button
            className="icon-button ws-icon-tool"
            disabled={refreshing}
            aria-label="Refresh"
            {...hint("Fetch and refresh status, PRs and checks")}
            onClick={() => load(true)}
          >
            {refreshing ? <span className="spinner" /> : <RefreshIcon size={15} />}
          </button>
          <button className="icon-button icon-button-danger ws-icon-tool" aria-label="Remove workspace" {...hint("Remove workspace")} onClick={() => setConfirmRemove("normal")}>
            <TrashIcon size={15} />
          </button>
        </div>
      </header>

      <BranchDriftBanner workspace={workspace.name} statuses={statuses} onSynced={() => load(false)} />

      {featureDone && (
        <div className="done-banner">
          <CheckIcon size={13} />
          <span>
            Merged into <strong>{workspace.base}</strong>. This feature is done; removing the workspace deletes its worktrees and local
            branches.
          </span>
          <button className="secondary btn-mini" onClick={() => setConfirmRemove("normal")}>
            Remove workspace
          </button>
        </div>
      )}

      <div className="flow">
        {steps.map((s, i) => (
          <div key={s.key} className={`flow-step flow-${s.state} ${i === nextStep ? "flow-next" : ""}`}>
            <div className="flow-step-top">
              <span className="flow-icon">
                {s.state === "done" ? <CheckIcon size={13} /> : s.state === "fail" ? <XIcon size={12} /> : s.state === "running" || s.state === "loading" ? <span className="spinner" /> : s.icon}
              </span>
              <span className="flow-title">{s.title}</span>
              {i < steps.length - 1 && <span className="flow-arrow">→</span>}
            </div>
            <div className="flow-detail">{s.detail}</div>
            {s.action && (
              <button className={`flow-btn ${s.action.secondary || i !== nextStep ? "secondary" : ""}`} onClick={s.action.onClick}>
                {s.action.label}
              </button>
            )}
          </div>
        ))}
      </div>

      <PlanProgress workspace={workspace.name} refreshKey={statuses === null ? 0 : 1} onOpenPlan={onOpenPlan} onError={onError} />

      {!workspace.variant_of && (
        <RaceSection
          workspace={workspace}
          statusOf={statusOf}
          onOpenWorkspace={onOpenWorkspace}
          onChanged={() => {
            onWorkspacesChanged();
            load(false);
          }}
          onBeforeRemove={onBeforeRemove}
          onError={onError}
        />
      )}
      {raceOpen && (
        <RaceModal workspace={workspace} onStartSession={onStartSession} onStarted={onWorkspacesChanged} onClose={() => setRaceOpen(false)} />
      )}

      <section className="ws-section">
        <div className="ws-section-head">
          <h3>Repositories</h3>
        </div>
        <div className="repo-list ws-repos">
          {statuses === null
            ? workspace.repos.map((repo) => (
                <div key={repo} className="repo-row ws-repo-row">
                  <span className="repo-icon">
                    <GitIcon size={15} />
                  </span>
                  <div className="repo-main">
                    <span className="repo-name">{repo}</span>
                    <Skeleton w={140} h={10} />
                  </div>
                </div>
              ))
            : statuses.map((st) => (
                <div
                  key={st.repo}
                  className="repo-row ws-repo-row"
                  role="button"
                  tabIndex={0}
                  onClick={() => onOpenEditor(st.repo)}
                  onKeyDown={(e) => e.key === "Enter" && onOpenEditor(st.repo)}
                >
                  <span className="repo-icon">
                    <GitIcon size={15} />
                  </span>
                  <div className="repo-main">
                    <span className="repo-name">{st.repo}</span>
                    <span className="repo-sub">{st.branch ?? "no branch"}</span>
                  </div>
                  <div className="ws-repo-badges">
                    {st.offBranch && (
                      <span
                        className="ws-badge ws-badge-warn"
                        {...hint(`Expected ${st.expectedBranch}${st.heldBy ? `; the clone is on workspace ${st.heldBy}'s branch` : ""}`)}
                      >
                        <BranchIcon size={11} /> Off branch
                      </span>
                    )}
                    {st.dirty ? (
                      <span className="ws-badge ws-badge-warn">
                        <span className="dash-repo-dot" /> Uncommitted changes
                      </span>
                    ) : (
                      <span className="ws-badge ws-badge-ok">
                        <span className="dash-repo-dot" /> Clean
                      </span>
                    )}
                    {st.integrated ? (
                      <span className="ws-badge ws-badge-merged" {...hint(`This branch's work is already in ${workspace.base}`)}>
                        Merged
                      </span>
                    ) : (
                      st.ahead > 0 && (
                        <span className="ws-badge ws-badge-info" {...hint(`${st.ahead} commit(s) not pushed`)}>
                          ↑ {st.ahead}
                        </span>
                      )
                    )}
                    {st.behind > 0 &&
                      (st.ahead === 0 ? (
                        <button
                          className="ws-badge ws-badge-warn"
                          disabled={pulling === st.repo}
                          {...hint(`${st.behind} new commit(s) on the remote — click to pull them`)}
                          onClick={(e) => {
                            e.stopPropagation();
                            pull(st.repo);
                          }}
                        >
                          {pulling === st.repo ? "Pulling…" : `↓ ${st.behind} Pull`}
                        </button>
                      ) : (
                        <span className="ws-badge ws-badge-warn" {...hint(`${st.behind} commit(s) behind the remote`)}>
                          ↓ {st.behind}
                        </span>
                      ))}
                  </div>
                  <span className="ws-repo-open">Browse files →</span>
                </div>
              ))}
        </div>
      </section>

      <section className="ws-section" id={`prs-${workspace.name}`}>
        <div className="ws-section-head">
          <h3>
            Pull requests {prStatuses && prStatuses.length > 0 && <span className="section-count">{prStatuses.length}</span>}
          </h3>
        </div>
        {prStatuses === null ? (
          <div className="pr-list">
            <div className="pr-row">
              <Skeleton w="60%" h={14} />
            </div>
          </div>
        ) : prStatuses.length === 0 ? (
          <div className="pr-empty">
            <span className="pr-empty-icon">
              <PullRequestIcon size={18} />
            </span>
            <div>
              <strong>{canCreatePrs ? "Ready for pull requests" : "No pull requests yet"}</strong>
              <span>
                {canCreatePrs
                  ? `Orbit opens one PR per repo from ${workspace.branch} into ${workspace.base}, with AI-drafted titles and descriptions.`
                  : dirtyCount > 0
                    ? "Commit your changes, push them, then open the PRs here."
                    : aheadCount > 0
                      ? "Push your commits, then open the PRs here."
                      : `PRs for ${workspace.branch} across this workspace's repos show up here, with their CI checks.`}
              </span>
            </div>
            {canCreatePrs ? (
              <button onClick={() => setPrCreateOpen(true)}>Create pull requests</button>
            ) : dirtyCount > 0 ? (
              <button className="secondary" onClick={() => setCommitOpen(true)}>
                Commit changes
              </button>
            ) : aheadCount > 0 ? (
              <button className="secondary" onClick={() => setPushOpen(true)}>
                Push
              </button>
            ) : null}
          </div>
        ) : (
          <div className="pr-list">
            {prStatuses.map((pr) => {
              const wsCheck = (wsChecks ?? []).find((c) => c.repo === pr.repo && c.prNumber === pr.number);
              const key = `${pr.repo}/${pr.number}`;
              const open = expanded === key;
              const state = pr.state === "MERGED" ? "merged" : pr.state === "CLOSED" ? "closed" : pr.isDraft ? "draft" : "open";
              const total = wsCheck?.checks.length ?? 0;
              const passed = wsCheck?.checks.filter((c) => c.bucket === "pass").length ?? 0;
              return (
                <div key={key} className={`pr-item ${open ? "open" : ""}`}>
                  <div className="pr-row" role="button" tabIndex={0} onClick={() => setExpanded(open ? null : key)}>
                    <span className={`pr-state pr-state-${state}`}>
                      <PullRequestIcon size={13} />
                    </span>
                    <div className="pr-main">
                      <span className="pr-title">
                        {pr.title} <span className="pr-number">#{pr.number}</span>
                      </span>
                      <span className="pr-sub">
                        <span className="pr-repo">{pr.repo}</span>
                        <span className={`pr-pill pr-pill-${state}`}>{state}</span>
                        {pr.commits} commit{pr.commits === 1 ? "" : "s"} · {pr.author}
                      </span>
                    </div>
                    <span className="pr-checks">
                      <CheckDot status={wsCheck?.status ?? "none"} />
                      {total > 0 ? `${passed}/${total} checks` : "No checks"}
                    </span>
                    {state === "open" || state === "draft" ? (
                      <button
                        className="btn-mini secondary pr-feedback-btn"
                        onClick={(e) => {
                          e.stopPropagation();
                          setAddressing(pr);
                        }}
                        {...hint("Apply review comments with AI, reply and resolve")}
                      >
                        Review feedback
                      </button>
                    ) : null}
                    {state === "open" && (
                      <button
                        className="btn-mini secondary pr-feedback-btn"
                        disabled={wsCheck?.status === "fail" || wsCheck?.status === "running"}
                        onClick={(e) => {
                          e.stopPropagation();
                          setMerging([pr]);
                        }}
                        {...hint(
                          wsCheck?.status === "fail"
                            ? "Checks are failing"
                            : wsCheck?.status === "running"
                              ? "Checks are still running"
                              : `Squash into ${workspace.base} and delete the branch on GitHub`,
                        )}
                      >
                        Squash and merge
                      </button>
                    )}
                    <button
                      className="icon-button"
                      aria-label="Open in code review"
                      onClick={(e) => {
                        e.stopPropagation();
                        openReview(pr);
                      }}
                      {...hint("Open in code review")}
                    >
                      <DiffIcon size={14} />
                    </button>
                    <button
                      className="icon-button"
                      aria-label="Open on GitHub"
                      onClick={(e) => {
                        e.stopPropagation();
                        openUrl(pr.url).catch(() => null);
                      }}
                      {...hint("Open on GitHub ↗")}
                    >
                      <GitHubIcon size={14} />
                    </button>
                    <ChevronRightIcon size={13} className={open ? "ws-chevron ws-chevron-open" : "ws-chevron"} />
                  </div>
                  {open && (
                    <ChecksList
                      wsCheck={wsCheck}
                      repo={pr.repo}
                      onRerun={(link) =>
                        invoke("check_rerun", { link })
                          .then(() => {
                            toast.info("Re-running failed jobs");
                            loadChecks();
                          })
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
      </section>

      {addressing && (
        <AddressReviewModal
          workspace={workspace.name}
          repo={addressing.repo}
          number={addressing.number}
          title={addressing.title}
          url={addressing.url}
          onClose={() => setAddressing(null)}
          onSettled={() => load(false)}
          onError={onError}
        />
      )}

      {buildOpen && <BuildModal workspace={workspace.name} repos={workspace.repos} onClose={() => setBuildOpen(false)} onError={onError} />}

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
          repos={statuses ? pushRepos.map((s) => s.repo) : workspace.repos}
          onCreatePrs={() => {
            setPushOpen(false);
            setPrCreateOpen(true);
          }}
          onClose={() => setPushOpen(false)}
          onSettled={() => load(false)}
        />
      )}

      {merging && (
        <MergeModal
          workspace={workspace.name}
          base={workspace.base}
          prs={merging}
          onClose={() => setMerging(null)}
          // The merge deletes the remote branch: fetch so status sees it.
          onSettled={() => load(true)}
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
            loadPrs();
            loadChecks();
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

interface FlowStep {
  key: string;
  icon: React.ReactNode;
  title: string;
  state: "loading" | "idle" | "action" | "done" | "running" | "fail";
  detail: string;
  action?: { label: string; onClick: () => void; secondary?: boolean };
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