import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  BranchInfo,
  Config,
  GitChange,
  GitCommit,
  PullRequest,
  RepoOverview,
  Workspace,
  WsPrStatus,
} from "../types/config";
import { CheckBox } from "../components/CheckBox";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { ContextMenu, MenuItem } from "../components/ContextMenu";
import { MergeModal } from "../components/MergeModal";
import { PrsModal } from "../components/PrsModal";
import { Skeleton } from "../components/Skeleton";
import { toast } from "../components/Toast";
import { tooltip } from "../components/Tooltip";
import {
  BranchIcon,
  ChevronIcon,
  CodeViewIcon,
  DownloadIcon,
  FolderIcon,
  PlusIcon,
  PullRequestIcon,
  PushIcon,
  RefreshIcon,
  RepoIcon,
  SatelliteIcon,
  SearchIcon,
  SparkIcon,
  StashIcon,
  TerminalIcon,
  TrashIcon,
  UndoIcon,
} from "../components/Icons";
import { GitHubIcon } from "../components/BrandIcons";

interface Props {
  repo: string;
  config: Config;
  workspaces: Workspace[];
  onOpenWorkspace: (ws: Workspace) => void;
  /** Diff review / commit inspection tabs for this repo's clone. */
  onReviewFile: (path: string) => void;
  onReviewCommit: (commit: GitCommit) => void;
  onOpenPrList: (prs: PullRequest[]) => void;
  /** Opens a terminal in the clone: a shell (null) or an agent CLI. */
  onNewSession: (label: string, cmd: string | null) => void;
  onError: (msg: string) => void;
}

const STATUS_CLASS: Record<string, string> = {
  M: "git-status-modified",
  A: "git-status-added",
  D: "git-status-deleted",
  U: "git-status-untracked",
};

/** Pending branch switch while the working tree has changes. */
interface SwitchRequest {
  branch: string;
}

/**
 * Repository view: one repo's base clone, managed directly — current branch
 * and switching, sync with origin, selective commits, history, branches,
 * stashes and the workspaces built on it. Everything the dock, editor and
 * terminals do here runs on the clone through the "@repo" scope.
 */
export function RepoPage({
  repo,
  config,
  workspaces,
  onOpenWorkspace,
  onReviewFile,
  onReviewCommit,
  onOpenPrList,
  onNewSession,
  onError,
}: Props) {
  const scope = `@${repo}`;
  const [ov, setOv] = useState<RepoOverview | null>(null);
  const [changes, setChanges] = useState<GitChange[] | null>(null);
  const [history, setHistory] = useState<GitCommit[] | null>(null);
  const [prs, setPrs] = useState<WsPrStatus[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [fetching, setFetching] = useState(false);
  // Unchecked files: new files arrive checked, like GitHub Desktop.
  const [excluded, setExcluded] = useState<Set<string>>(new Set());
  const [summary, setSummary] = useState("");
  const [description, setDescription] = useState("");
  const [drafting, setDrafting] = useState(false);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const [switchReq, setSwitchReq] = useState<SwitchRequest | null>(null);
  const [confirm, setConfirm] = useState<null | { title: string; message: string; label: string; danger?: boolean; run: () => Promise<void> }>(null);
  const [prCreateOpen, setPrCreateOpen] = useState(false);
  const [merging, setMerging] = useState<WsPrStatus[] | null>(null);
  const [sessionMenu, setSessionMenu] = useState<{ x: number; y: number } | null>(null);
  const [branchFilter, setBranchFilter] = useState("");
  const [showRemote, setShowRemote] = useState(false);

  const load = useCallback(async () => {
    try {
      const o = await invoke<RepoOverview>("repo_overview", { name: repo });
      setOv(o);
      if (!o.cloned) return;
      const [c, h] = await Promise.all([
        invoke<GitChange[]>("git_changes", { workspace: scope, repo }),
        invoke<GitCommit[]>("repo_history", { name: repo, limit: 40 }),
      ]);
      setChanges(c);
      setHistory(h);
      invoke<WsPrStatus[]>("repo_branch_prs", { name: repo })
        .then(setPrs)
        .catch(() => setPrs([]));
    } catch (e) {
      onError(String(e));
    }
  }, [repo, scope, onError]);

  const fetchOrigin = useCallback(async () => {
    setFetching(true);
    try {
      await invoke("refresh_repo", { name: repo });
    } catch (e) {
      onError(String(e));
    } finally {
      setFetching(false);
      load();
    }
  }, [repo, load, onError]);

  // Fresh state on open, and whenever Orbit regains focus (edits made in an
  // editor or terminal elsewhere show up without a manual refresh).
  useEffect(() => {
    setOv(null);
    setChanges(null);
    setHistory(null);
    setPrs(null);
    load();
    const onFocus = () => load();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [load]);

  /** Runs one git action with a busy label, then refreshes. */
  const run = async (label: string, action: () => Promise<unknown>, success?: string) => {
    setBusy(label);
    try {
      await action();
      if (success) toast.success(success);
    } catch (e) {
      toast.error(`${label} failed`, { description: String(e) });
    } finally {
      setBusy(null);
      load();
    }
  };

  const hint = (text: string) => ({
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  });

  // ---------- derived state ----------
  const branch = ov?.branch ?? null;
  const defaultBranch = ov?.defaultBranch ?? "main";
  const onDefault = branch !== null && branch === defaultBranch;
  const selected = (changes ?? []).filter((c) => !excluded.has(c.path));
  const openPr = (prs ?? []).find((p) => p.state === "OPEN");
  const mergedPr = (prs ?? []).find((p) => p.state === "MERGED");
  const repoWorkspaces = workspaces.filter((w) => w.repos.includes(repo));
  const service = config.services.find((s) => s.name === repo);
  // Branch-only workspaces check their branch out in the clone itself.
  const holder =
    service?.worktree === false ? repoWorkspaces.find((w) => w.branch === branch && !w.variant_of) : undefined;
  const otherWorktrees = (ov?.worktrees ?? []).filter((w) => !w.main && !w.workspace);
  const locals = (ov?.branches ?? []).filter((b) => !b.remoteOnly);
  const remotes = (ov?.branches ?? []).filter((b) => b.remoteOnly);
  const filterText = branchFilter.trim().toLowerCase();
  const matches = (b: BranchInfo) => !filterText || b.name.toLowerCase().includes(filterText);
  const worktreeOwner = (b: BranchInfo) =>
    b.worktree ? ov?.worktrees.find((w) => norm(w.path) === norm(b.worktree!))?.workspace ?? null : null;

  // ---------- actions ----------
  const requestSwitch = (target: string) => {
    setSwitcherOpen(false);
    if (target === branch) return;
    if ((ov?.changes ?? 0) > 0) setSwitchReq({ branch: target });
    else run("Switch branch", () => invoke("repo_switch", { name: repo, branch: target, stash: false }), `Switched to ${target}`);
  };

  const createBranch = (name: string, from: string | null) => {
    setSwitcherOpen(false);
    run("Create branch", () => invoke("repo_create_branch", { name: repo, branch: name, from }), `Created and switched to ${name}`);
  };

  const commit = async () => {
    const message = description.trim() ? `${summary.trim()}\n\n${description.trim()}` : summary.trim();
    if (!message || selected.length === 0) return;
    setBusy("Commit");
    try {
      await invoke("repo_commit", { name: repo, message, paths: selected.map((c) => c.path) });
      toast.success(`Committed to ${branch ?? "HEAD"}`, { description: summary.trim() });
      setSummary("");
      setDescription("");
      setExcluded(new Set());
    } catch (e) {
      toast.error("Commit failed", { description: String(e) });
    } finally {
      setBusy(null);
      load();
    }
  };

  const draftMessage = async () => {
    setDrafting(true);
    try {
      const r = await invoke<{ message: string }>("ws_commit_message", { workspace: scope, repo });
      setSummary(r.message);
    } catch (e) {
      toast.error("Couldn't draft a message", { description: String(e) });
    } finally {
      setDrafting(false);
    }
  };

  const discard = (paths: string[]) =>
    setConfirm({
      title: paths.length === 1 ? "Discard changes" : `Discard ${paths.length} changes`,
      message:
        paths.length === 1
          ? `Discard the changes to ${paths[0]}? Edits are lost; new files go to the Trash.`
          : `Discard the changes to ${paths.length} files? Edits are lost; new files go to the Trash.`,
      label: "Discard",
      danger: true,
      run: () => run("Discard", () => invoke("repo_discard", { name: repo, paths })),
    });

  const deleteBranch = (b: BranchInfo) =>
    setConfirm({
      title: "Delete branch",
      message: b.gone
        ? `Delete ${b.name}? It was deleted on origin, so its PR was most likely merged.`
        : `Delete ${b.name}? Commits that aren't on any other branch will be lost.`,
      label: "Delete",
      danger: true,
      run: () => run("Delete branch", () => invoke("repo_delete_branch", { name: repo, branch: b.name, force: true }), `Deleted ${b.name}`),
    });

  // Merged and deleted on GitHub: go back to the default branch, up to date.
  const finishBranch = () =>
    setConfirm({
      title: "Finish branch",
      message: `Switch to ${defaultBranch}, pull it and delete ${branch}?`,
      label: "Finish",
      run: () =>
        run(
          "Finish branch",
          async () => {
            await invoke("repo_switch", { name: repo, branch: defaultBranch, stash: false });
            await invoke("repo_pull", { name: repo, rebase: false });
            await invoke("repo_delete_branch", { name: repo, branch, force: true });
          },
          `Back on ${defaultBranch}`,
        ),
    });

  const openPrTab = () =>
    invoke<PullRequest[]>("ws_prs_flat", { workspace: scope })
      .then((list) => (list.length ? onOpenPrList(list) : openPr && openUrl(openPr.url)))
      .catch((e) => onError(String(e)));

  // ---------- render ----------
  if (ov && !ov.cloned) {
    return (
      <div className="page repo-page">
        <RepoHeader repo={repo} ov={ov} />
        <div className="pr-empty">
          <span className="pr-empty-icon">
            <RepoIcon size={18} />
          </span>
          <div>
            <strong>{repo} isn't cloned yet</strong>
            <span>Clone it to manage its branches, commit and push from here.</span>
          </div>
          <button
            disabled={busy !== null}
            onClick={() => run("Clone", () => invoke("clone_service", { name: repo }), `${repo} cloned`)}
          >
            {busy === "Clone" ? "Cloning…" : "Clone repository"}
          </button>
        </div>
      </div>
    );
  }

  const sync = syncAction(ov, fetching);

  return (
    <div className="page repo-page">
      <RepoHeader repo={repo} ov={ov}>
        <button className="secondary ws-tool" onClick={(e) => setSessionMenu({ x: e.clientX, y: e.clientY })} {...hint("Terminal or agent session in this clone")}>
          <TerminalIcon size={14} /> New session
        </button>
        <button
          className="secondary ws-tool"
          onClick={() => invoke("open_in_editor", { workspace: scope, repo }).catch((e) => onError(String(e)))}
          {...hint("Open the clone in VS Code or Zed")}
        >
          <CodeViewIcon size={14} /> Editor
        </button>
        <button
          className="icon-button ws-icon-tool"
          aria-label="Reveal folder"
          onClick={() => invoke("reveal_service", { name: repo }).catch((e) => onError(String(e)))}
          {...hint(ov?.path ?? "Reveal folder")}
        >
          <FolderIcon size={15} />
        </button>
        <span className="ws-tool-sep" />
        <button className="icon-button ws-icon-tool" aria-label="Refresh" disabled={fetching} onClick={fetchOrigin} {...hint("Fetch origin and refresh")}>
          {fetching ? <span className="spinner" /> : <RefreshIcon size={15} />}
        </button>
      </RepoHeader>

      {/* GitHub Desktop-style bar: where you are, how you stand with origin, the PR. */}
      <div className="repo-bar">
        <div className="repo-bar-cell repo-branch-cell">
          <button className="btn-plain repo-bar-btn" onClick={() => setSwitcherOpen((o) => !o)} disabled={!ov || busy !== null}>
            <BranchIcon size={15} />
            <span className="repo-bar-text">
              <span className="repo-bar-label">Current branch</span>
              <span className="repo-bar-value">{ov ? branch ?? `detached at ${ov.head}` : <Skeleton w={90} h={12} />}</span>
            </span>
            <ChevronIcon size={12} />
          </button>
          {switcherOpen && ov && (
            <BranchSwitcher
              ov={ov}
              ownerOf={worktreeOwner}
              onSwitch={requestSwitch}
              onCreate={createBranch}
              onClose={() => setSwitcherOpen(false)}
            />
          )}
        </div>

        <div className="repo-bar-cell">
          <button
            className={`btn-plain repo-bar-btn ${sync.primary ? "repo-bar-primary" : ""}`}
            disabled={!ov || busy !== null || fetching}
            onClick={() => {
              if (sync.kind === "fetch") fetchOrigin();
              else if (sync.kind === "pull") run("Pull", () => invoke("repo_pull", { name: repo, rebase: sync.rebase }), "Pulled from origin");
              else run(sync.kind === "publish" ? "Publish" : "Push", () => invoke("ws_push", { workspace: scope, repo }), sync.kind === "publish" ? `Published ${branch}` : "Pushed to origin");
            }}
            {...hint(sync.hint)}
          >
            {sync.kind === "push" || sync.kind === "publish" ? <PushIcon size={15} /> : sync.kind === "pull" ? <DownloadIcon size={15} /> : fetching ? <span className="spinner" /> : <RefreshIcon size={15} />}
            <span className="repo-bar-text">
              <span className="repo-bar-label">{busy === "Push" || busy === "Pull" || busy === "Publish" ? `${busy}ing…` : sync.label}</span>
              <span className="repo-bar-value">{sync.detail}</span>
            </span>
          </button>
        </div>

        <div className="repo-bar-cell">
          {onDefault ? (
            <div className="repo-bar-btn repo-bar-static">
              <PullRequestIcon size={15} />
              <span className="repo-bar-text">
                <span className="repo-bar-label">Pull request</span>
                <span className="repo-bar-value repo-bar-muted">Create a branch to open one</span>
              </span>
            </div>
          ) : openPr ? (
            <button className="btn-plain repo-bar-btn" onClick={openPrTab} {...hint(openPr.title)}>
              <PullRequestIcon size={15} />
              <span className="repo-bar-text">
                <span className="repo-bar-label">Pull request · open</span>
                <span className="repo-bar-value">#{openPr.number} {openPr.title}</span>
              </span>
            </button>
          ) : mergedPr ? (
            <button className="btn-plain repo-bar-btn" onClick={() => openUrl(mergedPr.url)} {...hint("Open on GitHub")}>
              <PullRequestIcon size={15} />
              <span className="repo-bar-text">
                <span className="repo-bar-label">Pull request · merged</span>
                <span className="repo-bar-value">#{mergedPr.number} {mergedPr.title}</span>
              </span>
            </button>
          ) : (
            <button
              className="btn-plain repo-bar-btn"
              disabled={!ov?.upstream || ov.upstreamGone || busy !== null}
              onClick={() => setPrCreateOpen(true)}
              {...hint(ov?.upstream ? `Open a PR from ${branch} into ${defaultBranch}` : "Publish the branch first")}
            >
              <PullRequestIcon size={15} />
              <span className="repo-bar-text">
                <span className="repo-bar-label">Pull request</span>
                <span className="repo-bar-value">{prs === null ? "Checking…" : "Create pull request"}</span>
              </span>
            </button>
          )}
          {openPr && !openPr.isDraft && (
            <button className="secondary btn-mini repo-bar-side" onClick={() => setMerging([openPr])} {...hint(`Squash into ${defaultBranch}`)}>
              Merge
            </button>
          )}
        </div>
      </div>

      {/* State that needs attention before anything else. */}
      {ov?.operation && (
        <div className="repo-banner repo-banner-warn">
          <span>
            <strong>{capitalize(ov.operation)} in progress</strong>
            {ov.conflicts.length > 0 ? ` · ${ov.conflicts.length} conflicted file${ov.conflicts.length === 1 ? "" : "s"}` : " · no conflicts left"}
          </span>
          {ov.operation === "rebase" && (
            <span className="repo-banner-actions">
              {ov.conflicts.length > 0 && (
                <button className="secondary btn-mini" disabled={busy !== null} onClick={() => run("Resolve conflicts", () => invoke("ws_resolve_conflicts", { workspace: scope, repo }), "Conflicts resolved by the agent")}>
                  <SparkIcon size={12} /> Resolve with AI
                </button>
              )}
              <button className="btn-mini" disabled={busy !== null || ov.conflicts.length > 0} onClick={() => run("Continue rebase", () => invoke("ws_rebase_continue", { workspace: scope, repo }))}>
                Continue
              </button>
              <button className="secondary btn-mini" disabled={busy !== null} onClick={() => run("Abort rebase", () => invoke("ws_rebase_abort", { workspace: scope, repo }), "Rebase aborted")}>
                Abort
              </button>
            </span>
          )}
        </div>
      )}
      {ov?.upstreamGone && !onDefault && (
        <div className="repo-banner repo-banner-merged">
          <span>
            <strong>{ov.upstream}</strong> was deleted on GitHub{mergedPr ? ` after #${mergedPr.number} was merged` : ""}.
          </span>
          <button className="secondary btn-mini" disabled={busy !== null || (ov.changes ?? 0) > 0} onClick={finishBranch} {...hint(ov.changes ? "Commit or stash your changes first" : "")}>
            Switch to {defaultBranch} and delete branch
          </button>
        </div>
      )}
      {holder && (
        <div className="repo-banner">
          <span>
            This clone is checked out on <strong>{holder.branch}</strong> for the workspace <strong>{holder.name}</strong>.
          </span>
          <button className="secondary btn-mini" onClick={() => onOpenWorkspace(holder)}>
            Open workspace
          </button>
        </div>
      )}

      <div className="repo-grid">
        {/* ---------- Changes + commit ---------- */}
        <section className="repo-panel repo-changes">
          <div className="repo-panel-head">
            {changes && changes.length > 0 && (
              <CheckBox
                checked={selected.length === changes.length}
                label="Select all"
                onChange={(on) => setExcluded(on ? new Set() : new Set(changes.map((c) => c.path)))}
              />
            )}
            <h3>
              Changes {changes && changes.length > 0 && <span className="section-count">{changes.length}</span>}
            </h3>
            {changes && changes.length > 0 && (
              <span className="repo-panel-tools">
                <button className="icon-button" aria-label="Stash all changes" disabled={busy !== null} onClick={() => run("Stash", () => invoke("repo_stash", { name: repo }), "Changes stashed")} {...hint("Stash all changes")}>
                  <StashIcon size={14} />
                </button>
                <button className="icon-button icon-button-danger" aria-label="Discard all changes" disabled={busy !== null} onClick={() => discard(changes.map((c) => c.path))} {...hint("Discard all changes")}>
                  <TrashIcon size={14} />
                </button>
              </span>
            )}
          </div>

          <div className="repo-files">
            {changes === null ? (
              <div className="repo-file">
                <Skeleton w="70%" h={12} />
              </div>
            ) : changes.length === 0 ? (
              <div className="rv-empty">
                <strong>No local changes</strong>
                <span>
                  {ov && ov.ahead > 0
                    ? `${ov.ahead} commit${ov.ahead === 1 ? "" : "s"} waiting to be pushed.`
                    : "Edit files here, in your editor or with an agent session — changes show up as you work."}
                </span>
                <button className="secondary btn-mini" onClick={() => onNewSession("claude", "claude")}>
                  <SparkIcon size={12} /> Start an agent session
                </button>
              </div>
            ) : (
              changes.map((c) => {
                const dir = c.path.includes("/") ? c.path.slice(0, c.path.lastIndexOf("/")) : "";
                return (
                  <div key={c.path} className={`repo-file ${excluded.has(c.path) ? "repo-file-off" : ""}`}>
                    <CheckBox
                      checked={!excluded.has(c.path)}
                      label={`Include ${c.path}`}
                      onChange={(on) =>
                        setExcluded((prev) => {
                          const next = new Set(prev);
                          if (on) next.delete(c.path);
                          else next.add(c.path);
                          return next;
                        })
                      }
                    />
                    <button className="btn-plain repo-file-main" onClick={() => onReviewFile(c.path)} {...hint(`Review changes of ${c.path}`)}>
                      <span className={`git-status-badge ${STATUS_CLASS[c.status] ?? STATUS_CLASS.M}`}>{c.status}</span>
                      <span className="repo-file-name">{c.path.split("/").pop()}</span>
                      {dir && <span className="repo-file-dir">{dir}</span>}
                    </button>
                    {(c.added > 0 || c.deleted > 0) && (
                      <span className="git-change-stats">
                        {c.added > 0 && <span className="git-stat-add">+{c.added}</span>}
                        {c.deleted > 0 && <span className="git-stat-del">−{c.deleted}</span>}
                      </span>
                    )}
                    <button className="icon-button icon-button-danger repo-file-discard" aria-label={`Discard ${c.path}`} onClick={() => discard([c.path])} {...hint("Discard changes")}>
                      <UndoIcon size={13} />
                    </button>
                  </div>
                );
              })
            )}
          </div>

          <div className="repo-commit">
            <div className="repo-commit-summary">
              <input
                placeholder={changes && changes.length > 0 ? "Summary (required)" : "Nothing to commit"}
                value={summary}
                disabled={!changes || changes.length === 0 || !!ov?.operation}
                onChange={(e) => setSummary(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) commit();
                }}
              />
              <button
                className="icon-button"
                aria-label="Draft with AI"
                disabled={drafting || !changes || changes.length === 0}
                onClick={draftMessage}
                {...hint("Draft the message with AI")}
              >
                {drafting ? <span className="spinner" /> : <SparkIcon size={14} />}
              </button>
            </div>
            <textarea
              placeholder="Description"
              rows={2}
              value={description}
              disabled={!changes || changes.length === 0 || !!ov?.operation}
              onChange={(e) => setDescription(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) commit();
              }}
            />
            <button
              className="repo-commit-btn"
              disabled={!summary.trim() || selected.length === 0 || busy !== null || !!ov?.operation}
              onClick={commit}
              {...hint("Ctrl+Enter")}
            >
              {busy === "Commit"
                ? "Committing…"
                : selected.length === 0
                  ? "Select files to commit"
                  : `Commit ${selected.length} file${selected.length === 1 ? "" : "s"} to ${branch ?? "HEAD"}`}
            </button>
          </div>
        </section>

        {/* ---------- History ---------- */}
        <section className="repo-panel repo-history">
          <div className="repo-panel-head">
            <h3>History</h3>
            {ov && ov.ahead > 0 && <span className="repo-pill repo-pill-info">↑ {ov.ahead} not pushed</span>}
          </div>
          <div className="repo-commits">
            {history === null ? (
              <div className="repo-commit-row">
                <Skeleton w="80%" h={12} />
              </div>
            ) : history.length === 0 ? (
              <div className="rv-empty">
                <span>No commits yet.</span>
              </div>
            ) : (
              history.map((c, i) => {
                const unpushed = !!ov && (ov.upstream === null || ov.upstreamGone ? false : i < ov.ahead);
                return (
                  <div key={c.sha} className={`repo-commit-row ${unpushed ? "repo-commit-unpushed" : ""}`}>
                    <button className="btn-plain repo-commit-main" onClick={() => onReviewCommit(c)} {...hint(`${c.message}\n\n${c.sha} · ${c.author}`)}>
                      <span className="repo-commit-msg">{c.message}</span>
                      <span className="repo-commit-meta">
                        {unpushed && <span className="repo-commit-flag">↑</span>}
                        {c.author} · {c.when}
                      </span>
                    </button>
                    {i === 0 && unpushed && (
                      <button
                        className="icon-button"
                        aria-label="Undo commit"
                        disabled={busy !== null}
                        onClick={() => run("Undo commit", () => invoke("repo_undo_commit", { name: repo }), "Commit undone — its changes are back in Changes")}
                        {...hint("Undo this commit (keeps its changes)")}
                      >
                        <UndoIcon size={13} />
                      </button>
                    )}
                  </div>
                );
              })
            )}
          </div>
        </section>
      </div>

      <div className="repo-grid">
        {/* ---------- Branches ---------- */}
        <section className="repo-panel">
          <div className="repo-panel-head">
            <h3>
              Branches {ov && <span className="section-count">{locals.length}</span>}
            </h3>
            <label className="repo-filter">
              <SearchIcon size={12} />
              <input placeholder="Filter" value={branchFilter} onChange={(e) => setBranchFilter(e.target.value)} />
            </label>
          </div>
          <div className="repo-branches">
            {!ov ? (
              <div className="repo-branch">
                <Skeleton w="60%" h={12} />
              </div>
            ) : (
              <>
                {locals.filter(matches).map((b) => (
                  <BranchRow
                    key={b.name}
                    b={b}
                    owner={worktreeOwner(b)}
                    busy={busy !== null}
                    onSwitch={() => requestSwitch(b.name)}
                    onDelete={() => deleteBranch(b)}
                    onOpenWorkspace={(name) => {
                      const w = workspaces.find((x) => x.name === name);
                      if (w) onOpenWorkspace(w);
                    }}
                  />
                ))}
                {remotes.length > 0 && (
                  <button className="btn-plain repo-remote-toggle" onClick={() => setShowRemote((s) => !s)}>
                    <ChevronIcon size={10} /> {showRemote ? "Hide" : "Show"} {remotes.length} branch{remotes.length === 1 ? "" : "es"} only on origin
                  </button>
                )}
                {(showRemote || filterText) &&
                  remotes.filter(matches).map((b) => (
                    <BranchRow key={`r:${b.name}`} b={b} owner={null} busy={busy !== null} onSwitch={() => requestSwitch(b.name)} />
                  ))}
              </>
            )}
          </div>
        </section>

        {/* ---------- Workspaces, worktrees, stashes ---------- */}
        <section className="repo-panel">
          <div className="repo-panel-head">
            <h3>
              Workspaces {repoWorkspaces.length > 0 && <span className="section-count">{repoWorkspaces.length}</span>}
            </h3>
          </div>
          <div className="repo-side-list">
            {repoWorkspaces.length === 0 ? (
              <div className="rv-empty">
                <span>No workspace uses {repo}. Create one from the Dashboard to work on a branch in its own worktree.</span>
              </div>
            ) : (
              repoWorkspaces.map((w) => (
                <button key={w.name} className="btn-plain repo-side-row" onClick={() => onOpenWorkspace(w)}>
                  <SatelliteIcon size={13} />
                  <span className="repo-side-name">{w.name}</span>
                  <span className="repo-side-meta">{w.branch}</span>
                </button>
              ))
            )}
            {otherWorktrees.map((w) => (
              <div key={w.path} className="repo-side-row repo-side-static" {...hint(w.path)}>
                <BranchIcon size={13} />
                <span className="repo-side-name">{w.path.split(/[\\/]/).pop()}</span>
                <span className="repo-side-meta">{w.missing ? "folder deleted" : w.branch ?? "detached"}</span>
                {w.missing && (
                  <button className="secondary btn-mini" onClick={() => run("Prune worktrees", () => invoke("repo_prune_worktrees", { name: repo }))}>
                    Prune
                  </button>
                )}
              </div>
            ))}
          </div>

          {ov && ov.stashes.length > 0 && (
            <>
              <div className="repo-panel-head repo-panel-subhead">
                <h3>
                  Stashes <span className="section-count">{ov.stashes.length}</span>
                </h3>
              </div>
              <div className="repo-side-list">
                {ov.stashes.map((s) => (
                  <div key={s.index} className="repo-side-row repo-side-static">
                    <StashIcon size={13} />
                    <span className="repo-side-name">{s.message}</span>
                    <span className="repo-side-meta">{s.when}</span>
                    <button className="secondary btn-mini" disabled={busy !== null} onClick={() => run("Restore stash", () => invoke("repo_stash_pop", { name: repo, index: s.index }), "Stash restored")}>
                      Restore
                    </button>
                    <button
                      className="icon-button icon-button-danger"
                      aria-label="Drop stash"
                      disabled={busy !== null}
                      onClick={() =>
                        setConfirm({
                          title: "Drop stash",
                          message: `Drop "${s.message}"? Its changes are lost.`,
                          label: "Drop",
                          danger: true,
                          run: () => run("Drop stash", () => invoke("repo_stash_drop", { name: repo, index: s.index })),
                        })
                      }
                    >
                      <TrashIcon size={13} />
                    </button>
                  </div>
                ))}
              </div>
            </>
          )}
        </section>
      </div>

      {switchReq && ov && (
        <div className="modal-overlay" onMouseDown={() => setSwitchReq(null)}>
          <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
            <div className="modal-body">
              <h3>Switch to {switchReq.branch}</h3>
              <p>
                You have {ov.changes} uncommitted change{ov.changes === 1 ? "" : "s"} on <strong>{branch}</strong>. What should happen to
                {ov.changes === 1 ? " it" : " them"}?
              </p>
            </div>
            <div className="modal-footer">
              <button type="button" className="secondary" onClick={() => setSwitchReq(null)}>
                Cancel
              </button>
              <button
                type="button"
                className="secondary"
                onClick={() => {
                  const target = switchReq.branch;
                  setSwitchReq(null);
                  run("Switch branch", () => invoke("repo_switch", { name: repo, branch: target, stash: false }), `Switched to ${target} with your changes`);
                }}
              >
                Bring them to {switchReq.branch}
              </button>
              <button
                type="button"
                autoFocus
                onClick={() => {
                  const target = switchReq.branch;
                  setSwitchReq(null);
                  run("Switch branch", () => invoke("repo_switch", { name: repo, branch: target, stash: true }), `Switched to ${target}; your changes are stashed`);
                }}
              >
                Stash and switch
              </button>
            </div>
          </div>
        </div>
      )}

      {confirm && (
        <ConfirmDialog
          title={confirm.title}
          message={confirm.message}
          confirmLabel={confirm.label}
          danger={confirm.danger}
          onConfirm={() => {
            const c = confirm;
            setConfirm(null);
            void c.run();
          }}
          onClose={() => setConfirm(null)}
        />
      )}

      {prCreateOpen && branch && (
        <PrsModal
          workspace={scope}
          base={defaultBranch}
          repos={[{ repo, prCommits: 1 }]}
          defaultTitle={branch}
          onOpenPrs={(list) => {
            setPrCreateOpen(false);
            load();
            onOpenPrList(list);
          }}
          onClose={() => setPrCreateOpen(false)}
          onError={onError}
        />
      )}

      {merging && (
        <MergeModal workspace={scope} base={defaultBranch} prs={merging} onClose={() => setMerging(null)} onSettled={fetchOrigin} />
      )}

      {sessionMenu && (
        <ContextMenu
          x={sessionMenu.x}
          y={sessionMenu.y}
          items={SESSION_ITEMS.map((s): MenuItem => ({ label: s.label, onSelect: () => onNewSession(s.tabLabel, s.cmd) }))}
          onClose={() => setSessionMenu(null)}
        />
      )}
    </div>
  );
}

const SESSION_ITEMS: { label: string; tabLabel: string; cmd: string | null }[] = [
  { label: "Terminal", tabLabel: "shell", cmd: null },
  { label: "Claude Code", tabLabel: "claude", cmd: "claude" },
  { label: "Open Code", tabLabel: "opencode", cmd: "opencode" },
  { label: "OMP", tabLabel: "omp", cmd: "omp" },
];

function RepoHeader({ repo, ov, children }: { repo: string; ov: RepoOverview | null; children?: React.ReactNode }) {
  return (
    <header className="ws-head">
      <div className="ws-head-main">
        <h2 className="repo-title">
          <RepoIcon size={18} /> {repo}
        </h2>
        <div className="ws-head-meta">
          {ov?.ownerRepo && (
            <button className="dash-card-link repo-gh-link" onClick={() => openUrl(`https://github.com/${ov.ownerRepo}`).catch(() => null)}>
              <GitHubIcon size={11} /> {ov.ownerRepo}
            </button>
          )}
          {ov && <span className="ws-meta-muted repo-path">{ov.path}</span>}
        </div>
      </div>
      {children && <div className="ws-head-tools">{children}</div>}
    </header>
  );
}

function BranchRow({
  b,
  owner,
  busy,
  onSwitch,
  onDelete,
  onOpenWorkspace,
}: {
  b: BranchInfo;
  owner: string | null;
  busy: boolean;
  onSwitch: () => void;
  onDelete?: () => void;
  onOpenWorkspace?: (name: string) => void;
}) {
  return (
    <div className={`repo-branch ${b.current ? "repo-branch-current" : ""}`}>
      <BranchIcon size={13} />
      <span className="repo-branch-name">{b.name}</span>
      {b.current && <span className="repo-pill repo-pill-accent">current</span>}
      {b.remoteOnly && <span className="repo-pill">origin</span>}
      {b.gone && <span className="repo-pill repo-pill-merged">deleted on origin</span>}
      {owner && (
        <button className="btn-plain repo-pill repo-pill-link" onClick={() => onOpenWorkspace?.(owner)}>
          in {owner}
        </button>
      )}
      {b.ahead > 0 && <span className="repo-sync">↑{b.ahead}</span>}
      {b.behind > 0 && <span className="repo-sync">↓{b.behind}</span>}
      <span className="repo-branch-when">{b.updated}</span>
      <span className="repo-branch-actions">
        {!b.current && !b.worktree && (
          <button className="secondary btn-mini" disabled={busy} onClick={onSwitch}>
            Switch
          </button>
        )}
        {onDelete && !b.current && !b.worktree && (
          <button className="icon-button icon-button-danger" aria-label={`Delete ${b.name}`} disabled={busy} onClick={onDelete}>
            <TrashIcon size={13} />
          </button>
        )}
      </span>
    </div>
  );
}

/** Branch picker: filter as you type, switch, or create a branch from the
 *  typed name (from the current branch or a fresh default branch). */
function BranchSwitcher({
  ov,
  ownerOf,
  onSwitch,
  onCreate,
  onClose,
}: {
  ov: RepoOverview;
  ownerOf: (b: BranchInfo) => string | null;
  onSwitch: (branch: string) => void;
  onCreate: (name: string, from: string | null) => void;
  onClose: () => void;
}) {
  const [q, setQ] = useState("");
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const text = q.trim().toLowerCase();
  const list = useMemo(() => ov.branches.filter((b) => !text || b.name.toLowerCase().includes(text)).slice(0, 60), [ov.branches, text]);
  const exact = ov.branches.some((b) => b.name === q.trim());
  const base = ov.defaultBranch ?? "main";

  return (
    <div className="branch-switcher" ref={ref}>
      <div className="branch-switcher-search">
        <SearchIcon size={13} />
        <input
          autoFocus
          placeholder="Filter or name a new branch"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key !== "Enter") return;
            if (exact) onSwitch(q.trim());
            else if (q.trim()) onCreate(q.trim(), null);
          }}
        />
      </div>
      {q.trim() && !exact && (
        <div className="branch-switcher-create">
          <button className="btn-plain" onClick={() => onCreate(q.trim(), null)}>
            <PlusIcon size={12} /> Create <strong>{q.trim()}</strong> from {ov.branch ?? "HEAD"}
          </button>
          {/* A fresh base: the local one may lag behind origin. */}
          {(ov.branch !== base || ov.behind > 0) && (
            <button className="btn-plain" onClick={() => onCreate(q.trim(), `origin/${base}`)}>
              <PlusIcon size={12} /> Create <strong>{q.trim()}</strong> from origin/{base}
            </button>
          )}
        </div>
      )}
      <div className="branch-switcher-list">
        {list.length === 0 && <div className="branch-switcher-empty">No branch matches</div>}
        {list.map((b) => {
          const owner = ownerOf(b);
          const blocked = !!b.worktree;
          return (
            <button
              key={`${b.remoteOnly ? "r" : "l"}:${b.name}`}
              className={`btn-plain branch-switcher-item ${b.current ? "active" : ""}`}
              disabled={blocked}
              onClick={() => onSwitch(b.name)}
              title={blocked ? `Checked out in ${owner ? `workspace ${owner}` : b.worktree}` : undefined}
            >
              <BranchIcon size={12} />
              <span className="branch-switcher-name">{b.name}</span>
              {b.remoteOnly && <span className="repo-pill">origin</span>}
              {owner && <span className="repo-pill">in {owner}</span>}
              <span className="branch-switcher-when">{b.updated}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

/** What the sync button does next, GitHub Desktop style. */
function syncAction(ov: RepoOverview | null, fetching: boolean) {
  const fetched = ov?.lastFetch ? `Last fetched ${ago(ov.lastFetch)}` : "Never fetched";
  if (!ov || fetching) return { kind: "fetch" as const, label: "Fetch origin", detail: fetching ? "Fetching…" : fetched, hint: "Check origin for new commits", primary: false, rebase: false };
  if (ov.branch && (!ov.upstream || ov.upstreamGone))
    return { kind: "publish" as const, label: "Publish branch", detail: "Push it to origin", hint: `Push ${ov.branch} to origin and track it`, primary: true, rebase: false };
  if (ov.behind > 0)
    return {
      kind: "pull" as const,
      label: `Pull origin ↓${ov.behind}${ov.ahead > 0 ? ` ↑${ov.ahead}` : ""}`,
      detail: ov.ahead > 0 ? "Diverged: your commits go on top" : fetched,
      hint: ov.ahead > 0 ? "Rebase your local commits on top of origin" : "Fast-forward to origin",
      primary: true,
      rebase: ov.ahead > 0,
    };
  if (ov.ahead > 0)
    return { kind: "push" as const, label: `Push origin ↑${ov.ahead}`, detail: fetched, hint: `Push ${ov.ahead} commit${ov.ahead === 1 ? "" : "s"} to ${ov.upstream}`, primary: true, rebase: false };
  return { kind: "fetch" as const, label: "Fetch origin", detail: fetched, hint: "Check origin for new commits", primary: false, rebase: false };
}

function ago(unix: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - unix));
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

function capitalize(s: string) {
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/** Comparable path (git and Windows spell separators and case differently). */
function norm(p: string) {
  const s = p.replace(/\\/g, "/").replace(/\/$/, "");
  return navigator.userAgent.includes("Windows") ? s.toLowerCase() : s;
}
