import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrDetail, PullRequest, Workspace, WsCheck } from "../types/config";
import { CheckDot, ChecksList } from "./WorkspaceDetailPage";
import { toast } from "../components/Toast";
import { tooltip } from "../components/Tooltip";
import { CheckIcon, ChevronRightIcon, CommitIcon, DiffIcon, GitIcon, PullRequestIcon, PushIcon, RefreshIcon, XIcon } from "../components/Icons";
import { Skeleton } from "../components/Skeleton";
import { GitHubIcon } from "../components/BrandIcons";

interface Props {
  /** The feature's PRs: one per repo, sharing a branch name. */
  prs: PullRequest[];
  /** Local workspace already holding this feature, if any. */
  workspace: Workspace | null;
  onOpenReview: () => void;
  onCheckout: () => void;
  onOpenWorkspace: (ws: Workspace) => void;
  onError: (msg: string) => void;
}

const DECISION: Record<string, [string, string]> = {
  APPROVED: ["Approved", "ok"],
  CHANGES_REQUESTED: ["Changes requested", "bad"],
  REVIEW_REQUIRED: ["Review required", "muted"],
};

interface Step {
  key: string;
  icon: React.ReactNode;
  title: string;
  state: "loading" | "idle" | "done" | "running" | "fail";
  detail: string;
  action?: { label: string; onClick: () => void; disabled?: string };
  extra?: { label: string; onClick: () => void };
}

const hint = (text: string) => ({
  onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
  onMouseLeave: () => tooltip.hide(),
});

/**
 * A Code Review feature before it's checked out: the workspace page's
 * layout for PRs that only exist on GitHub. Review them from here, or
 * Check out to get the full workspace (build, agents, commit, fix checks).
 */
export function FeaturePage({ prs, workspace, onOpenReview, onCheckout, onOpenWorkspace, onError }: Props) {
  const [checks, setChecks] = useState<WsCheck[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null); // "repo/number"
  const first = prs[0];
  const multi = prs.length > 1;

  const loadChecks = useCallback(async () => {
    setRefreshing(true);
    try {
      setChecks(
        await invoke<WsCheck[]>("pr_group_checks", {
          prs: prs.map((p) => ({ repo: p.repo, ownerRepo: p.ownerRepo, number: p.number })),
        })
      );
    } catch (e) {
      setChecks([]);
      onError(String(e));
    } finally {
      setRefreshing(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prs.map((p) => `${p.ownerRepo}#${p.number}`).join(",")]);

  // Size of each PR (for the Repositories list), from GitHub.
  const [details, setDetails] = useState<Record<string, PrDetail | null>>({});
  useEffect(() => {
    let cancelled = false;
    for (const p of prs) {
      invoke<PrDetail>("pr_detail", { ownerRepo: p.ownerRepo, number: p.number })
        .then((d) => !cancelled && setDetails((all) => ({ ...all, [p.repo]: d })))
        .catch(() => !cancelled && setDetails((all) => ({ ...all, [p.repo]: null })));
    }
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [prs.map((p) => `${p.ownerRepo}#${p.number}`).join(",")]);

  // Like the workspace page: load on enter, then every minute while visible.
  useEffect(() => {
    setChecks(null);
    loadChecks();
    const t = window.setInterval(() => document.visibilityState === "visible" && loadChecks(), 60_000);
    return () => window.clearInterval(t);
  }, [loadChecks]);

  if (!first) return null;

  const failing = (checks ?? []).filter((c) => c.status === "fail");
  const running = (checks ?? []).some((c) => c.status === "running");
  const passing = (checks ?? []).length > 0 && (checks ?? []).every((c) => c.status === "pass" || c.status === "none");
  const openCount = prs.filter((p) => !p.isDraft).length;
  const reviewFailing = () => {
    const f = failing[0];
    if (!f) return;
    const key = `${f.repo}/${f.prNumber}`;
    setExpanded(key);
    requestAnimationFrame(() => document.getElementById(`ft-pr-${key}`)?.scrollIntoView({ behavior: "smooth", block: "nearest" }));
  };
  const checkoutFirst = "Check out the feature first: this needs the code on your machine";
  // The workspace page's delivery flow; the local steps wait for a checkout.
  const steps: Step[] = [
    {
      key: "changes",
      icon: <CommitIcon size={15} />,
      title: "Changes",
      state: "idle",
      detail: workspace ? "See the workspace" : "Check out to see local changes",
    },
    {
      key: "push",
      icon: <PushIcon size={15} />,
      title: "Push",
      state: "idle",
      detail: workspace ? "See the workspace" : "Check out to commit and push",
    },
    {
      key: "prs",
      icon: <PullRequestIcon size={14} />,
      title: "Pull requests",
      state: "done",
      detail: `${openCount} open${prs.length > openCount ? `, ${prs.length - openCount} draft` : ""}`,
      action: {
        label: "View",
        onClick: () => document.getElementById(`ft-prs-${first.branch}`)?.scrollIntoView({ behavior: "smooth" }),
      },
    },
    {
      key: "checks",
      icon: <CheckIcon size={14} />,
      title: "Checks",
      state: checks === null ? "loading" : failing.length ? "fail" : running ? "running" : passing ? "done" : "idle",
      detail:
        checks === null
          ? "Checking CI…"
          : failing.length
            ? "Some checks failed"
            : running
              ? "Running…"
              : passing
                ? "All checks passed"
                : "No checks yet",
      action: failing.length ? { label: "Investigate all", onClick: () => undefined, disabled: checkoutFirst } : undefined,
      extra: failing.length ? { label: "Review", onClick: reviewFailing } : undefined,
    },
  ];
  const nextStep = steps.findIndex((s) => s.state === "fail");

  return (
    <div className="page ws-detail">
      <header className="ws-head">
        <div className="ws-head-main">
          <h2>{multi ? first.branch : first.title}</h2>
          <div className="ws-head-meta">
            <span className="ws-branch">
              <GitIcon size={12} /> {first.branch}
            </span>
            <span className="ws-meta-sep">into</span>
            <span className="ws-meta-muted">{first.base}</span>
            <span className="ws-meta-sep">·</span>
            <span className="ws-meta-muted">
              {prs.length} repo{prs.length === 1 ? "" : "s"}
            </span>
            <span className="ws-meta-sep">·</span>
            <span className="ws-meta-muted">{first.author}</span>
          </div>
        </div>
        <div className="ws-head-tools">
          <button className="secondary ws-tool" onClick={onOpenReview} {...hint("Review the PRs: diffs, threads and verdict, one tab")}>
            <DiffIcon size={14} /> Code Review
          </button>
          {workspace ? (
            <button className="ws-tool" onClick={() => onOpenWorkspace(workspace)} {...hint(`Checked out in workspace ${workspace.name}`)}>
              Open workspace
            </button>
          ) : (
            <button className="ws-tool" onClick={onCheckout} {...hint("Create a local workspace on this branch to build, run and fix it")}>
              Check out
            </button>
          )}
          <span className="ws-tool-sep" />
          <button
            className="icon-button ws-icon-tool"
            disabled={refreshing}
            aria-label="Refresh"
            {...hint("Refresh checks")}
            onClick={loadChecks}
          >
            {refreshing ? <span className="spinner" /> : <RefreshIcon size={15} />}
          </button>
        </div>
      </header>

      {!workspace && (
        <div className="feature-banner">
          Not on this machine yet. <strong>Check out</strong> to get the full workspace: build, agents, commits and AI fixes for failing
          checks.
        </div>
      )}

      <div className="flow">
        {steps.map((s, i) => (
          <div key={s.key} className={`flow-step flow-${s.state} ${i === nextStep ? "flow-next" : ""}`}>
            <div className="flow-step-top">
              <span className="flow-icon">
                {s.state === "done" ? (
                  <CheckIcon size={13} />
                ) : s.state === "fail" ? (
                  <XIcon size={12} />
                ) : s.state === "running" || s.state === "loading" ? (
                  <span className="spinner" />
                ) : (
                  s.icon
                )}
              </span>
              <span className="flow-title">{s.title}</span>
              {i < steps.length - 1 && <span className="flow-arrow">→</span>}
            </div>
            <div className="flow-detail">{s.detail}</div>
            {s.action && (
              <div className="flow-actions">
                {/* A disabled button swallows hover: the hint sits on a wrapper. */}
                <span {...(s.action.disabled ? hint(s.action.disabled) : {})}>
                  <button
                    className={`flow-btn ${i !== nextStep || s.action.disabled ? "secondary" : ""}`}
                    disabled={!!s.action.disabled}
                    onClick={s.action.onClick}
                  >
                    {s.action.label}
                  </button>
                </span>
                {s.extra && (
                  <button className="flow-btn secondary" onClick={s.extra.onClick}>
                    {s.extra.label}
                  </button>
                )}
              </div>
            )}
          </div>
        ))}
      </div>

      <section className="ws-section">
        <div className="ws-section-head">
          <h3>Repositories</h3>
        </div>
        <div className="repo-list ws-repos">
          {prs.map((pr) => {
            const d = details[pr.repo];
            return (
              <div
                key={pr.repo}
                className="repo-row ws-repo-row"
                role="button"
                tabIndex={0}
                onClick={onOpenReview}
                onKeyDown={(e) => e.key === "Enter" && onOpenReview()}
                {...hint("Open in Code Review")}
              >
                <span className="repo-icon">
                  <GitIcon size={15} />
                </span>
                <div className="repo-main">
                  <span className="repo-name">{pr.repo}</span>
                  <span className="repo-sub">{pr.branch}</span>
                </div>
                <div className="ws-repo-badges">
                  {d === undefined ? (
                    <Skeleton w={90} h={10} />
                  ) : (
                    d && (
                      <span className="ws-feature-size">
                        <span className="git-stat-add">+{d.additions}</span>
                        <span className="git-stat-del">−{d.deletions}</span>
                        <span>
                          {d.files.length} file{d.files.length === 1 ? "" : "s"}
                        </span>
                      </span>
                    )
                  )}
                  <span className="ws-badge ws-badge-info">#{pr.number}</span>
                  {workspace ? (
                    <span className="ws-badge ws-badge-ok">
                      <span className="dash-repo-dot" /> Checked out
                    </span>
                  ) : (
                    <span className="ws-badge" {...hint("Only on GitHub — Check out to work on it locally")}>
                      Not checked out
                    </span>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      </section>

      <section className="ws-section" id={`ft-prs-${first.branch}`}>
        <div className="ws-section-head">
          <h3>
            Pull requests <span className="section-count">{prs.length}</span>
          </h3>
        </div>
        <div className="pr-list">
          {prs.map((pr) => {
            const key = `${pr.repo}/${pr.number}`;
            const wsCheck = (checks ?? []).find((c) => c.repo === pr.repo && c.prNumber === pr.number);
            const open = expanded === key;
            const state = pr.isDraft ? "draft" : "open";
            const total = wsCheck?.checks.length ?? 0;
            const passed = wsCheck?.checks.filter((c) => c.bucket === "pass").length ?? 0;
            const decision = pr.status?.reviewDecision ? DECISION[pr.status.reviewDecision] : null;
            return (
              <div key={key} id={`ft-pr-${key}`} className={`pr-item ${open ? "open" : ""}`}>
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
                      {pr.author}
                    </span>
                  </div>
                  {decision && <span className={`cr-status cr-status-${decision[1]}`}>{decision[0]}</span>}
                  <span className="pr-checks">
                    <CheckDot status={checks === null ? "running" : (wsCheck?.status ?? "none")} />
                    {checks === null ? "Loading checks…" : total > 0 ? `${passed}/${total} checks` : "No checks"}
                  </span>
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
                  />
                )}
              </div>
            );
          })}
        </div>
      </section>
    </div>
  );
}
