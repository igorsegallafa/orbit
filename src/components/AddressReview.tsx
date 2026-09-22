import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ReviewData, ReviewThread } from "../types/review";
import { RepoStatus } from "../types/config";
import { CheckBox } from "./CheckBox";
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

function useElapsed(running: boolean) {
  const [s, setS] = useState(0);
  useEffect(() => {
    if (!running) return setS(0);
    const start = Date.now();
    const t = window.setInterval(() => setS(Math.floor((Date.now() - start) / 1000)), 1000);
    return () => window.clearInterval(t);
  }, [running]);
  return s >= 60 ? `${Math.floor(s / 60)}m ${s % 60}s` : `${s}s`;
}

/**
 * Work through the open review conversations of one of your PRs: the agent
 * applies them in the local worktree (one pass, no commit) and drafts a
 * short reply per conversation; you edit, then push, reply and resolve so
 * nothing gets handled twice.
 */
export function AddressReviewModal({
  workspace,
  repo,
  number,
  title,
  url,
  onClose,
  onSettled,
  onError,
}: {
  workspace: string;
  repo: string;
  number: number;
  title: string;
  url: string;
  onClose: () => void;
  onSettled: () => void;
  onError: (msg: string) => void;
}) {
  const [ownerRepo, setOwnerRepo] = useState<string | null>(null);
  const [threads, setThreads] = useState<ReviewThread[] | null>(null);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [phase, setPhase] = useState<"idle" | "agent" | "posting">("idle");
  const [dirty, setDirty] = useState(false);
  const [agentRan, setAgentRan] = useState(false);
  const elapsed = useElapsed(phase === "agent");

  const load = async () => {
    try {
      const or = ownerRepo ?? (await invoke<string>("ws_owner_repo", { workspace, repo }));
      setOwnerRepo(or);
      const data = await invoke<ReviewData>("pr_review_data", { ownerRepo: or, number, withLines: false });
      const open = data.threads.filter((t) => !t.isResolved);
      setThreads(open);
      setDrafts((prev) => Object.fromEntries(open.map((t) => [t.id, prev[t.id] ?? { include: true, reply: "", resolve: false }])));
    } catch (e) {
      onError(String(e));
      setThreads([]);
    }
  };

  const refreshDirty = () =>
    invoke<RepoStatus[]>("workspace_status", { name: workspace })
      .then((all) => setDirty(!!all.find((s) => s.repo === repo)?.dirty))
      .catch(() => null);

  useEffect(() => {
    load();
    refreshDirty();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const set = (id: string, patch: Partial<Draft>) => setDrafts((d) => ({ ...d, [id]: { ...d[id], ...patch } }));
  const selected = useMemo(() => (threads ?? []).filter((t) => drafts[t.id]?.include), [threads, drafts]);
  const toPost = selected.filter((t) => drafts[t.id]?.reply.trim() || drafts[t.id]?.resolve);
  const busy = phase !== "idle";

  const runAgent = async () => {
    setPhase("agent");
    try {
      const replies = await invoke<ThreadReply[]>("ws_address_review", { workspace, repo, number, threadIds: selected.map((t) => t.id) });
      setDrafts((d) => {
        const next = { ...d };
        for (const r of replies) {
          if (next[r.id]) next[r.id] = { ...next[r.id], reply: r.reply, status: r.status, resolve: r.status === "fixed" };
        }
        return next;
      });
      setAgentRan(true);
      const fixed = replies.filter((r) => r.status === "fixed").length;
      toast.success(`Agent went through ${replies.length} conversation${replies.length === 1 ? "" : "s"}`, {
        description: `${fixed} fixed · ${replies.length - fixed} answered. Check the replies before posting.`,
      });
      refreshDirty();
    } catch (e) {
      onError(String(e));
    } finally {
      setPhase("idle");
    }
  };

  /** Push first when there are changes, so "Done" replies point at code that exists. */
  const post = async (withPush: boolean) => {
    if (!ownerRepo) return;
    setPhase("posting");
    const t = toast.loading("Posting review replies…");
    try {
      if (withPush && dirty) {
        toast.update(t, "loading", "Committing and pushing…");
        const msg = await invoke<{ message: string }>("ws_commit_message", { workspace, repo });
        await invoke("ws_commit", { workspace, repo, message: msg.message });
        await invoke("ws_push", { workspace, repo });
      }
      let replied = 0;
      let resolved = 0;
      const failures: string[] = [];
      for (const th of toPost) {
        const d = drafts[th.id];
        try {
          if (d.reply.trim()) {
            await invoke("pr_reply", { ownerRepo, number, commentId: th.comments[0].id, body: d.reply.trim() });
            replied++;
          }
          if (d.resolve) {
            await invoke("pr_resolve_thread", { threadId: th.id, resolved: true });
            resolved++;
          }
        } catch (e) {
          failures.push(`${th.path}: ${String(e)}`);
        }
      }
      const summary = `${replied} repl${replied === 1 ? "y" : "ies"} · ${resolved} resolved${withPush && dirty ? " · changes pushed" : ""}`;
      if (failures.length) toast.update(t, "error", "Some replies failed", { description: `${summary}\n${failures.join("\n")}` });
      else toast.update(t, "success", "Review replies posted", { description: summary, action: { label: "Open PR", onClick: () => openUrl(url).catch(() => null) } });
      onSettled();
      await load();
      refreshDirty();
    } catch (e) {
      toast.update(t, "error", "Couldn't finish", { description: String(e) });
    } finally {
      setPhase("idle");
    }
  };

  const hasReplies = selected.some((t) => drafts[t.id]?.reply.trim());

  return (
    <div className="modal-overlay" onMouseDown={() => !busy && onClose()}>
      <div className="modal modal-address" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header ar-header">
          <h3>Review feedback</h3>
          <button className="btn-link ar-pr" onClick={() => openUrl(url).catch(() => null)}>
            {repo} #{number} · {title} ↗
          </button>
        </div>
        <div className="modal-body ar-body">
          {threads === null ? (
            <div className="ar-loading">
              <Skeleton w="70%" h={12} />
              <Skeleton w="90%" h={40} />
              <Skeleton w="80%" h={40} />
            </div>
          ) : threads.length === 0 ? (
            <div className="ar-empty">
              <strong>No open conversations</strong>
              <span>Everything on this PR is resolved.</span>
            </div>
          ) : (
            <>
              {phase === "agent" && (
                <div className="ar-running">
                  <span className="spinner" /> Agent is working through {selected.length} conversation{selected.length === 1 ? "" : "s"} in {repo}… {elapsed}
                </div>
              )}
              {threads.map((th) => {
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
                      {d.status && <span className={`rv-pill ${d.status === "fixed" ? "rv-pill-ok" : "rv-pill-pending"}`}>{d.status === "fixed" ? "Fixed" : "Answered"}</span>}
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
            </>
          )}
        </div>
        <div className="modal-footer ar-footer">
          <span className="ar-count">
            {threads && threads.length > 0 ? `${selected.length} of ${threads.length} selected${dirty ? ` · uncommitted changes in ${repo}` : ""}` : ""}
          </span>
          <button className="secondary" disabled={busy} onClick={onClose}>
            Close
          </button>
          {threads && threads.length > 0 && (
            <>
              <button className={hasReplies ? "secondary" : ""} disabled={busy || selected.length === 0} onClick={runAgent}>
                <SparkIcon size={13} /> {agentRan ? "Run again" : `Address ${selected.length} with AI`}
              </button>
              <button className={hasReplies ? "" : "secondary"} disabled={busy || toPost.length === 0} onClick={() => post(true)}>
                {dirty ? "Commit, push & reply" : "Post replies"}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
