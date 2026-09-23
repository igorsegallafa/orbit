import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { PullRequest } from "../types/config";
import { CheckBox } from "./CheckBox";
import { Skeleton } from "./Skeleton";
import { tooltip } from "./Tooltip";
import { SparkIcon } from "./Icons";

interface Draft {
  title: string;
  body: string;
}

type RepoStatus =
  | "checking" // looking for existing PRs
  | "has-pr" // PR already open
  | "ready" // needs PR: branch ahead, content editable
  | "nothing" // nothing to PR (no commits vs base)
  | "drafting" // AI is writing title/description
  | "draft-error"
  | "creating"
  | "created"
  | "error";

interface RepoRow {
  repo: string;
  status: RepoStatus;
  prUrl?: string;
  draft: Draft;
  error?: string;
  included: boolean;
  /** When the current drafting/creating attempt began (elapsed display). */
  startedAt?: number;
}

interface Props {
  workspace: string;
  base: string;
  /** Repos of the workspace with their PR-worthy commit counts. */
  repos: { repo: string; prCommits: number }[];
  /** Fallback PR title (card title or branch). */
  defaultTitle: string;
  onOpenPrs: (prs: PullRequest[]) => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

/** Pull-request modal: detects which repos already have PRs and which
 *  need one. Existing PRs open in the review tab; missing ones get an
 *  editable title + description — drafted by the AI from the repo's own
 *  pull_request_template — then created in parallel. */
export function PrsModal({ workspace, base, repos, defaultTitle, onOpenPrs, onClose, onError }: Props) {
  const [rows, setRows] = useState<RepoRow[]>(() =>
    repos.map((r) => ({
      repo: r.repo,
      status: "checking",
      draft: { title: defaultTitle, body: "" },
      included: true,
    }))
  );
  const [creating, setCreating] = useState(false);
  const [now, setNow] = useState(Date.now());

  // Ticks while anything drafts so the card can show elapsed seconds —
  // proof the request is alive (agents take 15-60s exploring the repo).
  useEffect(() => {
    if (!rows.some((r) => r.status === "drafting")) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [rows]);

  const set = (repo: string, patch: Partial<RepoRow>) =>
    setRows((prev) => prev.map((r) => (r.repo === repo ? { ...r, ...patch } : r)));

  // On open: find existing PRs — ONE invoke for the whole workspace (the
  // backend scans every repo in parallel). A PR candidate is a branch
  // whose remote has commits the base lacks (prCommits > 0) — a pushed
  // branch with no content of its own has nothing to PR.
  useEffect(() => {
    let cancelled = false;
    const candidates = repos.filter((r) => r.prCommits > 0).map((r) => r.repo);
    repos.forEach((r) => {
      if (r.prCommits === 0) set(r.repo, { status: "nothing", included: false });
    });
    if (candidates.length > 0) {
      invoke<PullRequest[]>("ws_prs_flat", { workspace })
        .then((prs) => {
          if (cancelled) return;
          candidates.forEach((repo) => {
            const mine = prs.find((p) => p.repo === repo);
            if (mine) {
              set(repo, { status: "has-pr", prUrl: mine.url, included: false });
            } else {
              set(repo, { status: "ready" });
            }
          });
        })
        .catch(() => {
          if (!cancelled) candidates.forEach((repo) => set(repo, { status: "ready" }));
        });
    }
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace]);

  const draftWithAi = (repo: string) => {
    // Clear any stale error/draft from a previous attempt before running.
    set(repo, { status: "drafting", error: undefined, included: true, startedAt: Date.now() });
    invoke<{ title: string; body: string }>("ws_pr_draft", { workspace, repo })
      .then((d) =>
        set(repo, { status: "ready", draft: { title: d.title, body: d.body } })
      )
      .catch((e) => {
        set(repo, { status: "draft-error", error: String(e) });
        onError(`${repo}: ${String(e)}`);
      });
  };

  const create = async () => {
    setCreating(true);
    const specs = rows
      .filter((r) => r.included && r.status === "ready")
      .map((r) => ({ repo: r.repo, title: r.draft.title, body: r.draft.body }));
    if (specs.length === 0) {
      setCreating(false);
      return;
    }
    specs.forEach((s) => set(s.repo, { status: "creating" }));
    try {
      const prs = await invoke<PullRequest[]>("ws_create_prs", { workspace, base, specs });
      setCreating(false);
      onOpenPrs(prs);
    } catch (e) {
      specs.forEach((s) => set(s.repo, { status: "error", error: String(e) }));
      setCreating(false);
      onError(String(e));
    }
  };

  const existing = rows.filter((r) => r.status === "has-pr");
  // Cards stay visible through the whole lifecycle — drafting/creating rows
  // render their own skeleton/spinner. Dropping them here made the modal
  // collapse to the "Nothing to PR" empty state while the AI worked.
  const targets = rows.filter(
    (r) =>
      r.status === "ready" ||
      r.status === "drafting" ||
      r.status === "draft-error" ||
      r.status === "creating" ||
      r.status === "error"
  );
  const readyToCreate = targets.filter((r) => r.included);
  const anyDrafting = rows.some((r) => r.status === "drafting");
  const checking = rows.some((r) => r.status === "checking");
  const nothingRow = rows.find((r) => r.status === "nothing");

  return (
    <div className="modal-overlay" onMouseDown={creating ? undefined : onClose}>
      <div className="modal ws-action-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Pull requests for this feature</h3>

          {checking && (
            <div className="ws-action-list">
              {rows.map((r) => (
                <div key={r.repo} className="ws-commit-card">
                  <div className="ws-commit-head">
                    <span className="mono">{r.repo}</span>
                    <span className="ws-commit-state">
                      <span className="spinner" /> looking for PRs…
                    </span>
                  </div>
                  <div className="ws-pr-skeleton">
                    <Skeleton w="70%" h={26} />
                    <Skeleton w="100%" h={11} />
                    <Skeleton w="90%" h={11} />
                    <Skeleton w="60%" h={11} />
                  </div>
                </div>
              ))}
            </div>
          )}

          {!checking && existing.length > 0 && (
            <div className="ws-prs-existing">
              {existing.map((r) => (
                <a
                  key={r.repo}
                  className="ws-prs-existing-row"
                  href={r.prUrl}
                  onClick={(e) => {
                    e.preventDefault();
                    import("@tauri-apps/plugin-opener")
                      .then(({ openUrl }) => openUrl(r.prUrl!))
                      .catch(() => null);
                  }}
                >
                  <span className="mono">{r.repo}</span> — PR already open ↗
                </a>
              ))}
            </div>
          )}

          {!checking && !anyDrafting && targets.length === 0 && existing.length === 0 && (
            <p className="ws-commit-placeholder">
              {nothingRow
                ? "Nothing to PR — the feature branches have no commits beyond the base."
                : "Every repo already has its pull request."}
            </p>
          )}

          {!checking && targets.length > 0 && (
            <div className="ws-action-list">
              {targets.map((r) => (
                <div key={r.repo} className={`ws-commit-card ws-pr-card ${r.status === "creating" ? "ws-creating" : ""}`}>
                  <div className="ws-commit-head">
                    <CheckBox
                      checked={r.included}
                      disabled={creating}
                      onChange={(c) => set(r.repo, { included: c })}
                      label={`Create PR for ${r.repo}`}
                    />
                    <span className="mono">{r.repo}</span>
                    <span className="ws-commit-state">
                      {r.status === "drafting" && (
                        <>
                          <span className="spinner" />{" "}
                          {r.startedAt
                            ? `${Math.floor((now - r.startedAt) / 1000)}s — AI is reading the template and the diff…`
                            : "AI is reading the template and the diff…"}
                        </>
                      )}
                      {r.status === "draft-error" && <span className="ws-dot ws-dot-err">✗ draft failed</span>}
                      {r.status === "creating" && (
                        <>
                          <span className="spinner" /> creating PR…
                        </>
                      )}
                    </span>
                    <button
                      type="button"
                      className="btn-mini"
                      disabled={r.status === "drafting" || r.status === "creating" || creating}
                      onClick={() => draftWithAi(r.repo)}
                      onMouseEnter={(e) =>
                        tooltip.show("AI reads the repo's PR template and the diff, drafts title + description", e)
                      }
                      onMouseLeave={() => tooltip.hide()}
                    >
                      <SparkIcon size={11} /> Draft with AI
                    </button>
                  </div>
                  {r.status === "drafting" ? (
                    <div className="ws-pr-skeleton">
                      <Skeleton w="60%" h={26} />
                      <Skeleton w="100%" h={11} />
                      <Skeleton w="92%" h={11} />
                      <Skeleton w="70%" h={11} />
                    </div>
                  ) : (
                    <>
                      <input
                        className="ws-pr-title"
                        placeholder="PR title"
                        value={r.draft.title}
                        disabled={creating || r.status === "creating"}
                        onChange={(e) =>
                          set(r.repo, { draft: { ...r.draft, title: e.target.value } })
                        }
                      />
                      <textarea
                        className="ws-pr-body"
                        placeholder="Description — what, why and how (short, objective)"
                        value={r.draft.body}
                        disabled={creating || r.status === "creating"}
                        onChange={(e) =>
                          set(r.repo, { draft: { ...r.draft, body: e.target.value } })
                        }
                      />
                    </>
                  )}
                  {r.error && <div className="ws-commit-error">{r.error}</div>}
                </div>
              ))}
            </div>
          )}

          {!checking && !anyDrafting && existing.length > 0 && targets.length === 0 && (
            <p className="ws-action-ok">✓ every repo already has its PR</p>
          )}
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose} disabled={creating}>
            Close
          </button>
          {checking && (
            <button type="button" className="secondary" disabled>
              <span className="spinner" /> Checking PRs…
            </button>
          )}
          {!checking && anyDrafting && (
            <button type="button" className="secondary" disabled>
              <span className="spinner" /> AI is drafting…
            </button>
          )}
          {!checking && !anyDrafting && targets.length > 0 && (
            <button
              type="button"
              autoFocus
              disabled={creating || readyToCreate.length === 0}
              onClick={create}
            >
              {creating
                ? "Creating…"
                : `Create ${readyToCreate.length} PR${readyToCreate.length === 1 ? "" : "s"}`}
            </button>
          )}
          {!checking && !anyDrafting && existing.length > 0 && targets.length === 0 && (
            <button
              type="button"
              autoFocus
              onClick={() =>
                invoke<PullRequest[]>("ws_prs_flat", { workspace }).then((prs) => {
                  onOpenPrs(prs);
                  onClose();
                })
              }
            >
              Open in review
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
