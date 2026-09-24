import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrGroup, PullRequest, Workspace } from "../types/config";
import { ContextMenu, MenuItem, useContextMenu } from "../components/ContextMenu";
import { workspaceForPrs } from "../components/PrCheckoutModal";
import { Skeleton } from "../components/Skeleton";
import { toast } from "../components/Toast";
import { tooltip } from "../components/Tooltip";

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

/**
 * Code Review home: open PRs from every configured repo (last 7 days),
 * grouped by identical branch name — multi-repo features surface as one
 * card. Search covers all PRs regardless of age or state.
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

  const display = query.trim() ? searchResults : groups;

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
        <h2>Code Review</h2>
        <button
          className="secondary"
          disabled={refreshing}
          onMouseEnter={(e) => tooltip.show("Reload open PRs from the last 7 days", e)}
          onMouseLeave={() => tooltip.hide()}
          onClick={() => load(true)}
        >
          {refreshing ? "Refreshing…" : "↻ Refresh"}
        </button>
      </div>

      <div className="pr-search-row">
        <input
          value={query}
          placeholder="Search PRs by title, branch or author… (any age, any state)"
          onChange={(e) => setQuery(e.target.value)}
          autoComplete="off"
          autoCorrect="off"
          autoCapitalize="off"
          spellCheck={false}
        />
        {query.trim() && (
          <span className="tag tag-muted">
            {searching ? "searching…" : `${display?.length ?? 0} groups`}
          </span>
        )}
      </div>

      {display === null ? (
        <div className="workspace-grid">
          {Array.from({ length: 3 }).map((_, i) => (
            <div className="skeleton-card" key={i}>
              <Skeleton w="55%" h={14} />
              <Skeleton w="75%" h={10} />
              <Skeleton w="35%" h={10} />
            </div>
          ))}
        </div>
      ) : display.length === 0 ? (
        <div className="empty-state">
          <div className="empty-state-icon">🔍</div>
          <h3>
            {query.trim() ? "No PRs match your search" : "No open PRs in the last 7 days"}
          </h3>
          <p>
            {query.trim()
              ? "Try another term — search covers every configured repository."
              : "PRs updated recently will appear here. Use the search above to find older or merged PRs."}
          </p>
        </div>
      ) : (
        <div className="pr-grid">
          {display.map((g) => {
            const multi = g.prs.length > 1;
            const first = g.prs[0];
            const ws = workspaceForPrs(g.prs, workspaces);
            return (
              <div
                key={g.branch + first.repo + first.number}
                className={`pr-card ${multi ? "pr-card-multi" : ""}`}
                onClick={() => onOpenPr(g.prs)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  tooltip.hide();
                  setMenu({ x: e.clientX, y: e.clientY, payload: g });
                }}
                onMouseDown={(e) => openFromEvent(e, g)}
                onMouseEnter={(e) =>
                  tooltip.show(
                    multi
                      ? `Feature across ${g.prs.length} repos — click to review`
                      : `Review ${first.title}`,
                    e
                  )
                }
                onMouseLeave={() => tooltip.hide()}
              >
                <div className="pr-card-head">
                  <span className="pr-card-title">{multi ? g.branch : first.title}</span>
                  {multi && <span className="tag tag-info">{g.prs.length} repos</span>}
                </div>
                <div className="pr-card-body">
                  {multi ? (
                    <div className="pr-card-repos">
                      {g.prs.map((pr) => (
                        <span key={pr.repo} className="pr-repo-chip">
                          <span className="pr-repo-name">{pr.repo}</span>
                          <span className="pr-repo-num">#{pr.number}</span>
                        </span>
                      ))}
                    </div>
                  ) : (
                    <>
                      <span className="mono pr-card-branch">{first.branch}</span>
                      <span className="pr-card-meta">
                        {first.repo} #{first.number} · {first.author} · {relTime(first.updatedAt)}
                      </span>
                    </>
                  )}
                </div>
                <div className="pr-card-foot">
                  {first.isDraft && <span className="tag tag-warn">draft</span>}
                  <button
                    className="link"
                    onClick={(e) => {
                      e.stopPropagation();
                      tooltip.hide();
                      if (ws) onOpenWorkspace(ws);
                      else onCheckout(g.prs);
                    }}
                    onMouseEnter={(e) =>
                      tooltip.show(ws ? `Already checked out in workspace ${ws.name}` : "Create a local workspace on this branch to run it", e)
                    }
                    onMouseLeave={() => tooltip.hide()}
                  >
                    {ws ? "open workspace" : "check out"}
                  </button>
                  <button
                    className="link"
                    onClick={(e) => {
                      e.stopPropagation();
                      openUrl(first.url).catch(() => null);
                    }}
                    onMouseEnter={(e) => tooltip.show("Open on GitHub ↗", e)}
                    onMouseLeave={() => tooltip.hide()}
                  >
                    open ↗
                  </button>
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