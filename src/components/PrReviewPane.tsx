import { useCallback, useEffect, useMemo, useState } from "react";
import { DiffEditor, type BeforeMount } from "@monaco-editor/react";
import type { editor as Monaco } from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrDetail, PrFileDiff, PullRequest } from "../types/config";
import { Draft, ReviewComment, ReviewData, ReviewEvent, ReviewThread, Side, timeAgo } from "../types/review";
import { Skeleton } from "./Skeleton";
import { tooltip } from "./Tooltip";
import { toast } from "./Toast";
import { useDiffNav } from "./useDiffNav";
import { RangeMark, ZoneItem, useReviewZones } from "./useReviewZones";
import { Composer, DraftCard, ThreadActions, ThreadCard } from "./ReviewThreads";
import { SubmitReview } from "./SubmitReview";
import { SafeMarkdown } from "./SafeMarkdown";
import { CheckBox } from "./CheckBox";
import { CheckIcon, ChevronIcon, DocIcon, EyeIcon, RepoIcon } from "./Icons";

interface Props {
  /** All PRs of the feature group (1 for single-repo PRs). */
  prs: PullRequest[];
  onError: (msg: string) => void;
}

let themeDefined = false;
const beforeMount: BeforeMount = (monaco) => {
  if (themeDefined) return;
  themeDefined = true;
  monaco.editor.defineTheme("orbit-dark", {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "comment", foreground: "6b7280", fontStyle: "italic" },
      { token: "keyword", foreground: "c792ea" },
      { token: "string", foreground: "a5d6a7" },
      { token: "number", foreground: "f78c6c" },
      { token: "type", foreground: "7aa7ff" },
      { token: "function", foreground: "82aaff" },
    ],
    colors: {
      "editor.background": "#0d0f13",
      "editor.foreground": "#e6e8ec",
      "editorLineNumber.foreground": "#4a5060",
      "editorLineNumber.activeForeground": "#9aa1ad",
      "diffEditor.insertedTextBackground": "#12281a",
      "diffEditor.removedTextBackground": "#2d1416",
      "diffEditor.insertedLineBackground": "#0e2016",
      "diffEditor.removedLineBackground": "#241012",
    },
  });
};

function langOf(path: string): string | undefined {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "ts" || ext === "tsx") return "typescript";
  if (ext === "js" || ext === "jsx") return "javascript";
  if (ext === "rs") return "rust";
  if (ext === "go") return "go";
  if (ext === "py") return "python";
  if (ext === "md") return "markdown";
  if (ext === "json" || ext === "yaml" || ext === "yml" || ext === "toml") return ext;
  return undefined;
}

const OVERVIEW = "\u0000overview";

function useLocal<T>(key: string, initial: T): [T, (v: T | ((p: T) => T)) => void] {
  const [value, setValue] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw ? (JSON.parse(raw) as T) : initial;
    } catch {
      return initial;
    }
  });
  useEffect(() => {
    try {
      const raw = localStorage.getItem(key);
      setValue(raw ? (JSON.parse(raw) as T) : initial);
    } catch {
      setValue(initial);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  const set = useCallback(
    (v: T | ((p: T) => T)) =>
      setValue((prev) => {
        const next = typeof v === "function" ? (v as (p: T) => T)(prev) : v;
        try {
          localStorage.setItem(key, JSON.stringify(next));
        } catch {
          // storage unavailable: state still works for this session
        }
        return next;
      }),
    [key],
  );
  return [value, set];
}

const DECISION: Record<string, { label: string; cls: string }> = {
  APPROVED: { label: "Approved", cls: "ok" },
  CHANGES_REQUESTED: { label: "Changes requested", cls: "bad" },
  REVIEW_REQUIRED: { label: "Review required", cls: "muted" },
};

/**
 * PR review tab: files and overview on the left, diff with inline review
 * threads on the right. Comments can be posted right away or collected in
 * a pending review (kept locally until submitted) and sent with a verdict.
 */
export function PrReviewPane({ prs, onError }: Props) {
  const [activeIdx, setActiveIdx] = useState(0);
  const [detail, setDetail] = useState<PrDetail | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [diff, setDiff] = useState<PrFileDiff | null>(null);
  const [split, setSplit] = useState(false);
  const [review, setReview] = useState<ReviewData | null>(null);
  const [composer, setComposer] = useState<{ side: Side; start: number; end: number } | null>(null);
  const [submitOpen, setSubmitOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const nav = useDiffNav();

  const pr = prs[activeIdx];
  const prKey = pr ? `${pr.ownerRepo}#${pr.number}` : "";
  const [drafts, setDrafts] = useLocal<Draft[]>(`orbit.review.drafts.${prKey}`, []);
  const [viewed, setViewed] = useLocal<string[]>(`orbit.review.viewed.${prKey}`, []);
  const [hideViewed, setHideViewed] = useLocal<boolean>("orbit.review.hideViewed", false);

  const loadReview = useCallback(
    async (withLines: boolean) => {
      if (!pr) return;
      try {
        const data = await invoke<ReviewData>("pr_review_data", { ownerRepo: pr.ownerRepo, number: pr.number, withLines });
        setReview((prev) => (withLines || !prev ? data : { ...data, commentable: prev.commentable }));
      } catch (e) {
        onError(String(e));
      }
    },
    [pr?.ownerRepo, pr?.number, onError],
  );

  useEffect(() => {
    if (!pr) return;
    setDetail(null);
    setSelected(null);
    setReview(null);
    setComposer(null);
    invoke<PrDetail>("pr_detail", { ownerRepo: pr.ownerRepo, number: pr.number })
      .then((d) => {
        setDetail(d);
        setSelected(OVERVIEW);
      })
      .catch((e) => onError(String(e)));
    loadReview(true);
  }, [pr?.ownerRepo, pr?.number, onError, loadReview]);

  useEffect(() => {
    setComposer(null);
    if (!pr || !detail || !selected || selected === OVERVIEW) return;
    setDiff(null);
    invoke<PrFileDiff>("pr_file_diff", { ownerRepo: pr.ownerRepo, headSha: detail.headSha, baseSha: detail.baseSha, path: selected })
      .then(setDiff)
      .catch((e) => {
        setDiff({ original: "", modified: "" });
        onError(String(e));
      });
  }, [pr?.ownerRepo, detail?.headSha, selected, onError]);

  const threadsByPath = useMemo(() => {
    const m = new Map<string, ReviewThread[]>();
    for (const t of review?.threads ?? []) m.set(t.path, [...(m.get(t.path) ?? []), t]);
    return m;
  }, [review]);
  const unresolved = (review?.threads ?? []).filter((t) => !t.isResolved).length;
  const viewedCount = detail ? detail.files.filter((f) => viewed.includes(f.path)).length : 0;
  const ownPr = !!review && review.viewer === review.prAuthor;

  const markViewed = (path: string, on: boolean, advance: boolean) => {
    setViewed((all) => (on ? [...all.filter((p) => p !== path), path] : all.filter((p) => p !== path)));
    if (!on || !advance || !detail) return;
    const files = detail.files;
    const at = files.findIndex((f) => f.path === path);
    const next = [...files.slice(at + 1), ...files.slice(0, at)].find((f) => !viewed.includes(f.path));
    if (next) setSelected(next.path);
  };

  // ---------- actions ----------
  const withToast = async (fn: () => Promise<unknown>, ok?: string) => {
    try {
      await fn();
      if (ok) toast.success(ok);
      await loadReview(false);
    } catch (e) {
      onError(String(e));
      throw e;
    }
  };

  const actions: ThreadActions = {
    reply: (t, body) => withToast(() => invoke("pr_reply", { ownerRepo: pr.ownerRepo, number: pr.number, commentId: t.comments[0].id, body })),
    resolve: (t, resolved) => withToast(() => invoke("pr_resolve_thread", { threadId: t.id, resolved })),
    edit: (c: ReviewComment, body) => withToast(() => invoke("pr_edit_comment", { ownerRepo: pr.ownerRepo, commentId: c.id, body })),
    remove: (c: ReviewComment) => withToast(() => invoke("pr_delete_comment", { ownerRepo: pr.ownerRepo, commentId: c.id }), "Comment deleted"),
  };

  const commentNow = async (side: Side, start: number, end: number, body: string) => {
    if (!detail || !selected) return;
    await withToast(
      () =>
        invoke("pr_add_comment", {
          ownerRepo: pr.ownerRepo,
          number: pr.number,
          commitId: detail.headSha,
          comment: { path: selected, side, line: end, startLine: start < end ? start : null, body },
        }),
      "Comment posted",
    );
    setComposer(null);
  };

  const addDraft = (side: Side, start: number, end: number, body: string) => {
    if (!selected) return;
    setDrafts((d) => [...d, { id: `${Date.now()}`, path: selected, side, line: end, startLine: start < end ? start : undefined, body }]);
    setComposer(null);
  };

  const submit = async (event: ReviewEvent, body: string) => {
    if (!detail) return;
    setSubmitting(true);
    try {
      await invoke("pr_submit_review", {
        ownerRepo: pr.ownerRepo,
        number: pr.number,
        commitId: detail.headSha,
        event,
        body,
        comments: drafts.map(({ path, side, line, startLine, body }) => ({ path, side, line, startLine: startLine ?? null, body })),
      });
      setDrafts([]);
      try {
        localStorage.removeItem(`orbit.review.summary.${prKey}`);
      } catch {
        // nothing to clear
      }
      setSubmitOpen(false);
      toast.success(event === "APPROVE" ? "Approved" : event === "REQUEST_CHANGES" ? "Changes requested" : "Review submitted", {
        description: `${pr.repo} #${pr.number}${drafts.length ? ` · ${drafts.length} comment${drafts.length === 1 ? "" : "s"}` : ""}`,
        action: { label: "Open on GitHub", onClick: () => openUrl(pr.url).catch(() => null) },
      });
      await loadReview(false);
    } catch (e) {
      onError(String(e));
    } finally {
      setSubmitting(false);
    }
  };

  // ---------- inline layer for the current file ----------
  const fileThreads = (selected && threadsByPath.get(selected)) || [];
  const fileDrafts = drafts.filter((d) => d.path === selected);
  const placedThreads = fileThreads.filter((t) => t.line !== null && (t.side === "RIGHT" || split));
  const offDiffThreads = fileThreads.filter((t) => !placedThreads.includes(t));

  const items: ZoneItem[] = useMemo(() => {
    const groups = new Map<string, { side: Side; line: number; nodes: React.ReactNode[] }>();
    const add = (side: Side, line: number, key: string, node: React.ReactNode) => {
      const g = groups.get(`${side}:${line}`) ?? { side, line, nodes: [] };
      g.nodes.push(<div key={key}>{node}</div>);
      groups.set(`${side}:${line}`, g);
    };
    for (const t of placedThreads) add(t.side, t.line!, t.id, <ThreadCard thread={t} actions={actions} />);
    for (const d of fileDrafts) {
      if (d.side === "LEFT" && !split) continue;
      add(
        d.side,
        d.line,
        d.id,
        <DraftCard
          draft={d}
          viewer={review?.viewer ?? ""}
          onChange={(body) => setDrafts((all) => all.map((x) => (x.id === d.id ? { ...x, body } : x)))}
          onDelete={() => setDrafts((all) => all.filter((x) => x.id !== d.id))}
        />,
      );
    }
    if (composer) {
      const { side, start, end } = composer;
      add(
        side,
        end,
        "composer",
        <Composer
          label={start < end ? `Comment on lines ${start}–${end}` : `Comment on line ${end}`}
          reviewInProgress={drafts.length > 0}
          onAddToReview={(body) => addDraft(side, start, end, body)}
          onCommentNow={(body) => commentNow(side, start, end, body)}
          onCancel={() => setComposer(null)}
        />,
      );
    }
    return [...groups.values()].map((g) => ({
      key: `${selected}:${g.side}:${g.line}`,
      side: g.side,
      line: g.line,
      node: <div className="rv-zone-stack">{g.nodes}</div>,
    }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, placedThreads, fileDrafts, composer, split, drafts.length, review?.viewer]);

  const marks: RangeMark[] = useMemo(
    () => [
      ...placedThreads
        .filter((t) => t.startLine && t.startLine < (t.line ?? 0))
        .map((t) => ({ side: t.side, start: t.startLine!, end: t.line!, kind: "thread" as const })),
      ...fileDrafts.filter((d) => d.startLine).map((d) => ({ side: d.side, start: d.startLine!, end: d.line, kind: "draft" as const })),
      ...(composer ? [{ side: composer.side, start: composer.start, end: composer.end, kind: "selecting" as const }] : []),
    ],
    [placedThreads, fileDrafts, composer],
  );

  const zones = useReviewZones({
    items,
    marks,
    commentable: selected ? review?.commentable[selected] : undefined,
    split,
    onRequestComment: (side, start, end) => setComposer({ side, start, end }),
  });

  const onMount = (editor: Monaco.IStandaloneDiffEditor, monaco: typeof import("monaco-editor")) => {
    nav.onMount(editor, monaco);
    zones.onMount(editor, monaco);
  };

  if (!pr) return null;
  const decision = review?.reviewDecision ? DECISION[review.reviewDecision] : null;

  return (
    <div className="commit-review pr-review">
      <div className="pr-review-side">
        {prs.length > 1 && (
          <div className="pr-selector">
            <div className="pr-selector-label">
              Repositories <span className="pr-selector-count">{prs.length}</span>
            </div>
            {prs.map((p, i) => (
              <button
                key={p.ownerRepo}
                className={`btn-plain pr-selector-item ${i === activeIdx ? "on" : ""}`}
                aria-pressed={i === activeIdx}
                title={`${p.repo} #${p.number}`}
                onClick={() => setActiveIdx(i)}
              >
                <RepoIcon size={13} />
                <span className="pr-selector-name">{p.repo}</span>
                <span className="pr-selector-num">#{p.number}</span>
              </button>
            ))}
          </div>
        )}

        {detail === null ? (
          <div className="pr-side-loading">
            <Skeleton w="80%" h={14} />
            <Skeleton w="60%" h={10} />
            <Skeleton w="70%" h={11} />
          </div>
        ) : (
          <>
            <div className="pr-header">
              <div className="pr-header-top">
                <a
                  className="pr-header-link"
                  href={pr.url}
                  onClick={(e) => {
                    e.preventDefault();
                    openUrl(pr.url).catch(() => null);
                  }}
                >
                  {pr.repo} #{pr.number} ↗
                </a>
                {decision && <span className={`rv-decision rv-decision-${decision.cls}`}>{decision.label}</span>}
              </div>
              <div className="pr-header-title" title={detail.title}>
                {detail.title}
              </div>
              <div className="pr-header-branch" title={`${detail.branch} → ${detail.base}`}>
                <span className="pr-branch mono">{detail.branch}</span>
                <span className="pr-branch-arrow">→</span>
                <span className="pr-branch mono">{detail.base}</span>
              </div>
              <div className="pr-header-stats">
                <span className="pr-header-author">{detail.author}</span>
                <span className="git-stat-add">+{detail.additions}</span>
                <span className="git-stat-del">−{detail.deletions}</span>
                {unresolved > 0 && <span className="rv-unresolved">{unresolved} unresolved</span>}
              </div>
            </div>

            <div className="pr-files">
              <button className={`rv-nav ${selected === OVERVIEW ? "on" : ""}`} onClick={() => setSelected(OVERVIEW)}>
                <DocIcon size={13} />
                <span>Overview</span>
                {(review?.reviews.length ?? 0) > 0 && (
                  <span className="rv-file-count" {...tipProps(`${review!.reviews.length} review${review!.reviews.length === 1 ? "" : "s"}`)}>
                    {review!.reviews.length}
                  </span>
                )}
              </button>
              <div className="rv-files-label">
                Files <span>{viewedCount}/{detail.files.length} viewed</span>
                <button
                  className={`rv-hide-viewed ${hideViewed ? "on" : ""}`}
                  aria-pressed={hideViewed}
                  aria-label="Hide viewed files"
                  {...tipProps(hideViewed ? `Show viewed files${viewedCount ? ` (${viewedCount} hidden)` : ""}` : "Hide viewed files")}
                  onClick={() => setHideViewed((v) => !v)}
                >
                  <EyeIcon size={13} />
                </button>
              </div>
              <div className="rv-progress" aria-hidden>
                <div style={{ width: `${detail.files.length ? (viewedCount / detail.files.length) * 100 : 0}%` }} />
              </div>
              {hideViewed && viewedCount === detail.files.length && (
                <div className="rv-files-done">
                  <CheckIcon size={12} /> All files viewed
                </div>
              )}
              {detail.files
                // The open file stays listed so marking it viewed doesn't yank it away.
                .filter((f) => !hideViewed || f.path === selected || !viewed.includes(f.path))
                .map((f) => {
                const threads = threadsByPath.get(f.path) ?? [];
                const open = threads.filter((t) => !t.isResolved).length;
                const draftsHere = drafts.filter((d) => d.path === f.path).length;
                const isViewed = viewed.includes(f.path);
                return (
                  <div key={f.path} className={`rv-file-row ${selected === f.path ? "tree-active" : ""} ${isViewed ? "viewed" : ""}`}>
                    <CheckBox
                      label={isViewed ? "Mark as not viewed" : "Mark as viewed"}
                      checked={isViewed}
                      onChange={(v) => markViewed(f.path, v, false)}
                    />
                    <button className="rv-file-btn" title={f.path} onClick={() => setSelected(f.path)}>
                      <span className="git-change-name">
                        {f.path.split("/").pop()}
                        {f.path.includes("/") && <span className="git-change-dir">{f.path.slice(0, f.path.lastIndexOf("/"))}</span>}
                      </span>
                      {open > 0 && <span className="rv-file-count" {...tipProps(`${open} unresolved thread${open === 1 ? "" : "s"}`)}>{open}</span>}
                      {draftsHere > 0 && <span className="rv-file-count rv-file-count-draft" {...tipProps(`${draftsHere} pending`)}>{draftsHere}</span>}
                      <span className="git-change-stats">
                        {f.additions > 0 && <span className="git-stat-add">+{f.additions}</span>}
                        {f.deletions > 0 && <span className="git-stat-del">−{f.deletions}</span>}
                      </span>
                    </button>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>

      <div className="pr-diff-col">
        <div className="editor-filebar rv-filebar">
          {selected && selected !== OVERVIEW ? (
            <span className="rv-path" title={selected}>
              {selected.includes("/") && <span className="rv-path-dir">{selected.slice(0, selected.lastIndexOf("/") + 1)}</span>}
              <span className="rv-path-name">{selected.split("/").pop()}</span>
            </span>
          ) : (
            <span className="rv-path">
              <span className="rv-path-name">Overview</span>
            </span>
          )}
          <span className="editor-filebar-actions">
            {selected && selected !== OVERVIEW && (
              <>
                {nav.count > 0 && (
                  <span className="diff-nav">
                    <button className="mode-btn" aria-label="Previous change" {...tipProps("Previous change (Shift+F7)")} onClick={() => nav.jump(-1)}>
                      <ChevronIcon size={13} dir="left" />
                    </button>
                    <span className="diff-nav-count">
                      {nav.index < 0 ? "–" : nav.index + 1}/{nav.count}
                    </span>
                    <button className="mode-btn" aria-label="Next change" {...tipProps("Next change (F7)")} onClick={() => nav.jump(1)}>
                      <ChevronIcon size={13} />
                    </button>
                  </span>
                )}
                <span className="mode-toggle seg">
                  <button className={`mode-btn ${!split ? "mode-active" : ""}`} onClick={() => setSplit(false)}>
                    Unified
                  </button>
                  <button className={`mode-btn ${split ? "mode-active" : ""}`} {...tipProps("Side by side; lets you comment on removed lines")} onClick={() => setSplit(true)}>
                    Split
                  </button>
                </span>
                <label className={`rv-viewed-toggle ${viewed.includes(selected) ? "on" : ""}`} {...tipProps("Mark as viewed and open the next file")}>
                  <CheckBox
                    label="Viewed"
                    checked={viewed.includes(selected)}
                    onChange={(v) => markViewed(selected, v, true)}
                  />
                  Viewed
                </label>
              </>
            )}
            <span className="rv-submit-anchor">
              <button className={`rv-review-btn ${drafts.length ? "has-pending" : ""}`} disabled={!detail} onClick={() => setSubmitOpen((v) => !v)}>
                Review
                {drafts.length > 0 && <span className="rv-review-count">{drafts.length}</span>}
              </button>
              {submitOpen && (
                <SubmitReview
                  summaryKey={`orbit.review.summary.${prKey}`}
                  pending={drafts.length}
                  ownPr={ownPr}
                  busy={submitting}
                  onSubmit={submit}
                  onDiscard={() => setDrafts([])}
                  onClose={() => setSubmitOpen(false)}
                />
              )}
            </span>
          </span>
        </div>

        {selected === OVERVIEW ? (
          <Overview detail={detail} review={review} onOpenFile={setSelected} />
        ) : selected === null ? (
          <div className="editor-empty">Pick a file to see its diff</div>
        ) : diff === null ? (
          <div className="table-loading">
            <span className="spinner" /> Loading diff…
          </div>
        ) : (
          <>
            {offDiffThreads.length > 0 && (
              <div className="rv-offdiff">
                <div className="rv-offdiff-head">
                  {offDiffThreads.some((t) => t.line === null) ? "Outdated comments" : "Comments on removed lines"}
                  {!split && offDiffThreads.some((t) => t.line !== null) && (
                    <button className="btn-link" onClick={() => setSplit(true)}>
                      Show them inline in split view
                    </button>
                  )}
                </div>
                {offDiffThreads.map((t) => (
                  <ThreadCard key={t.id} thread={t} actions={actions} />
                ))}
              </div>
            )}
            <div className="editor-host">
              <DiffEditor
                key={`${prKey}:${selected}`}
                height="100%"
                theme="orbit-dark"
                beforeMount={beforeMount}
                onMount={onMount}
                language={langOf(selected)}
                original={diff.original}
                modified={diff.modified}
                options={{
                  readOnly: true,
                  renderSideBySide: split,
                  fontSize: 12.5,
                  minimap: { enabled: false },
                  scrollBeyondLastLine: false,
                  automaticLayout: true,
                  renderOverviewRuler: true,
                  overviewRulerLanes: 3,
                  diffWordWrap: "off",
                  glyphMargin: true,
                  hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 6, revealLineCount: 20 },
                } as never}
                loading={
                  <div className="table-loading">
                    <span className="spinner" /> Loading diff…
                  </div>
                }
              />
            </div>
            {zones.portals}
          </>
        )}
      </div>
    </div>
  );
}

function tipProps(text: string) {
  return {
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  };
}

const REVIEW_STATE: Record<string, { label: string; cls: string }> = {
  APPROVED: { label: "approved", cls: "ok" },
  CHANGES_REQUESTED: { label: "requested changes", cls: "bad" },
  COMMENTED: { label: "reviewed", cls: "muted" },
  DISMISSED: { label: "review dismissed", cls: "muted" },
  PENDING: { label: "has a pending review", cls: "muted" },
};

function Overview({ detail, review, onOpenFile }: { detail: PrDetail | null; review: ReviewData | null; onOpenFile: (path: string) => void }) {
  if (!detail) return <div className="table-loading"><span className="spinner" /> Loading…</div>;
  const threads = review?.threads ?? [];
  return (
    <div className="rv-overview">
      <section className="rv-ov-card">
        <div className="rv-ov-head">
          <strong>{detail.author}</strong> wants to merge <span className="mono">{detail.branch}</span> into <span className="mono">{detail.base}</span>
        </div>
        {detail.body.trim() ? <SafeMarkdown content={detail.body} /> : <p className="muted">No description provided.</p>}
      </section>

      <section className="rv-ov-section">
        <h4>Reviews</h4>
        {review === null ? (
          <Skeleton w="50%" h={12} />
        ) : review.reviews.length === 0 ? (
          <p className="muted">No reviews yet. Comment on lines in the files, then press Review to submit.</p>
        ) : (
          review.reviews.map((r, i) => {
            const st = REVIEW_STATE[r.state] ?? { label: r.state.toLowerCase(), cls: "muted" };
            return (
              <div key={i} className="rv-ov-review">
                <img className="rv-avatar" src={r.avatarUrl} alt="" />
                <div className="rv-ov-review-main">
                  <div>
                    <strong>{r.author}</strong> <span className={`rv-state-txt rv-state-${st.cls}`}>{st.label}</span>
                    {r.submittedAt && <span className="rv-comment-time"> · {timeAgo(r.submittedAt)}</span>}
                  </div>
                  {r.body.trim() && <SafeMarkdown content={r.body} />}
                </div>
              </div>
            );
          })
        )}
      </section>

      {threads.length > 0 && (
        <section className="rv-ov-section">
          <h4>Conversations</h4>
          {threads.map((t) => (
            <button key={t.id} className="rv-ov-thread" onClick={() => onOpenFile(t.path)}>
              {t.isResolved ? <CheckIcon size={12} /> : <span className="rv-ov-dot" />}
              <span className="mono rv-ov-path">
                {t.path}
                {t.line ? `:${t.line}` : ""}
              </span>
              <span className="rv-ov-snippet">{t.comments[0]?.body.split("\n")[0]}</span>
              <span className="rv-comment-time">{t.comments.length}</span>
            </button>
          ))}
        </section>
      )}
    </div>
  );
}
