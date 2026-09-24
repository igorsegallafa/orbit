import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrGroup, PrReviewStatus, PullRequest, Workspace } from "../types/config";
import { ContextMenu, MenuItem, useContextMenu } from "../components/ContextMenu";
import { workspaceForPrs } from "../components/PrCheckoutModal";
import { Skeleton } from "../components/Skeleton";
import { toast } from "../components/Toast";
import { tooltip } from "../components/Tooltip";
import { CheckIcon, PullRequestIcon, RefreshIcon, SearchIcon, XIcon } from "../components/Icons";

interface Props {
  /** Opens a PR review tab. When a group has several PRs (multi-repo
   *  feature), the tab opens on the first PR and offers a chip selector. */
  onOpenPr: (prs: PullRequest[]) => void;
  workspaces: Workspace[];
  /** Check the PRs out as a local workspace (opens the checkout modal). */
  onCheckout: (prs: PullRequest[]) => void;
  onOpenWorkspace: (ws: Workspace) => void;
  onError: (msg: string) => void;
}

function relTime(iso: string): string {
  const s = Math.max(0, Date.now() - new Date(iso).getTime()) / 1000;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86_400)}d ago`;
}

type Filter = "all" | "review" | "reviewed" | "mine";

const FILTERS: { key: Filter; label: string; hint: string }[] = [
  { key: "all", label: "All", hint: "Every open PR" },
  { key: "review", label: "To review", hint: "Not reviewed by you yet, or new commits since your review" },
  { key: "reviewed", label: "Reviewed", hint: "You reviewed the latest commits" },
  { key: "mine", label: "Mine", hint: "PRs you opened" },
];

interface GroupStatus {
  decision: PrReviewStatus["reviewDecision"];
  checks: PrReviewStatus["checks"];
  conflicts: boolean;
  mine: boolean;
  draft: boolean;
  /** The viewer's review across the group (worst wins). */
  myReview: PrReviewStatus["myReview"];
  reviewed: number;
  total: number;
  /** New commits after the viewer's review, in any of the PRs. */
  stale: boolean;
}

/** A group's PRs read as one (multi-repo features): the worst state wins.
 *  null when the list carried no status (search results). */
function groupStatus(prs: PullRequest[]): GroupStatus | null {
  const st = prs.map((p) => p.status).filter((s): s is PrReviewStatus => !!s);
  if (!st.length) return null;
  const reviewed = st.filter((s) => s.myReview && s.myReview !== "DISMISSED");
  return {
    decision: st.some((s) => s.reviewDecision === "CHANGES_REQUESTED")
      ? "CHANGES_REQUESTED"
      : st.every((s) => s.reviewDecision === "APPROVED")
        ? "APPROVED"
        : st.some((s) => s.reviewDecision === "REVIEW_REQUIRED")
          ? "REVIEW_REQUIRED"
          : null,
    checks: st.some((s) => s.checks === "fail")
      ? "fail"
      : st.some((s) => s.checks === "pending")
        ? "pending"
        : st.some((s) => s.checks === "pass")
          ? "pass"
          : null,
    conflicts: st.some((s) => s.conflicts),
    mine: st.some((s) => s.mine),
    draft: prs.every((p) => p.isDraft),
    myReview: !reviewed.length
      ? null
      : reviewed.some((s) => s.myReview === "CHANGES_REQUESTED")
        ? "CHANGES_REQUESTED"
        : reviewed.every((s) => s.myReview === "APPROVED")
          ? "APPROVED"
          : "COMMENTED",
    reviewed: reviewed.length,
    total: st.length,
    stale: reviewed.some((s) => s.myReviewStale),
  };
}

function matches(f: Filter, s: GroupStatus | null): boolean {
  if (f === "all") return true;
  if (!s) return false;
  const toReview = !s.mine && (s.reviewed < s.total || s.stale);
  if (f === "review") return toReview;
  if (f === "reviewed") return !s.mine && !toReview;
  return s.mine;
}

const MY_REVIEW: Record<string, [string, string]> = {
  APPROVED: ["You approved", "ok"],
  CHANGES_REQUESTED: ["You requested changes", "bad"],
  COMMENTED: ["You commented", "muted"],
};

/** The one state worth a pill on a row, most urgent first; what it hides
 *  goes in its tooltip. null when there's nothing to say yet. */
function primaryState(s: GroupStatus): { label: string; tone: string; tip: string } | null {
  const ready = s.decision === "APPROVED" && s.checks !== "fail" && s.checks !== "pending" && !s.conflicts && !s.draft;
  const partial = s.total > 1 && s.reviewed < s.total ? ` (${s.reviewed} of ${s.total} PRs)` : "";
  const mine = s.myReview ? MY_REVIEW[s.myReview] : null;
  if (s.conflicts) return { label: "Conflicts", tone: "bad", tip: "Conflicts with the base branch" };
  if (s.stale)
    return { label: "New commits", tone: "warn", tip: `${mine?.[0] ?? "You reviewed"}${partial}; commits were pushed since` };
  if (ready) return { label: "Ready to merge", tone: "ok", tip: "Approved, checks passing, no conflicts" };
  if (s.mine) {
    if (s.decision === "CHANGES_REQUESTED") return { label: "Changes requested", tone: "bad", tip: "A reviewer asked for changes on your PR" };
    if (s.decision === "APPROVED") return { label: "Approved", tone: "ok", tip: "Your PR is approved" };
    return { label: "Your PR", tone: "info", tip: "You opened this PR" };
  }
  if (mine) return { label: mine[0], tone: mine[1], tip: `Your latest review${partial}` };
  if (s.decision === "CHANGES_REQUESTED") return { label: "Changes requested", tone: "bad", tip: "Another reviewer asked for changes" };
  if (s.decision === "APPROVED") return { label: "Approved", tone: "ok", tip: "Approved by another reviewer" };
  return null;
}

const tip = (text: string) => ({
  onMouseEnter: (e: React.MouseEvent) => {
    e.stopPropagation();
    tooltip.show(text, e);
  },
  onMouseLeave: () => tooltip.hide(),
});

/** CI as a single glyph: check, cross, or a pulsing dot while running. */
function ChecksGlyph({ checks }: { checks: GroupStatus["checks"] }) {
  if (!checks) return <span className="cr-row-checks" />;
  const text = checks === "pass" ? "Checks passing" : checks === "fail" ? "Checks failing" : "Checks running";
  return (
    <span className={`cr-row-checks cr-row-checks-${checks}`} {...tip(text)}>
      {checks === "pass" ? <CheckIcon size={13} /> : checks === "fail" ? <XIcon size={13} /> : <span className="cr-row-pending" />}
    </span>
  );
}

/**
 * Code Review home: open PRs from every configured repo (last 7 days),
 * grouped by identical branch name — multi-repo features surface as one
 * row. Search covers all PRs regardless of age or state.
 */
export function CodeReviewPage({ onOpenPr, workspaces, onCheckout, onOpenWorkspace, onError }: Props) {
  const [groups, setGroups] = useState<PrGroup[] | null>(null);
  const { menu, setMenu, openFromEvent } = useContextMenu<PrGroup>();
  const [refreshing, setRefreshing] = useState(false);
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [searchResults, setSearchResults] = useState<PrGroup[] | null>(null);

  const load = useCallback(
    async (force: boolean) => {
      setRefreshing(true);
      try {
        setGroups(await invoke<PrGroup[]>("pr_list", { forceRefresh: force }));
      } catch (e) {
        onError(String(e));
      } finally {
        setRefreshing(false);
      }
    },
    [onError]
  );

  useEffect(() => {
    load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Debounced search over all PRs (any state/age)
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setSearchResults(null);
      setSearching(false);
      return;
    }
    setSearching(true);
    const t = window.setTimeout(async () => {
      try {
        setSearchResults(await invoke<PrGroup[]>("pr_search", { query: q }));
      } catch (e) {
        onError(String(e));
      } finally {
        setSearching(false);
      }
    }, 400);
    return () => window.clearTimeout(t);
  }, [query, onError]);

  const [filter, setFilter] = useState<Filter>("all");
  const searchingNow = !!query.trim();
  // Search results carry no review state: the filter applies to the list only.
  const display = searchingNow ? searchResults : groups && groups.filter((g) => matches(filter, groupStatus(g.prs)));
  const counts = Object.fromEntries(
    FILTERS.map((f) => [f.key, (groups ?? []).filter((g) => matches(f.key, groupStatus(g.prs))).length]),
  ) as Record<Filter, number>;

  const menuItems = (g: PrGroup): MenuItem[] => {
    const ws = workspaceForPrs(g.prs, workspaces);
    return [
      { label: "Review", onSelect: () => onOpenPr(g.prs) },
      ws
        ? { label: `Open workspace ${ws.name}`, onSelect: () => onOpenWorkspace(ws) }
        : { label: "Check out as workspace…", onSelect: () => onCheckout(g.prs) },
      { label: "Open on GitHub ↗", onSelect: () => openUrl(g.prs[0].url).catch(() => null) },
      {
        label: "Copy branch name",
        onSelect: () =>
          navigator.clipboard
            .writeText(g.branch)
            .then(() => toast.success("Branch name copied", { description: g.branch }))
            .catch(() => null),
      },
    ];
  };

  return (
    <div className="page">
      <div className="page-header">
        <div className="page-title">
          <h2>Code Review</h2>
          <span className="page-sub">Open PRs across your repositories from the last 7 days</span>
        </div>
        <button
          className="icon-button"
          aria-label="Refresh"
          disabled={refreshing}
          {...tip("Reload open PRs from the last 7 days")}
          onClick={() => load(true)}
        >
          <span className={refreshing ? "spin" : ""} style={{ display: "inline-flex" }}>
            <RefreshIcon size={14} />
          </span>
        </button>
      </div>

      <div className="cr-toolbar">
        {!searchingNow && groups && groups.length > 0 && (
          <div className="cr-segments" role="tablist">
            {FILTERS.map((f) => (
              <button
                key={f.key}
                role="tab"
                aria-selected={filter === f.key}
                className={`btn-plain cr-segment ${filter === f.key ? "on" : ""}`}
                onClick={() => setFilter(f.key)}
                {...tip(f.hint)}
              >
                {f.label}
                <span className="cr-segment-count">{counts[f.key]}</span>
              </button>
            ))}
          </div>
        )}
        <label className="cr-search">
          <SearchIcon size={14} />
          <input
            value={query}
            placeholder="Search any PR by title, branch or author"
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
          />
          {searchingNow && (
            <span className="cr-search-count">{searching ? "searching…" : `${display?.length ?? 0} found`}</span>
          )}
        </label>
      </div>

      {display === null ? (
        <div className="cr-rows">
          {Array.from({ length: 5 }).map((_, i) => (
            <div className="cr-row cr-row-skeleton" key={i}>
              <Skeleton w={16} h={16} />
              <div className="cr-row-main">
                <Skeleton w="45%" h={12} />
                <Skeleton w="30%" h={10} />
              </div>
            </div>
          ))}
        </div>
      ) : display.length === 0 ? (
        <div className="empty-state">
          <div className="empty-state-icon">🔍</div>
          <h3>
            {searchingNow
              ? "No PRs match your search"
              : filter !== "all"
                ? `No PRs in "${FILTERS.find((f) => f.key === filter)!.label}"`
                : "No open PRs in the last 7 days"}
          </h3>
          <p>
            {searchingNow
              ? "Try another term — search covers every configured repository."
              : filter !== "all"
                ? "Open PRs from the last 7 days that match this filter show up here."
                : "PRs updated recently will appear here. Use the search above to find older or merged PRs."}
          </p>
        </div>
      ) : (
        <div className="cr-rows">
          {display.map((g, i) => {
            const multi = g.prs.length > 1;
            const first = g.prs[0];
            const ws = workspaceForPrs(g.prs, workspaces);
            const status = groupStatus(g.prs);
            const state = status && primaryState(status);
            const draft = g.prs.every((p) => p.isDraft);
            return (
              <div
                key={g.branch + first.repo + first.number}
                className={`cr-row ${draft ? "cr-row-draft" : ""}`}
                style={{ animationDelay: `${Math.min(i, 12) * 18}ms` }}
                role="button"
                tabIndex={0}
                onClick={() => onOpenPr(g.prs)}
                onKeyDown={(e) => e.key === "Enter" && onOpenPr(g.prs)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  tooltip.hide();
                  setMenu({ x: e.clientX, y: e.clientY, payload: g });
                }}
                onMouseDown={(e) => openFromEvent(e, g)}
              >
                <span className="cr-row-icon" {...tip(draft ? "Draft" : multi ? `Feature across ${g.prs.length} repos` : "Open")}>
                  <PullRequestIcon size={15} />
                  {multi && <span className="cr-row-multi">{g.prs.length}</span>}
                </span>
                <div className="cr-row-main">
                  <div className="cr-row-title">
                    <span className="cr-row-title-text">{multi ? g.branch : first.title}</span>
                    {draft && <span className="cr-row-draft-tag">Draft</span>}
                  </div>
                  <div className="cr-row-meta">
                    {multi ? (
                      g.prs.map((pr) => (
                        <span key={pr.repo} className="cr-row-repo">
                          {pr.repo} <span className="cr-row-num">#{pr.number}</span>
                        </span>
                      ))
                    ) : (
                      <>
                        <span className="cr-row-repo">
                          {first.repo} <span className="cr-row-num">#{first.number}</span>
                        </span>
                        <span>{first.author}</span>
                        <span className="mono cr-row-branch">{first.branch}</span>
                      </>
                    )}
                  </div>
                </div>
                <div className="cr-row-side">
                  {state && (
                    <span className={`cr-status cr-status-${state.tone}`} {...tip(state.tip)}>
                      {state.label}
                    </span>
                  )}
                  <ChecksGlyph checks={status?.checks ?? null} />
                  <span className="cr-row-time">{relTime(first.updatedAt)}</span>
                  <div className="cr-row-actions">
                    <button
                      className="btn-mini secondary"
                      onClick={(e) => {
                        e.stopPropagation();
                        tooltip.hide();
                        if (ws) onOpenWorkspace(ws);
                        else onCheckout(g.prs);
                      }}
                      {...tip(ws ? `Already checked out in workspace ${ws.name}` : "Create a local workspace on this branch to run it")}
                    >
                      {ws ? "Workspace" : "Check out"}
                    </button>
                    <button
                      className="btn-mini secondary"
                      aria-label="Open on GitHub"
                      onClick={(e) => {
                        e.stopPropagation();
                        openUrl(first.url).catch(() => null);
                      }}
                      {...tip("Open on GitHub")}
                    >
                      ↗
                    </button>
                  </div>
                </div>
              </div>
            );
          })}
        </div>
      )}
      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.payload)} onClose={() => setMenu(null)} />}
    </div>
  );
}
