import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GitChange, GitCommit, Workspace } from "../types/config";
import { Skeleton } from "./Skeleton";
import { tooltip } from "./Tooltip";

interface Props {
  workspace: Workspace;
  /** Opens the diff review for a changed file in a new tab. */
  onReviewFile: (repo: string, path: string) => void;
  /** Opens commit inspection in a new tab. */
  onReviewCommit: (repo: string, commit: GitCommit) => void;
  onError: (msg: string) => void;
}

const STATUS_LABEL: Record<string, { letter: string; cls: string }> = {
  M: { letter: "M", cls: "git-status-modified" },
  A: { letter: "A", cls: "git-status-added" },
  D: { letter: "D", cls: "git-status-deleted" },
  U: { letter: "U", cls: "git-status-untracked" },
};

/**
 * Dock "Git" panel: working changes and recent commits for the selected
 * repo. Clicking a changed file opens the diff review tab.
 */
export function GitPanel({ workspace, onReviewFile, onReviewCommit, onError }: Props) {
  // Only while it belongs to the focused workspace (see FileTreePanel).
  const [picked, setRepo] = useState(workspace.repos[0] ?? "");
  const repo = workspace.repos.includes(picked) ? picked : (workspace.repos[0] ?? "");
  const [changes, setChanges] = useState<GitChange[] | null>(null);
  const [commits, setCommits] = useState<GitCommit[] | null>(null);

  const load = useCallback(async () => {
    if (!repo) return;
    setChanges(null);
    setCommits(null);
    try {
      const [chg, cmt] = await Promise.all([
        invoke<GitChange[]>("git_changes", { workspace: workspace.name, repo }),
        invoke<GitCommit[]>("git_commits", { workspace: workspace.name, repo }),
      ]);
      setChanges(chg);
      setCommits(cmt);
    } catch (e) {
      onError(String(e));
      setChanges([]);
      setCommits([]);
    }
  }, [workspace.name, repo, onError]);

  useEffect(() => {
    load();
  }, [load]);

  return (
    <div className="dock-panel">
      <div className="dock-toolbar">
        <select className="dock-select" value={repo} onChange={(e) => setRepo(e.target.value)}>
          {workspace.repos.map((r) => (
            <option key={r} value={r}>
              {r}
            </option>
          ))}
        </select>
        <button
          className="dock-icon"
          onMouseEnter={(e) => tooltip.show("Refresh git state", e)}
          onMouseLeave={() => tooltip.hide()}
          onClick={load}
        >
          ⟳
        </button>
      </div>
      <div className="dock-panel-body git-panel-body">
        <div className="git-section-label">Changes</div>
        {changes === null ? (
          <div className="modal-row"><Skeleton w={30} h={11} /> <Skeleton w="70%" h={11} /></div>
        ) : changes.length === 0 ? (
          <div className="git-empty">No changes — working tree clean</div>
        ) : (
          changes.map((c) => {
            const meta = STATUS_LABEL[c.status] ?? STATUS_LABEL.M;
            return (
              <button
                key={c.path}
                className="tree-item git-change-row"
                onMouseEnter={(e) => tooltip.show(`Review changes of ${c.path}`, e)}
                onMouseLeave={() => tooltip.hide()}
                onClick={() => onReviewFile(repo, c.path)}
              >
                <span className={`git-status-badge ${meta.cls}`}>{meta.letter}</span>
                <span className="git-change-name">
                  {c.path.split("/").pop()}
                  {c.path.includes("/") && <span className="git-change-dir">{c.path.slice(0, c.path.lastIndexOf("/"))}</span>}
                </span>
                {(c.added > 0 || c.deleted > 0) && (
                  <span className="git-change-stats">
                    {c.added > 0 && <span className="git-stat-add">+{c.added}</span>}
                    {c.deleted > 0 && <span className="git-stat-del">−{c.deleted}</span>}
                  </span>
                )}
              </button>
            );
          })
        )}

        <div className="git-section-label" style={{ marginTop: 12 }}>
          Commits
        </div>
        {commits === null ? (
          <div className="modal-row"><Skeleton w={40} h={11} /> <Skeleton w="60%" h={11} /></div>
        ) : commits.length === 0 ? (
          <div className="git-empty">No commits yet</div>
        ) : (
          commits.map((c) => (
            <button
              key={c.sha}
              className="git-commit-row"
              onMouseEnter={(e) => tooltip.show(`${c.message}\n\n${c.sha} · ${c.author}`, e)}
              onMouseLeave={() => tooltip.hide()}
              onClick={() => onReviewCommit(repo, c)}
            >
              <span className="git-commit-msg">{c.message}</span>
              <span className="git-commit-meta">
                {c.when} · {c.author}
              </span>
            </button>
          ))
        )}
      </div>
    </div>
  );
}