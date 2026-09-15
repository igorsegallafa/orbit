import { useEffect, useState } from "react";
import { GitChange, GitCommit } from "../types/config";
import { fetchCommitFiles, ReviewPane } from "./ReviewPane";
import { Skeleton } from "./Skeleton";

interface Props {
  workspace: string;
  repo: string;
  commit: GitCommit;
  onError: (msg: string) => void;
}

const STATUS_LABEL: Record<string, { letter: string; cls: string }> = {
  M: { letter: "M", cls: "git-status-modified" },
  A: { letter: "A", cls: "git-status-added" },
  D: { letter: "D", cls: "git-status-deleted" },
  U: { letter: "U", cls: "git-status-untracked" },
};

/**
 * Commit inspection tab: file list of the commit on the left, diff of the
 * selected file (sha^ vs sha) on the right.
 */
export function CommitReviewPane({ workspace, repo, commit, onError }: Props) {
  const [files, setFiles] = useState<GitChange[] | null>(null);
  const [selected, setSelected] = useState<string | null>(null);

  useEffect(() => {
    setFiles(null);
    setSelected(null);
    fetchCommitFiles(workspace, repo, commit.sha)
      .then((f) => {
        setFiles(f);
        setSelected(f[0]?.path ?? null);
      })
      .catch((e) => {
        setFiles([]);
        onError(String(e));
      });
  }, [workspace, repo, commit.sha, onError]);

  return (
    <div className="commit-review">
      <div className="commit-review-files">
        <div className="git-section-label" style={{ paddingLeft: 4 }}>
          {commit.sha}
        </div>
        <div className="commit-review-subject" title={commit.message}>
          {commit.message}
        </div>
        {files === null ? (
          <>
            <div className="modal-row"><Skeleton w={30} h={11} /> <Skeleton w="70%" h={11} /></div>
            <div className="modal-row"><Skeleton w={30} h={11} /> <Skeleton w="55%" h={11} /></div>
          </>
        ) : (
          files.map((f) => {
            const meta = STATUS_LABEL[f.status] ?? STATUS_LABEL.M;
            return (
              <button
                key={f.path}
                className={`tree-item git-change-row ${selected === f.path ? "tree-active" : ""}`}
                title={f.path}
                onClick={() => setSelected(f.path)}
              >
                <span className={`git-status-badge ${meta.cls}`}>{meta.letter}</span>
                <span className="git-change-name">{f.path.split("/").pop()}</span>
                {(f.added > 0 || f.deleted > 0) && (
                  <span className="git-change-stats">
                    {f.added > 0 && <span className="git-stat-add">+{f.added}</span>}
                    {f.deleted > 0 && <span className="git-stat-del">−{f.deleted}</span>}
                  </span>
                )}
              </button>
            );
          })
        )}
      </div>
      {selected !== null ? (
        <ReviewPane
          workspace={workspace}
          repo={repo}
          path={selected}
          sha={commit.sha}
          shaLabel={commit.sha}
          onError={onError}
        />
      ) : (
        <div className="editor-empty">Pick a file to see its diff in this commit</div>
      )}
    </div>
  );
}