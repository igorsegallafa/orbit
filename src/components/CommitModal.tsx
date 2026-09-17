import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckBox } from "./CheckBox";
import { SparkIcon } from "./Icons";
import { tooltip } from "./Tooltip";

interface RepoMsg {
  repo: string;
  message: string;
  status: "loading" | "ready" | "error" | "done";
  error?: string;
  included: boolean;
}

interface Props {
  workspace: string;
  /** Only the dirty repos — clean ones have nothing to commit. */
  repos: string[];
  onClose: () => void;
  onSettled: () => void;
  onError: (msg: string) => void;
}

/** AI-drafted commit messages: loads one message per dirty repo (in
 *  parallel), lets the user edit/include each, then commits the included
 *  ones in parallel. */
export function CommitModal({ workspace, repos, onClose, onSettled, onError }: Props) {
  const [msgs, setMsgs] = useState<RepoMsg[]>(() =>
    repos.map((r) => ({ repo: r, message: "", status: "ready" as const, included: true }))
  );
  const [committing, setCommitting] = useState(false);

  // AI message for ONE repo, on demand.
  const generate = (repo: string) => {
    setMsgs((prev) =>
      prev.map((r) => (r.repo === repo ? { ...r, status: "loading" } : r))
    );
    invoke<{ repo: string; message: string }>("ws_commit_message", {
      workspace,
      repo,
    })
      .then((m) =>
        setMsgs((prev) =>
          prev.map((r) =>
            r.repo === repo ? { ...r, message: m.message, status: "ready" } : r
          )
        )
      )
      .catch((e) =>
        setMsgs((prev) =>
          prev.map((r) =>
            r.repo === repo ? { ...r, status: "error", error: String(e) } : r
          )
        )
      );
  };

  const generateAll = () => repos.forEach((r) => generate(r));

  const ready = msgs.filter((m) => m.status === "ready" && m.included);
  const anyRunning = msgs.some((m) => m.status === "loading") || committing;

  const commit = async () => {
    setCommitting(true);
    let failures = 0;
    await Promise.all(
      ready.map(async (m) => {
        try {
          await invoke("ws_commit", { workspace, repo: m.repo, message: m.message });
          setMsgs((prev) =>
            prev.map((r) => (r.repo === m.repo ? { ...r, status: "done" } : r))
          );
        } catch (e) {
          failures++;
          setMsgs((prev) =>
            prev.map((r) =>
              r.repo === m.repo
                ? { ...r, status: "error", error: String(e), included: false }
                : r
            )
          );
        }
      })
    );
    setCommitting(false);
    if (failures === 0) {
      onSettled();
      onClose();
    } else {
      onError(`${failures} commit(s) failed — see the rows`);
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={anyRunning ? undefined : onClose}>
      <div className="modal ws-action-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <div className="ws-commit-titlebar">
            <div>
              <h3>Commit</h3>
              <p className="ws-commit-hint">
                Write the messages yourself or let the AI draft them — edit freely, uncheck to skip.
              </p>
            </div>
            <button
              type="button"
              className="secondary"
              onClick={generateAll}
              disabled={anyRunning || msgs.every((m) => m.status === "done")}
            >
              <SparkIcon size={12} /> Draft with AI
            </button>
          </div>
          <div className="ws-action-list">
            {msgs.map((m) => (
              <div key={m.repo} className={`ws-commit-card ws-${m.status}`}>
                <div className="ws-commit-head">
                  <CheckBox
                    checked={m.included && m.status !== "done"}
                    disabled={m.status === "done" || committing}
                    onChange={(c) =>
                      setMsgs((prev) =>
                        prev.map((r) =>
                          r.repo === m.repo ? { ...r, included: c } : r
                        )
                      )
                    }
                    label={`Include ${m.repo} in the commit`}
                  />
                  <span className="mono">{m.repo}</span>
                  <span className="ws-commit-state">
                    {m.status === "loading" && (
                      <>
                        <span className="spinner" /> drafting…
                      </>
                    )}
                    {m.status === "done" && <span className="ws-dot ws-dot-ok">✓</span>}
                    {m.status === "error" && <span className="ws-dot ws-dot-err">✗</span>}
                  </span>
                  {(m.status === "ready" || m.status === "error") && !committing && (
                    <button
                      type="button"
                      className="btn-mini"
                      onClick={() => generate(m.repo)}
                      onMouseEnter={(e) => tooltip.show("Ask the agent to draft this message", e)}
                      onMouseLeave={() => tooltip.hide()}
                    >
                      <SparkIcon size={12} />
                    </button>
                  )}
                </div>
                {m.status === "error" ? (
                  <div className="ws-commit-error">{m.error}</div>
                ) : m.status === "loading" ? (
                  <div className="ws-commit-placeholder">The agent is drafting…</div>
                ) : (
                  <textarea
                    className="ws-commit-msg"
                    placeholder="Commit message (write it yourself or draft with AI)"
                    value={m.message}
                    disabled={m.status === "done" || committing}
                    onChange={(e) =>
                      setMsgs((prev) =>
                        prev.map((r) =>
                          r.repo === m.repo ? { ...r, message: e.target.value } : r
                        )
                      )
                    }
                  />
                )}
              </div>
            ))}
          </div>
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose} disabled={anyRunning}>
            Cancel
          </button>
          <button
            type="button"
            autoFocus
            disabled={anyRunning || ready.length === 0}
            onClick={commit}
          >
            {committing
              ? "Committing…"
              : `Commit ${ready.length} ${ready.length === 1 ? "repo" : "repos"}`}
          </button>
        </div>
      </div>
    </div>
  );
}
