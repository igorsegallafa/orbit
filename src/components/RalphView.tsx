import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Workspace } from "../types/config";
import { Prd, RalphEvent, RalphState, RunConfig, RunState, Story, reasonLabel } from "../types/ralph";
import { applyEvent, formatDuration, groupByIteration, IterationGroup } from "../lib/ralphFeed";
import { RalphPrdModal } from "./RalphPrdModal";
import { RalphRunModal, rememberedConfig } from "./RalphRunModal";
import { CheckBox } from "./CheckBox";
import { CheckIcon, CircleIcon, SparkIcon } from "./Icons";

interface Props {
  workspace: Workspace;
  /** A run finished: repo statuses (commits, ahead) changed. */
  onRunFinished?: () => void;
  onError: (msg: string) => void;
}

type PastRun = Omit<RunState, "events">;

/** Full-height Ralph tab: PRD stories on the left, live activity on the right. */
export function RalphView({ workspace, onRunFinished, onError }: Props) {
  const [repo, setRepo] = useState(workspace.repos[0]);
  const [state, setState] = useState<RalphState | null>(null);
  const [run, setRun] = useState<RunState | null>(null);
  const [prdOpen, setPrdOpen] = useState(false);
  const [runOpen, setRunOpen] = useState(false);
  const [editing, setEditing] = useState<Prd | null>(null);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [history, setHistory] = useState<PastRun[]>([]);
  const [viewing, setViewing] = useState<{ id: string; events: RalphEvent[] } | null>(null);
  const [atBottom, setAtBottom] = useState(true);
  const [, tick] = useState(0);
  const feedRef = useRef<HTMLDivElement>(null);
  const key = `${workspace.name}/${repo}`;

  const load = useCallback(async () => {
    try {
      const s = await invoke<RalphState>("ralph_state", { workspace: workspace.name, repo });
      setState(s);
      setRun(s.run);
      const runs = await invoke<PastRun[]>("ralph_runs", { workspace: workspace.name });
      setHistory(runs.filter((r) => r.repo === repo));
    } catch (e) {
      onError(String(e));
    }
  }, [workspace.name, repo, onError]);

  useEffect(() => {
    setState(null);
    setViewing(null);
    setEditing(null);
    load();
  }, [load]);

  useEffect(() => {
    const off = listen<{ key: string; event: RalphEvent }>("ralph-event", (e) => {
      if (e.payload.key !== key) return;
      const ev = e.payload.event;
      setRun((r) => (r ? applyEvent(r, ev) : r));
      if (ev.kind === "iteration_end" || ev.kind === "stopped" || ev.kind === "run_start") load();
      if (ev.kind === "stopped") onRunFinished?.();
    });
    return () => {
      off.then((f) => f());
    };
  }, [key, load, onRunFinished]);

  useEffect(() => {
    if (!run?.running) return;
    const t = window.setInterval(() => tick((n) => n + 1), 1000);
    return () => window.clearInterval(t);
  }, [run?.running]);

  const feedEvents = viewing?.events ?? run?.events ?? [];
  const scrollToEnd = () => feedRef.current?.scrollTo({ top: feedRef.current.scrollHeight, behavior: "smooth" });
  useEffect(() => {
    if (atBottom) feedRef.current?.scrollTo({ top: feedRef.current.scrollHeight });
  }, [feedEvents.length]);

  const start = async (config: RunConfig) => {
    setRunOpen(false);
    setViewing(null);
    setAtBottom(true);
    try {
      await invoke("ralph_start", { workspace: workspace.name, repo, config });
      await load();
    } catch (e) {
      onError(String(e));
    }
  };

  const savePrd = async (prd: Prd) => {
    try {
      await invoke("ralph_save_prd", { workspace: workspace.name, repo, prd });
      setEditing(null);
      await load();
    } catch (e) {
      onError(String(e));
    }
  };

  const prd = editing ?? state?.prd ?? null;
  const running = run?.running ?? false;
  // After a restart only the history on disk remains: show the last run.
  const last: PastRun | null = run ?? history[0] ?? null;
  const stories = [...(prd?.userStories ?? [])].sort((a, b) => a.priority - b.priority);
  const passed = stories.filter((s) => s.passes).length;
  const total = stories.length;
  const now = Date.now() / 1000;
  const waitingLimit = running && run?.limitUntil && run.limitUntil > now;
  const currentStory = running ? stories.find((s) => s.id === run?.currentStory) : undefined;

  const updateStory = (id: string, patch: Partial<Story>) =>
    setEditing((p) => p && { ...p, userStories: p.userStories.map((s) => (s.id === id ? { ...s, ...patch } : s)) });

  const move = (id: string, dir: -1 | 1) => {
    const order = stories.map((s) => s.id);
    const i = order.indexOf(id);
    const j = i + dir;
    if (j < 0 || j >= order.length) return;
    [order[i], order[j]] = [order[j], order[i]];
    setEditing((p) => p && { ...p, userStories: p.userStories.map((s) => ({ ...s, priority: order.indexOf(s.id) + 1 })) });
  };

  const addStory = () =>
    setEditing((p) => {
      if (!p) return p;
      const n = p.userStories.length + 1;
      return {
        ...p,
        userStories: [
          ...p.userStories,
          {
            id: `US-${String(n).padStart(3, "0")}`,
            title: "",
            description: "",
            acceptanceCriteria: ["Typecheck passes"],
            priority: n,
            passes: false,
            notes: "",
          },
        ],
      };
    });

  return (
    <div className="ralph-view">
      <header className="rv-header">
        <div className="rv-title">
          <span className="rv-logo">
            <SparkIcon size={16} />
          </span>
          <div>
            <h2>Ralph</h2>
            <span className="rv-sub">Autonomous loop · one story per agent run</span>
          </div>
          {workspace.repos.length > 1 && (
            <span className="mode-toggle seg rv-repos">
              {workspace.repos.map((r) => (
                <button key={r} className={`mode-btn ${r === repo ? "mode-active" : ""}`} onClick={() => setRepo(r)}>
                  {r}
                </button>
              ))}
            </span>
          )}
        </div>
        <div className="rv-actions">
          {running ? (
            <>
              <button
                className="secondary"
                title="Finish the current iteration, then stop"
                onClick={() => invoke("ralph_pause", { workspace: workspace.name, repo })}
              >
                Pause after iteration
              </button>
              <button
                className="secondary danger-outline"
                title="Kill the current iteration now; commits made so far stay"
                onClick={() => invoke("ralph_stop", { workspace: workspace.name, repo })}
              >
                Stop
              </button>
            </>
          ) : (
            <>
              {state?.prd && (
                <button className="secondary" onClick={() => setPrdOpen(true)}>
                  New PRD
                </button>
              )}
              {state?.prd && (
                <button disabled={passed === total || !!editing} onClick={() => setRunOpen(true)}>
                  ▶ {last ? "Run again" : "Start Ralph"}
                </button>
              )}
            </>
          )}
        </div>
      </header>

      {state && !state.prd ? (
        <div className="rv-empty">
          <div className="rv-empty-card">
            <span className="rv-empty-logo">
              <SparkIcon size={22} />
            </span>
            <h3>Plan it, then let Ralph build it</h3>
            <p>
              Ralph works through a PRD one small story at a time. Each story runs in a fresh agent session that
              implements it, runs the checks and commits, while you watch every step here.
            </p>
            <ol className="rv-steps">
              <li>
                <strong>Describe the feature</strong>
                <span>Orbit can interview you to settle scope and edge cases.</span>
              </li>
              <li>
                <strong>Review the stories</strong>
                <span>The PRD is split into small, ordered stories with checkable criteria.</span>
              </li>
              <li>
                <strong>Start Ralph</strong>
                <span>Set limits, then follow tool calls, commits and cost live.</span>
              </li>
            </ol>
            <button onClick={() => setPrdOpen(true)}>Create PRD for {repo}</button>
          </div>
        </div>
      ) : (
        prd && (
          <>
            <section className="rv-status">
              <div className="rv-status-main">
                {waitingLimit ? (
                  <span className="rv-state rv-state-warn">
                    <span className="rv-dot" /> Usage limit · retrying in {formatDuration((run!.limitUntil! - now) * 1000)}
                  </span>
                ) : running ? (
                  <span className="rv-state rv-state-run">
                    <span className="rv-dot" /> Running · iteration {run?.iteration || 1}
                    {currentStory && (
                      <span className="rv-state-story">
                        {currentStory.id} {currentStory.title}
                      </span>
                    )}
                  </span>
                ) : last ? (
                  <span className={`rv-state ${last.reason?.kind === "complete" ? "rv-state-ok" : "rv-state-idle"}`}>
                    <span className="rv-dot" /> {passed === total ? "All stories pass" : `Last run: ${reasonLabel(last.reason)}`}
                  </span>
                ) : (
                  <span className="rv-state rv-state-idle">
                    <span className="rv-dot" /> Ready to start
                  </span>
                )}
              </div>
              <div className="rv-stats">
                <Stat value={`${passed}/${total}`} label="stories" />
                {last && <Stat value={String(last.commits)} label={last.commits === 1 ? "commit" : "commits"} />}
                {last && last.costUsd > 0 && <Stat value={`$${last.costUsd.toFixed(2)}`} label="cost" />}
                {last && <Stat value={formatDuration(((last.finishedAt ?? now) - last.startedAt) * 1000)} label="time" />}
              </div>
            </section>

            <div className="rv-progress" aria-label={`${passed} of ${total} stories done`}>
              {stories.map((s) => (
                <span
                  key={s.id}
                  title={`${s.id} ${s.title}`}
                  className={`rv-seg ${s.passes ? "done" : ""} ${running && s.id === run?.currentStory ? "current" : ""}`}
                />
              ))}
            </div>

            <div className={`rv-body ${editing ? "editing" : ""}`}>
              <aside className="rv-stories">
                <div className="rv-col-head">
                  <span>Stories</span>
                  {editing ? (
                    <span className="rv-col-actions">
                      <button className="btn-mini secondary" onClick={() => setEditing(null)}>
                        Discard
                      </button>
                      <button className="btn-mini" onClick={() => savePrd(editing)}>
                        Save
                      </button>
                    </span>
                  ) : (
                    <button
                      className="btn-mini secondary"
                      disabled={running}
                      title={running ? "Stop Ralph to edit the PRD" : "Edit stories, criteria and order"}
                      onClick={() => state?.prd && setEditing(structuredClone(state.prd))}
                    >
                      Edit
                    </button>
                  )}
                </div>

                <div className="rv-stories-scroll">
                  {editing ? (
                    <div className="rv-edit">
                      <label className="rv-field">
                        <span>Rules for every story</span>
                        <textarea
                          rows={3}
                          value={editing.description}
                          placeholder="Context and constraints every story must follow"
                          onChange={(e) => setEditing({ ...editing, description: e.target.value })}
                        />
                      </label>
                      {stories.map((s, idx) => (
                        <div key={s.id} className="rv-edit-card">
                          <div className="rv-edit-head">
                            <span className="rv-story-id">{s.id}</span>
                            <input
                              value={s.title}
                              placeholder="Story title"
                              onChange={(e) => updateStory(s.id, { title: e.target.value })}
                            />
                            <button className="icon-button" title="Move up" disabled={idx === 0} onClick={() => move(s.id, -1)}>
                              ↑
                            </button>
                            <button
                              className="icon-button"
                              title="Move down"
                              disabled={idx === stories.length - 1}
                              onClick={() => move(s.id, 1)}
                            >
                              ↓
                            </button>
                            <button
                              className="icon-button icon-button-danger"
                              title="Delete story"
                              onClick={() => setEditing((p) => p && { ...p, userStories: p.userStories.filter((x) => x.id !== s.id) })}
                            >
                              ✕
                            </button>
                          </div>
                          <label className="rv-field">
                            <span>Description</span>
                            <textarea
                              rows={3}
                              value={s.description}
                              placeholder="As a …, I want … so that …"
                              onChange={(e) => updateStory(s.id, { description: e.target.value })}
                            />
                          </label>
                          <label className="rv-field">
                            <span>Acceptance criteria, one per line</span>
                            <textarea
                              rows={5}
                              value={s.acceptanceCriteria.join("\n")}
                              onChange={(e) => updateStory(s.id, { acceptanceCriteria: e.target.value.split("\n") })}
                              onBlur={() =>
                                updateStory(s.id, { acceptanceCriteria: s.acceptanceCriteria.map((x) => x.trim()).filter(Boolean) })
                              }
                            />
                          </label>
                          <label className="rv-field">
                            <span>Notes for the agent</span>
                            <input value={s.notes} placeholder="Optional" onChange={(e) => updateStory(s.id, { notes: e.target.value })} />
                          </label>
                          <label className="check-item">
                            <CheckBox label="Done" checked={s.passes} onChange={(v) => updateStory(s.id, { passes: v })} />
                            Done, Ralph skips it
                          </label>
                        </div>
                      ))}
                      <button className="rv-add-story" onClick={addStory}>
                        + Add story
                      </button>
                    </div>
                  ) : (
                    stories.map((s) => {
                      const open = expanded === s.id;
                      const current = running && run?.currentStory === s.id;
                      return (
                        <div
                          key={s.id}
                          className={`rv-story ${s.passes ? "done" : ""} ${current ? "current" : ""} ${open ? "open" : ""}`}
                        >
                          <button className="rv-story-row" onClick={() => setExpanded(open ? null : s.id)}>
                            <span className="rv-story-icon">
                              {current ? <span className="spinner" /> : s.passes ? <CheckIcon size={12} /> : <CircleIcon size={12} />}
                            </span>
                            <span className="rv-story-id">{s.id}</span>
                            <span className="rv-story-title">{s.title || "Untitled story"}</span>
                          </button>
                          {open && (
                            <div className="rv-story-detail">
                              {s.description && <p>{s.description}</p>}
                              {s.acceptanceCriteria.length > 0 && (
                                <ul>
                                  {s.acceptanceCriteria.map((c, i) => (
                                    <li key={i}>{c}</li>
                                  ))}
                                </ul>
                              )}
                              {s.notes && <p className="rv-story-notes">{s.notes}</p>}
                            </div>
                          )}
                        </div>
                      );
                    })
                  )}
                </div>

                {history.length > 0 && !editing && (
                  <div className="rv-history">
                    <div className="rv-col-head">
                      <span>Runs</span>
                    </div>
                    {history.slice(0, 8).map((h) => (
                      <button
                        key={h.id}
                        className={`rv-run ${viewing?.id === h.id ? "active" : ""}`}
                        onClick={async () => {
                          if (h.running) return setViewing(null);
                          try {
                            const events = await invoke<RalphEvent[]>("ralph_run_events", { workspace: workspace.name, id: h.id });
                            setViewing({ id: h.id, events });
                            setAtBottom(true);
                          } catch (e) {
                            onError(String(e));
                          }
                        }}
                      >
                        <span className={`rv-run-dot ${h.running ? "run" : h.reason?.kind === "complete" ? "ok" : ""}`} />
                        <span className="rv-run-when">{new Date(h.startedAt * 1000).toLocaleString([], { dateStyle: "short", timeStyle: "short" })}</span>
                        <span className="rv-run-meta">
                          {h.running ? "running" : `${h.commits} commit${h.commits === 1 ? "" : "s"}`}
                          {h.costUsd > 0 ? ` · $${h.costUsd.toFixed(2)}` : ""}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </aside>

              <section className="rv-activity">
                <div className="rv-col-head">
                  <span>{viewing ? "Past run" : "Activity"}</span>
                  {viewing && (
                    <button className="btn-mini secondary" onClick={() => setViewing(null)}>
                      Back to latest
                    </button>
                  )}
                </div>
                <div
                  className="rv-feed"
                  ref={feedRef}
                  onScroll={(e) => {
                    const el = e.currentTarget;
                    setAtBottom(el.scrollHeight - el.scrollTop - el.clientHeight < 40);
                  }}
                >
                  {feedEvents.length === 0 ? (
                    <div className="rv-feed-empty">
                      {running ? (
                        <>
                          <span className="spinner" /> Starting the agent…
                        </>
                      ) : (
                        "When Ralph runs, every file it reads or edits, every command, commit and passing story shows up here as it happens."
                      )}
                    </div>
                  ) : (
                    groupByIteration(feedEvents).map((g) => <IterationBlock key={g.n} group={g} live={running && !viewing} />)
                  )}
                </div>
                {!atBottom && feedEvents.length > 0 && (
                  <button className="rv-jump" onClick={scrollToEnd}>
                    ↓ Latest
                  </button>
                )}
              </section>
            </div>
          </>
        )
      )}

      {prdOpen && (
        <RalphPrdModal workspace={workspace} repo={repo} onCreated={() => load()} onClose={() => setPrdOpen(false)} onError={onError} />
      )}
      {runOpen && state?.prd && (
        <RalphRunModal
          workspace={workspace.name}
          repo={repo}
          prd={state.prd}
          initial={rememberedConfig(workspace.name, state.defaultConfig)}
          prompt={state.prompt}
          promptCustom={state.promptCustom}
          onStart={start}
          onPromptSaved={load}
          onClose={() => setRunOpen(false)}
          onError={onError}
        />
      )}
    </div>
  );
}

function Stat({ value, label }: { value: string; label: string }) {
  return (
    <span className="rv-stat">
      <strong>{value}</strong>
      <span>{label}</span>
    </span>
  );
}

const TOOL_KIND: Record<string, string> = {
  Read: "read",
  Grep: "search",
  Glob: "search",
  WebSearch: "search",
  WebFetch: "search",
  Edit: "edit",
  MultiEdit: "edit",
  Write: "edit",
  NotebookEdit: "edit",
  Bash: "run",
};

function IterationBlock({ group, live }: { group: IterationGroup; live: boolean }) {
  const end = group.end;
  const outcome = end?.outcome ? String(end.outcome).split("(")[0] : null;
  return (
    <div className="rv-iter">
      {group.n > 0 && (
        <div className="rv-iter-head">
          <span className="rv-iter-n">#{group.n}</span>
          <span className="rv-iter-story">
            {group.story ? (
              <>
                <span className="rv-story-id">{group.story.id}</span> {group.story.title}
              </>
            ) : (
              "Iteration"
            )}
          </span>
          {end ? (
            <span className="rv-iter-meta">
              {outcome && outcome !== "Done" && outcome !== "Complete" && <span className="rv-iter-bad">{outcome}</span>}
              {end.commits} commit{end.commits === 1 ? "" : "s"}
              {end.durationMs ? ` · ${formatDuration(end.durationMs)}` : ""}
              {end.costUsd ? ` · $${end.costUsd.toFixed(2)}` : ""}
            </span>
          ) : (
            live && (
              <span className="rv-iter-meta">
                <span className="spinner" /> working
              </span>
            )
          )}
        </div>
      )}
      <div className="rv-iter-body">
        {group.events.map((ev, i) => (
          <FeedLine key={i} ev={ev} />
        ))}
      </div>
    </div>
  );
}

function FeedLine({ ev }: { ev: RalphEvent }) {
  switch (ev.kind) {
    case "tool":
      return (
        <div className="rv-line rv-tool">
          <span className={`rv-chip rv-chip-${TOOL_KIND[ev.name] ?? "other"}`}>{ev.name}</span>
          <span className="rv-tool-summary" title={ev.summary}>
            {ev.summary}
          </span>
        </div>
      );
    case "text": {
      const text = String(ev.text).replace("<promise>COMPLETE</promise>", "").trim();
      return text ? <div className="rv-line rv-text">{inlineMarkdown(text)}</div> : null;
    }
    case "log":
      return <div className={`rv-line rv-log ${ev.stderr ? "err" : ""}`}>{ev.text}</div>;
    case "story_passed":
      return (
        <div className="rv-line rv-event rv-event-ok">
          <CheckIcon size={12} /> {ev.id} passes
        </div>
      );
    case "push":
      return (
        <div className={`rv-line rv-event ${ev.ok ? "" : "rv-event-bad"}`}>{ev.ok ? "↑ Pushed to origin" : `Push failed: ${ev.message}`}</div>
      );
    case "limit_wait":
      return (
        <div className="rv-line rv-event rv-event-warn">
          Usage limit reached · retrying at {new Date(ev.until * 1000).toLocaleTimeString([], { timeStyle: "short" })}
        </div>
      );
    case "run_start":
      return (
        <div className="rv-line rv-event">
          Run started with {ev.agent} · {ev.model}
        </div>
      );
    case "stopped":
      return (
        <div className={`rv-line rv-event ${ev.reason?.kind === "complete" ? "rv-event-ok" : ""}`}>
          {reasonLabel(ev.reason)} · {ev.iterations} iteration{ev.iterations === 1 ? "" : "s"}
        </div>
      );
    default:
      return null;
  }
}

/** **bold** and `code` as React nodes. Agent output is untrusted, so it is
 *  never injected as HTML. */
function inlineMarkdown(text: string) {
  return text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g).map((part, i) =>
    part.startsWith("**") && part.endsWith("**") ? (
      <strong key={i}>{part.slice(2, -2)}</strong>
    ) : part.startsWith("`") && part.endsWith("`") ? (
      <code key={i}>{part.slice(1, -1)}</code>
    ) : (
      part
    ),
  );
}
