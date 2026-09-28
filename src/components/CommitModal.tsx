import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GitChange } from "../types/config";
import { CheckBox } from "./CheckBox";
import { ConfirmDialog } from "./ConfirmDialog";
import { STATUS_LABEL } from "./GitPanel";
import { ChevronRightIcon, SparkIcon } from "./Icons";
import { ReviewPane } from "./ReviewPane";
import { Skeleton } from "./Skeleton";
import { tooltip } from "./Tooltip";
import { toast } from "./Toast";
import { StatusIcon } from "./StatusIcon";

interface RepoCommit {
  repo: string;
  /** Working changes; null while loading. */
  files: GitChange[] | null;
  /** Paths going into the commit (all of them by default). */
  selected: Set<string>;
  message: string;
  status: "loading" | "ready" | "error" | "done";
  /** Commit failure (status "error"). */
  error?: string;
  /** AI draft failed: a note only, the message can still be typed. */
  draftError?: string;
  open: boolean;
}

interface Props {
  workspace: string;
  /** Only the dirty repos — clean ones have nothing to commit. */
  repos: string[];
  onClose: () => void;
  onSettled: () => void;
  onError: (msg: string) => void;
}

/**
 * Commit dialog: per repo, the changed files with a checkbox each (all in by
 * default) and a message (typed or AI-drafted from the selected files); the
 * file under the cursor shows its diff on the right. Commits the repos with
 * files selected, in parallel; unselected changes stay in the working tree.
 */
export function CommitModal({ workspace, repos, onClose, onSettled, onError }: Props) {
  const [items, setItems] = useState<RepoCommit[]>(() =>
    repos.map((repo, i) => ({ repo, files: null, selected: new Set(), message: "", status: "ready", open: i === 0 }))
  );
  const [focus, setFocus] = useState<{ repo: string; path: string } | null>(null);
  const [committing, setCommitting] = useState(false);
  /** Discard waiting for confirmation: which repo, which files. */
  const [discarding, setDiscarding] = useState<{ repo: string; paths: string[]; label: string } | null>(null);
  const [discardBusy, setDiscardBusy] = useState(false);

  const update = (repo: string, fn: (r: RepoCommit) => RepoCommit) =>
    setItems((prev) => prev.map((r) => (r.repo === repo ? fn(r) : r)));

  // Changed files of every repo, all selected; the first file shows its diff.
  useEffect(() => {
    repos.forEach((repo, i) =>
      invoke<GitChange[]>("git_changes", { workspace, repo })
        .then((files) => {
          update(repo, (r) => ({ ...r, files, selected: new Set(files.map((f) => f.path)) }));
          if (i === 0 && files[0]) setFocus((f) => f ?? { repo, path: files[0].path });
        })
        .catch((e) => update(repo, (r) => ({ ...r, files: [], status: "error", error: String(e) })))
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /** Re-reads one repo's changes (after a discard), keeping the selection
   *  of files that are still there. */
  const reload = async (repo: string) => {
    const files = await invoke<GitChange[]>("git_changes", { workspace, repo });
    const paths = new Set(files.map((f) => f.path));
    update(repo, (r) => ({ ...r, files, selected: new Set([...r.selected].filter((p) => paths.has(p))) }));
    setFocus((f) => (f?.repo === repo && !paths.has(f.path) ? (files[0] ? { repo, path: files[0].path } : null) : f));
  };

  const discard = async () => {
    if (!discarding) return;
    setDiscardBusy(true);
    try {
      await invoke("ws_discard", { workspace, repo: discarding.repo, paths: discarding.paths });
      toast.success(`Discarded ${discarding.label}`, { description: "New files went to the Trash." });
      await reload(discarding.repo);
      onSettled();
    } catch (e) {
      onError(String(e));
    } finally {
      setDiscardBusy(false);
      setDiscarding(null);
    }
  };

  /** Only the selected paths, or undefined when everything is in (git add -A). */
  const pathsOf = (r: RepoCommit) =>
    r.files && r.selected.size < r.files.length ? [...r.selected] : undefined;

  const generate = (repo: string) => {
    const r = items.find((x) => x.repo === repo);
    if (!r || r.selected.size === 0) return;
    update(repo, (x) => ({ ...x, status: "loading", draftError: undefined, error: undefined }));
    invoke<{ repo: string; message: string }>("ws_commit_message", { workspace, repo, paths: pathsOf(r) ?? null })
      .then((m) => update(repo, (x) => ({ ...x, message: m.message, status: "ready" })))
      .catch((e) => update(repo, (x) => ({ ...x, status: "ready", draftError: String(e) })));
  };

  const pending = items.filter((r) => r.status !== "done" && r.selected.size > 0);
  const ready = pending.filter((r) => r.status !== "loading" && r.message.trim());
  const missingMessage = pending.filter((r) => r.status !== "loading" && !r.message.trim());
  const anyRunning = items.some((r) => r.status === "loading") || committing;

  const generateAll = () => pending.filter((r) => r.status !== "loading").forEach((r) => generate(r.repo));

  const commit = async () => {
    setCommitting(true);
    let failures = 0;
    await Promise.all(
      ready.map(async (r) => {
        try {
          await invoke("ws_commit", { workspace, repo: r.repo, message: r.message, paths: pathsOf(r) ?? null });
          update(r.repo, (x) => ({ ...x, status: "done", open: false }));
        } catch (e) {
          failures++;
          update(r.repo, (x) => ({ ...x, status: "error", error: String(e) }));
        }
      })
    );
    setCommitting(false);
    if (failures === 0) {
      toast.success(`Committed ${ready.length} repo${ready.length === 1 ? "" : "s"}`, {
        description: ready.map((r) => `${r.repo}: ${r.message.split("\n")[0]}`).join("\n"),
      });
      onSettled();
      onClose();
    } else {
      onError(`${failures} commit(s) failed — see the repos marked failed`);
    }
  };

  const toggleFile = (repo: string, path: string, on: boolean) =>
    update(repo, (r) => {
      const selected = new Set(r.selected);
      if (on) selected.add(path);
      else selected.delete(path);
      return { ...r, selected };
    });

  const toggleRepo = (r: RepoCommit) =>
    update(r.repo, (x) => ({
      ...x,
      // Partial or none -> all; all -> none.
      selected: x.files && x.selected.size < x.files.length ? new Set(x.files.map((f) => f.path)) : new Set(),
    }));

  const commitLabel = committing
    ? "Committing…"
    : missingMessage.length > 0
      ? `Write a message for ${missingMessage[0].repo}`
      : ready.length === 0
        ? "Select files to commit"
        : `Commit ${ready.length} ${ready.length === 1 ? "repo" : "repos"}`;

  return (
    <>
    <div className="modal-overlay" onMouseDown={anyRunning || discarding ? undefined : onClose}>
      <div className="modal commit-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="commit-modal-head">
          <div>
            <h3>Commit changes</h3>
            <p className="ws-commit-hint">Pick the files that go in, write or draft each message. Unselected changes stay uncommitted.</p>
          </div>
          <button type="button" className="secondary" onClick={generateAll} disabled={anyRunning || pending.length === 0}>
            <SparkIcon size={12} /> Draft all with AI
          </button>
        </div>

        <div className="commit-modal-body">
          <div className="commit-modal-list">
            {items.map((r) => {
              const total = r.files?.length ?? 0;
              const count = r.selected.size;
              const done = r.status === "done";
              const locked = done || committing;
              return (
                <div key={r.repo} className={`commit-repo ${done ? "commit-repo-done" : ""}`}>
                  <div className="commit-repo-head">
                    <button
                      type="button"
                      className="btn-plain commit-repo-toggle"
                      aria-expanded={r.open}
                      onClick={() => update(r.repo, (x) => ({ ...x, open: !x.open }))}
                    >
                      <ChevronRightIcon size={12} className={r.open ? "ws-chevron ws-chevron-open" : "ws-chevron"} />
                    </button>
                    <CheckBox
                      checked={total > 0 && count === total}
                      mixed={count > 0 && count < total}
                      disabled={locked || total === 0}
                      onChange={() => toggleRepo(r)}
                      label={`Include every file of ${r.repo}`}
                    />
                    <span className="commit-repo-name mono" onClick={() => update(r.repo, (x) => ({ ...x, open: !x.open }))}>
                      {r.repo}
                    </span>
                    <span className="commit-repo-count">
                      {r.files === null ? "…" : `${count} of ${total} file${total === 1 ? "" : "s"}`}
                    </span>
                    <span className="commit-repo-state">
                      {r.status === "loading" && <StatusIcon kind="working" />}
                      {done && (
                        <>
                          <StatusIcon kind="ok" /> committed
                        </>
                      )}
                      {r.status === "error" && (
                        <>
                          <StatusIcon kind="error" /> failed
                        </>
                      )}
                    </span>
                    {!locked && total > 0 && (
                      <button
                        type="button"
                        className="btn-mini secondary danger-outline"
                        onClick={() =>
                          setDiscarding({ repo: r.repo, paths: (r.files ?? []).map((f) => f.path), label: `every change in ${r.repo}` })
                        }
                        onMouseEnter={(e) => tooltip.show("Discard every change in this repo", e)}
                        onMouseLeave={() => tooltip.hide()}
                      >
                        Discard all
                      </button>
                    )}
                    {!locked && (
                      <button
                        type="button"
                        className="btn-mini secondary"
                        disabled={r.status === "loading" || count === 0}
                        onClick={() => generate(r.repo)}
                        onMouseEnter={(e) => tooltip.show("Draft this message with AI, from the selected files", e)}
                        onMouseLeave={() => tooltip.hide()}
                      >
                        <SparkIcon size={12} />
                      </button>
                    )}
                  </div>

                  {r.open && (
                    <div className="commit-repo-files">
                      {r.files === null ? (
                        <div className="commit-file-skeleton">
                          <Skeleton w="70%" h={11} />
                          <Skeleton w="50%" h={11} />
                        </div>
                      ) : (
                        r.files.map((f) => {
                          const meta = STATUS_LABEL[f.status] ?? STATUS_LABEL.M;
                          const focused = focus?.repo === r.repo && focus.path === f.path;
                          return (
                            <div
                              key={f.path}
                              className={`commit-file ${focused ? "on" : ""}`}
                              role="button"
                              tabIndex={0}
                              onClick={() => setFocus({ repo: r.repo, path: f.path })}
                              onKeyDown={(e) => e.key === "Enter" && setFocus({ repo: r.repo, path: f.path })}
                            >
                              <span onClick={(e) => e.stopPropagation()}>
                                <CheckBox
                                  checked={r.selected.has(f.path)}
                                  disabled={locked}
                                  onChange={(on) => toggleFile(r.repo, f.path, on)}
                                  label={`Include ${f.path}`}
                                />
                              </span>
                              <span className={`git-status-badge ${meta.cls}`}>{meta.letter}</span>
                              <span className="git-change-name" title={f.path}>
                                {f.path.split("/").pop()}
                                {f.path.includes("/") && <span className="git-change-dir">{f.path.slice(0, f.path.lastIndexOf("/"))}</span>}
                              </span>
                              {(f.added > 0 || f.deleted > 0) && (
                                <span className="git-change-stats">
                                  {f.added > 0 && <span className="git-stat-add">+{f.added}</span>}
                                  {f.deleted > 0 && <span className="git-stat-del">−{f.deleted}</span>}
                                </span>
                              )}
                              {!locked && (
                                <button
                                  type="button"
                                  className="icon-button commit-file-discard"
                                  aria-label={`Discard changes to ${f.path}`}
                                  onClick={(e) => {
                                    e.stopPropagation();
                                    setDiscarding({ repo: r.repo, paths: [f.path], label: f.path.split("/").pop() ?? f.path });
                                  }}
                                  onMouseEnter={(e) => tooltip.show("Discard this file's changes", e)}
                                  onMouseLeave={() => tooltip.hide()}
                                >
                                  ↺
                                </button>
                              )}
                            </div>
                          );
                        })
                      )}
                    </div>
                  )}

                  {!done && (
                    r.status === "loading" ? (
                      <div className="ws-commit-placeholder commit-repo-msg">The agent is drafting…</div>
                    ) : (
                      <textarea
                        className="ws-commit-msg commit-repo-msg"
                        placeholder={count === 0 ? "No files selected — nothing to commit here" : `Commit message for ${r.repo}`}
                        value={r.message}
                        disabled={committing || count === 0}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && ready.length > 0 && missingMessage.length === 0 && !anyRunning) {
                            e.preventDefault();
                            commit();
                          }
                        }}
                        onChange={(e) => update(r.repo, (x) => ({ ...x, message: e.target.value }))}
                      />
                    )
                  )}
                  {done && <div className="commit-repo-done-msg">{r.message.split("\n")[0]}</div>}
                  {r.status === "error" && r.error && <div className="ws-commit-error">{r.error}</div>}
                  {r.draftError && r.status !== "loading" && (
                    <div className="ws-commit-note">Couldn't draft with AI ({r.draftError.replace(/^.*?: /, "")}). Write the message yourself.</div>
                  )}
                </div>
              );
            })}
          </div>

          <div className="commit-modal-diff">
            {focus ? (
              <ReviewPane key={`${focus.repo}/${focus.path}`} workspace={workspace} repo={focus.repo} path={focus.path} onError={onError} />
            ) : (
              <div className="commit-modal-empty">Select a file to see its changes</div>
            )}
          </div>
        </div>

        <div className="modal-footer">
          <span className="commit-modal-kbd">⌘/Ctrl + Enter to commit</span>
          <button type="button" className="secondary" onClick={onClose} disabled={anyRunning}>
            Cancel
          </button>
          <button type="button" disabled={anyRunning || ready.length === 0 || missingMessage.length > 0} onClick={commit}>
            {commitLabel}
          </button>
        </div>
      </div>
    </div>
        {discarding && (
          <ConfirmDialog
            title="Discard changes"
            message={`Discard ${discarding.label}? Modified files go back to their last commit; new files move to the Trash.`}
            confirmLabel="Discard"
            danger
            busy={discardBusy}
            busyLabel="Discarding…"
            onConfirm={discard}
            onClose={() => setDiscarding(null)}
          />
        )}
    </>
  );
}
