import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ReviewData, ReviewThread } from "../types/review";
import { GitChange, WsPrStatus } from "../types/config";
import { AgentFeed, useAgentFeed } from "./AgentFeed";
import { CheckBox } from "./CheckBox";
import { STATUS_LABEL } from "./GitPanel";
import { SafeMarkdown } from "./SafeMarkdown";
import { Skeleton } from "./Skeleton";
import { SparkIcon } from "./Icons";
import { toast } from "./Toast";

interface ThreadReply {
  id: string;
  status: "fixed" | "answered" | string;
  reply: string;
}

interface Draft {
  include: boolean;
  reply: string;
  resolve: boolean;
  status?: string;
}

/** A review's general comment (no line, no thread to reply to). */
interface Note {
  id: string;
  author: string;
  body: string;
}

/** One PR of the feature and its open feedback. */
interface Group {
  pr: WsPrStatus;
  ownerRepo: string;
  threads: ReviewThread[];
  notes: Note[];
}

/** The agent's pass over one repo. */
interface AgentRun {
  run: string;
  startedAt: number;
  status: "running" | "done" | "error";
  error?: string;
  /** Files the agent changed (not dirty before it ran), to commit. */
  files: GitChange[];
  /** Of those, the ones going into the commit. */
  commit: Set<string>;
  /** Files that already had changes before: left out of the commit. */
  preexisting: string[];
  message: string;
}

/** Latest general comment per reviewer, from reviews that asked for changes. */
function notesOf(data: ReviewData): Note[] {
  const latest = new Map<string, Note>();
  for (const r of data.reviews) {
    if (r.state !== "CHANGES_REQUESTED" || !r.body.trim() || r.author === data.viewer) continue;
    latest.set(r.author, { id: `${r.author}:${r.submittedAt ?? ""}`, author: r.author, body: r.body.trim() });
  }
  return [...latest.values()];
}

function GroupFeed({ run, startedAt }: { run: string; startedAt: number }) {
  const items = useAgentFeed(run);
  return <AgentFeed items={items} startedAt={startedAt} waiting="Reading the feedback and the code…" />;
}

/**
 * The feature's review feedback, every open PR at once: the agent applies
 * the selected conversations (one live run per repo), drafts a reply for
 * each; you check what it changed, then commit only its files, push, reply
 * and resolve.
 */
export function AddressReviewModal({
  workspace,
  prs,
  onClose,
  onSettled,
  onError,
}: {
  workspace: string;
  /** The workspace's open PRs. */
  prs: WsPrStatus[];
  onClose: () => void;
  onSettled: () => void;
  onError: (msg: string) => void;
}) {
  const [groups, setGroups] = useState<Group[] | null>(null);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [notesOn, setNotesOn] = useState<Record<string, boolean>>({});
  const [runs, setRuns] = useState<Record<string, AgentRun>>({});
  const [posting, setPosting] = useState(false);

  const load = async () => {
    const loaded = await Promise.all(
      prs.map(async (pr): Promise<Group | null> => {
        try {
          const ownerRepo = await invoke<string>("ws_owner_repo", { workspace, repo: pr.repo });
          const data = await invoke<ReviewData>("pr_review_data", { ownerRepo, number: pr.number, withLines: false });
          return { pr, ownerRepo, threads: data.threads.filter((t) => !t.isResolved), notes: notesOf(data) };
        } catch (e) {
          onError(`${pr.repo} #${pr.number}: ${e}`);
          return null;
        }
      })
    );
    const list = loaded.filter((g): g is Group => !!g && (g.threads.length > 0 || g.notes.length > 0));
    setGroups(list);
    setDrafts((prev) =>
      Object.fromEntries(list.flatMap((g) => g.threads.map((t) => [t.id, prev[t.id] ?? { include: true, reply: "", resolve: false }])))
    );
    setNotesOn((prev) => Object.fromEntries(list.flatMap((g) => g.notes.map((n) => [n.id, prev[n.id] ?? true]))));
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const set = (id: string, patch: Partial<Draft>) => setDrafts((d) => ({ ...d, [id]: { ...d[id], ...patch } }));
  const setRun = (repo: string, patch: Partial<AgentRun>) => setRuns((r) => ({ ...r, [repo]: { ...r[repo], ...patch } }));

  const selectedThreads = (g: Group) => g.threads.filter((t) => drafts[t.id]?.include);
  const selectedNotes = (g: Group) => g.notes.filter((n) => notesOn[n.id]);
  const totals = useMemo(() => {
    const all = groups ?? [];
    return {
      threads: all.reduce((n, g) => n + g.threads.length, 0),
      selected: all.reduce((n, g) => n + selectedThreads(g).length + selectedNotes(g).length, 0),
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groups, drafts, notesOn]);
  const agentRunning = Object.values(runs).some((r) => r.status === "running");
  const busy = agentRunning || posting;

  // ---- the agent, one live run per repo ----
  const address = async (g: Group) => {
    const threads = selectedThreads(g);
    const notes = selectedNotes(g);
    if (!threads.length && !notes.length) return;
    const repo = g.pr.repo;
    const run = `review:${workspace}:${repo}:${Date.now()}`;
    let before: Set<string>;
    try {
      before = new Set((await invoke<GitChange[]>("git_changes", { workspace, repo })).map((f) => f.path));
    } catch {
      before = new Set();
    }
    setRuns((r) => ({
      ...r,
      [repo]: { run, startedAt: Date.now(), status: "running", files: [], commit: new Set(), preexisting: [], message: "" },
    }));
    try {
      const replies = await invoke<ThreadReply[]>("ws_address_review", {
        run,
        workspace,
        repo,
        number: g.pr.number,
        threadIds: threads.map((t) => t.id),
        notes: notes.map((n) => `@${n.author}: ${n.body}`),
      });
      setDrafts((d) => {
        const next = { ...d };
        for (const r of replies) {
          if (next[r.id]) next[r.id] = { ...next[r.id], reply: r.reply, status: r.status, resolve: r.status === "fixed" };
        }
        return next;
      });
      const after = await invoke<GitChange[]>("git_changes", { workspace, repo });
      const files = after.filter((f) => !before.has(f.path));
      const preexisting = after.filter((f) => before.has(f.path)).map((f) => f.path);
      setRun(repo, { status: "done", files, commit: new Set(files.map((f) => f.path)), preexisting });
      if (files.length) {
        invoke<{ message: string }>("ws_commit_message", { workspace, repo, paths: files.map((f) => f.path) })
          .then((m) => setRuns((r) => (r[repo] && !r[repo].message ? { ...r, [repo]: { ...r[repo], message: m.message } } : r)))
          .catch(() => setRuns((r) => (r[repo] && !r[repo].message ? { ...r, [repo]: { ...r[repo], message: "fix: address review feedback" } } : r)));
      }
    } catch (e) {
      const msg = String(e);
      setRun(repo, { status: msg === "cancelled" ? "done" : "error", error: msg === "cancelled" ? undefined : msg });
    }
  };

  const addressAll = () => (groups ?? []).forEach((g) => void address(g));
  const stopAll = () => Object.values(runs).forEach((r) => r.status === "running" && invoke("agent_cancel", { run: r.run }).catch(() => null));

  // ---- commit, push, reply ----
  const toReply = (g: Group) => selectedThreads(g).filter((t) => drafts[t.id]?.reply.trim() || drafts[t.id]?.resolve);
  const toCommit = (repo: string) => {
    const r = runs[repo];
    return r && r.status === "done" ? r.files.filter((f) => r.commit.has(f.path)) : [];
  };
  const pending = (groups ?? []).filter((g) => toReply(g).length > 0 || toCommit(g.pr.repo).length > 0);
  const anyCommit = pending.some((g) => toCommit(g.pr.repo).length > 0);
  const missingMessage = pending.find((g) => toCommit(g.pr.repo).length > 0 && !runs[g.pr.repo]?.message.trim());

  const post = async () => {
    setPosting(true);
    const t = toast.loading("Posting review replies…");
    let replied = 0;
    let resolved = 0;
    let pushed = 0;
    const failures: string[] = [];
    for (const g of pending) {
      const repo = g.pr.repo;
      const files = toCommit(repo);
      try {
        // Push first, so "Done" replies point at code that exists.
        if (files.length) {
          toast.update(t, "loading", `Committing and pushing ${repo}…`);
          await invoke("ws_commit", { workspace, repo, message: runs[repo].message.trim(), paths: files.map((f) => f.path) });
          await invoke("ws_push", { workspace, repo });
          pushed++;
          setRun(repo, { files: [], commit: new Set() });
        }
      } catch (e) {
        failures.push(`${repo}: ${e}`);
        continue; // no replies claiming a fix that didn't land
      }
      for (const th of toReply(g)) {
        const d = drafts[th.id];
        try {
          if (d.reply.trim()) {
            await invoke("pr_reply", { ownerRepo: g.ownerRepo, number: g.pr.number, commentId: th.comments[0].id, body: d.reply.trim() });
            replied++;
          }
          if (d.resolve) {
            await invoke("pr_resolve_thread", { threadId: th.id, resolved: true });
            resolved++;
          }
        } catch (e) {
          failures.push(`${repo} ${th.path}: ${e}`);
        }
      }
    }
    const summary = `${replied} repl${replied === 1 ? "y" : "ies"} · ${resolved} resolved${pushed ? ` · ${pushed} repo${pushed === 1 ? "" : "s"} pushed` : ""}`;
    if (failures.length) toast.update(t, "error", "Some steps failed", { description: `${summary}\n${failures.join("\n")}` });
    else toast.update(t, "success", "Review feedback posted", { description: summary });
    setPosting(false);
    onSettled();
    await load();
  };

  return (
    <div className="modal-overlay" onMouseDown={() => !busy && onClose()}>
      <div className="modal modal-address" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header ar-header">
          <h3>Review feedback</h3>
          {groups && (
            <span className="ar-summary">
              {totals.threads} open conversation{totals.threads === 1 ? "" : "s"} · {groups.length} PR{groups.length === 1 ? "" : "s"}
            </span>
          )}
        </div>
        <div className="modal-body ar-body">
          {groups === null ? (
            <div className="ar-loading">
              <Skeleton w="70%" h={12} />
              <Skeleton w="90%" h={40} />
              <Skeleton w="80%" h={40} />
            </div>
          ) : groups.length === 0 ? (
            <div className="ar-empty">
              <strong>No open feedback</strong>
              <span>Every conversation on this feature's pull requests is resolved.</span>
            </div>
          ) : (
            groups.map((g) => {
              const repo = g.pr.repo;
              const run = runs[repo];
              const sel = selectedThreads(g).length + selectedNotes(g).length;
              const allOn = sel === g.threads.length + g.notes.length;
              return (
                <section key={`${repo}#${g.pr.number}`} className="ar-group">
                  <div className="ar-group-head">
                    <CheckBox
                      label={`Select all of ${repo}`}
                      checked={allOn}
                      mixed={sel > 0 && !allOn}
                      disabled={busy}
                      onChange={() => {
                        const on = !allOn;
                        setDrafts((d) => {
                          const next = { ...d };
                          for (const t of g.threads) next[t.id] = { ...next[t.id], include: on };
                          return next;
                        });
                        setNotesOn((n) => ({ ...n, ...Object.fromEntries(g.notes.map((x) => [x.id, on])) }));
                      }}
                    />
                    <span className="ar-group-repo mono">{repo}</span>
                    <button className="btn-link ar-pr" onClick={() => openUrl(g.pr.url).catch(() => null)}>
                      #{g.pr.number} · {g.pr.title} ↗
                    </button>
                  </div>

                  {g.notes.map((n) => (
                    <div key={n.id} className={`ar-thread ${notesOn[n.id] ? "" : "off"}`}>
                      <div className="ar-thread-head">
                        <CheckBox
                          label="Include this comment"
                          checked={!!notesOn[n.id]}
                          disabled={busy}
                          onChange={(v) => setNotesOn((x) => ({ ...x, [n.id]: v }))}
                        />
                        <span className="ar-loc">General comment · changes requested</span>
                      </div>
                      <div className="ar-comments">
                        <div className="ar-comment">
                          <strong>{n.author}</strong>
                          <SafeMarkdown content={n.body} />
                        </div>
                      </div>
                    </div>
                  ))}

                  {g.threads.map((th) => {
                    const d = drafts[th.id];
                    if (!d) return null;
                    const line = th.line ?? th.originalLine;
                    return (
                      <div key={th.id} className={`ar-thread ${d.include ? "" : "off"}`}>
                        <div className="ar-thread-head">
                          <CheckBox label="Include this conversation" checked={d.include} disabled={busy} onChange={(v) => set(th.id, { include: v })} />
                          <span className="ar-loc mono">
                            {th.path}
                            {line ? `:${line}` : ""}
                          </span>
                          {th.isOutdated && <span className="rv-pill">Outdated</span>}
                          {d.status && (
                            <span className={`rv-pill ${d.status === "fixed" ? "rv-pill-ok" : "rv-pill-pending"}`}>
                              {d.status === "fixed" ? "Fixed" : "Answered"}
                            </span>
                          )}
                        </div>
                        <div className="ar-comments">
                          {th.comments.map((c) => (
                            <div key={c.id} className="ar-comment">
                              <strong>{c.author}</strong>
                              <SafeMarkdown content={c.body} />
                            </div>
                          ))}
                        </div>
                        {d.include && (
                          <div className="ar-reply">
                            <textarea
                              rows={2}
                              value={d.reply}
                              disabled={busy}
                              placeholder="Reply (optional): short and objective"
                              onChange={(e) => set(th.id, { reply: e.target.value })}
                            />
                            <label className="ar-resolve">
                              <CheckBox label="Resolve conversation" checked={d.resolve} disabled={busy} onChange={(v) => set(th.id, { resolve: v })} />
                              Resolve conversation
                            </label>
                          </div>
                        )}
                      </div>
                    );
                  })}

                  {run?.status === "running" && <GroupFeed run={run.run} startedAt={run.startedAt} />}
                  {run?.status === "error" && <div className="plan-error">{run.error}</div>}
                  {run?.status === "done" && (
                    <div className="ar-changes">
                      {run.files.length === 0 ? (
                        <span className="ar-changes-none">The agent changed no files in {repo}.</span>
                      ) : (
                        <>
                          <div className="ar-changes-head">
                            Changes by the agent · {run.commit.size} of {run.files.length} to commit
                          </div>
                          {run.files.map((f) => {
                            const meta = STATUS_LABEL[f.status] ?? STATUS_LABEL.M;
                            return (
                              <label key={f.path} className="ar-change">
                                <CheckBox
                                  label={`Commit ${f.path}`}
                                  checked={run.commit.has(f.path)}
                                  disabled={busy}
                                  onChange={(on) => {
                                    const next = new Set(run.commit);
                                    if (on) next.add(f.path);
                                    else next.delete(f.path);
                                    setRun(repo, { commit: next });
                                  }}
                                />
                                <span className={`git-status-badge ${meta.cls}`}>{meta.letter}</span>
                                <span className="git-change-name">{f.path}</span>
                                {(f.added > 0 || f.deleted > 0) && (
                                  <span className="git-change-stats">
                                    {f.added > 0 && <span className="git-stat-add">+{f.added}</span>}
                                    {f.deleted > 0 && <span className="git-stat-del">−{f.deleted}</span>}
                                  </span>
                                )}
                              </label>
                            );
                          })}
                          <input
                            className="ar-commit-msg mono"
                            value={run.message}
                            disabled={busy}
                            placeholder="Commit message (drafting…)"
                            onChange={(e) => setRun(repo, { message: e.target.value })}
                          />
                        </>
                      )}
                      {run.preexisting.length > 0 && (
                        <span className="ar-changes-note">
                          Left out: {run.preexisting.join(", ")} already had your own uncommitted changes. Commit them from the
                          Commit dialog if they're part of this.
                        </span>
                      )}
                    </div>
                  )}
                </section>
              );
            })
          )}
        </div>
        <div className="modal-footer ar-footer">
          <span className="ar-count">{groups && totals.threads + groups.reduce((n, g) => n + g.notes.length, 0) > 0 ? `${totals.selected} selected` : ""}</span>
          {agentRunning ? (
            <button className="secondary danger-outline" onClick={stopAll}>
              Stop
            </button>
          ) : (
            <button className="secondary" disabled={posting} onClick={onClose}>
              Close
            </button>
          )}
          {groups && groups.length > 0 && (
            <>
              <button className={pending.length ? "secondary" : ""} disabled={busy || totals.selected === 0} onClick={addressAll}>
                <SparkIcon size={13} /> {Object.keys(runs).length ? "Run again" : `Address ${totals.selected} with AI`}
              </button>
              <button
                className={pending.length ? "" : "secondary"}
                disabled={busy || pending.length === 0 || !!missingMessage}
                title={missingMessage ? `Write a commit message for ${missingMessage.pr.repo}` : undefined}
                onClick={post}
              >
                {anyCommit ? "Commit, push & reply" : "Post replies"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
