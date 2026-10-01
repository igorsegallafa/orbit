import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CardRef, GrillOption } from "../types/config";
import { AgentFeed, useAgentFeed } from "./AgentFeed";

interface Question {
  id: string;
  text: string;
  options: GrillOption[];
}

interface Round {
  done: boolean;
  questions: Question[];
  summary: string;
}

interface Answer {
  question: string;
  answer: string;
}

/** The interview so far, kept per workspace so closing loses nothing. */
interface Saved {
  goal: string;
  answers: Answer[];
  /** Answers the agent hasn't received yet (a round that failed or was stopped). */
  unsent: Answer[];
  /** Agent conversation to continue (null: agents that can't resume). */
  session: string | null;
  /** The round being answered, and how far into it. */
  round: { questions: Question[]; current: number; answers: Answer[] } | null;
  rounds: number;
  /** Decisions to draft from, once the interview is done (editable). */
  summary: string | null;
}

type Phase = "setup" | "thinking" | "asking" | "summary" | "drafting";

interface Props {
  workspace: string;
  card?: CardRef;
  /** A PLAN.md exists: drafting replaces it (the old one is kept). */
  planExists: boolean;
  /** PLAN.md is ready (drafted or created): open it. */
  onPlanReady: () => void;
  /** Plan in an interactive agent session seeded with `prompt`. */
  onPlanInTerminal: (prompt: string) => void;
  onClose: () => void;
}

const storeKey = (ws: string) => `orbit.plan-interview:${ws}`;

function loadSaved(ws: string): Saved | null {
  try {
    return JSON.parse(localStorage.getItem(storeKey(ws)) ?? "null");
  } catch {
    return null;
  }
}

function asSummary(list: Answer[]): string {
  return list.length ? list.map((a) => `- ${a.question}\n  → ${a.answer}`).join("\n") : "- No answers were given.";
}

/**
 * Plan this feature: from the linked card and/or a goal typed here, the
 * agent interviews the developer then drafts PLAN.md, drafts it right away,
 * plans with them in a terminal session, or the developer writes it from a
 * template. Every agent step shows live; everything can be stopped; the
 * interview survives closing the dialog.
 */
export function PlanModal({ workspace, card, planExists, onPlanReady, onPlanInTerminal, onClose }: Props) {
  const saved = useRef(loadSaved(workspace)).current;
  const [goal, setGoal] = useState(saved?.goal ?? "");
  const [answers, setAnswers] = useState<Answer[]>(saved?.answers ?? []);
  const [unsent, setUnsent] = useState<Answer[]>(saved?.unsent ?? []);
  const [session, setSession] = useState<string | null>(saved?.session ?? null);
  const [round, setRound] = useState<Saved["round"]>(saved?.round ?? null);
  const [rounds, setRounds] = useState(saved?.rounds ?? 0);
  const [summary, setSummary] = useState<string | null>(saved?.summary ?? null);
  const [resumable, setResumable] = useState(!!saved && (saved.answers.length > 0 || !!saved.round || saved.summary !== null));
  const [phase, setPhase] = useState<Phase>("setup");
  const [value, setValue] = useState("");
  const [run, setRun] = useState<string | null>(null);
  const [startedAt, setStartedAt] = useState(Date.now());
  const [error, setError] = useState<{ message: string; retry: () => void } | null>(null);
  const feed = useAgentFeed(run);
  const running = phase === "thinking" || phase === "drafting";

  // Keep the interview on disk as it goes.
  useEffect(() => {
    const data: Saved = { goal, answers, unsent, session, round, rounds, summary };
    const empty = !answers.length && !round && summary === null;
    try {
      if (empty) localStorage.removeItem(storeKey(workspace));
      else localStorage.setItem(storeKey(workspace), JSON.stringify(data));
    } catch {
      // Not remembered; the dialog still works.
    }
  }, [workspace, goal, answers, unsent, session, round, rounds, summary]);

  const needsGoal = !card && !goal.trim();

  const begin = () => {
    const id = `plan:${workspace}:${Date.now()}`;
    setRun(id);
    setStartedAt(Date.now());
    setError(null);
    return id;
  };

  const cancelRun = () => {
    if (run) invoke("agent_cancel", { run }).catch(() => null);
  };

  // ---- interview ----
  const ask = async (all: Answer[], fresh: Answer[], finish: boolean) => {
    const id = begin();
    setPhase("thinking");
    try {
      const turn = await invoke<{ round: Round; session: string | null }>("plan_interview", {
        run: id,
        name: workspace,
        goal,
        answers: all,
        newAnswers: fresh,
        session,
        finish,
      });
      setSession(turn.session ?? session);
      setUnsent([]);
      if (turn.round.done || !turn.round.questions.length) {
        setRound(null);
        setSummary(turn.round.summary?.trim() || asSummary(all));
        setPhase("summary");
      } else {
        setRound({ questions: turn.round.questions, current: 0, answers: [] });
        setRounds((n) => n + 1);
        setValue("");
        setPhase("asking");
      }
    } catch (e) {
      const msg = String(e);
      // Finishing can fall back on the raw answers.
      if (finish && msg !== "cancelled") {
        setSummary(asSummary(all));
        setPhase("summary");
        return;
      }
      // Back to the start, the answers kept: Resume (or Try again) sends
      // the ones the agent didn't get.
      setPhase("setup");
      setResumable(all.length > 0);
      if (msg !== "cancelled") setError({ message: msg, retry: () => ask(all, fresh, finish) });
    }
  };

  const startInterview = () => {
    setAnswers([]);
    setUnsent([]);
    setSession(null);
    setRound(null);
    setRounds(0);
    setSummary(null);
    setResumable(false);
    ask([], [], false);
  };

  const resume = () => {
    setResumable(false);
    if (summary !== null) setPhase("summary");
    else if (round) setPhase("asking");
    else ask(answers, unsent, false);
  };

  const discard = () => {
    setAnswers([]);
    setUnsent([]);
    setSession(null);
    setRound(null);
    setRounds(0);
    setSummary(null);
    setResumable(false);
  };

  const question = round?.questions[round.current];

  const submitAnswer = () => {
    if (!round || !question || !value.trim()) return;
    const a = { question: question.text, answer: value.trim() };
    const all = [...answers, a];
    const pending = [...unsent, a];
    setAnswers(all);
    setUnsent(pending);
    setValue("");
    if (round.current + 1 < round.questions.length) {
      setRound({ ...round, current: round.current + 1, answers: [...round.answers, a] });
    } else {
      setRound(null);
      ask(all, pending, false);
    }
  };

  const finishNow = () => {
    setRound(null);
    ask(answers, unsent, true);
  };

  // ---- drafting ----
  const draft = async (decisions: string) => {
    const back: Phase = summary !== null ? "summary" : "setup";
    const id = begin();
    setPhase("drafting");
    try {
      await invoke<string>("plan_draft", { run: id, name: workspace, goal, decisions });
      discard();
      onPlanReady();
      onClose();
    } catch (e) {
      const msg = String(e);
      setPhase(back);
      if (msg !== "cancelled") setError({ message: msg, retry: () => draft(decisions) });
    }
  };

  const writeMyself = async () => {
    setError(null);
    try {
      await invoke<string>("plan_blank", { name: workspace, goal });
      onPlanReady();
      onClose();
    } catch (e) {
      setError({ message: String(e), retry: writeMyself });
    }
  };

  const inTerminal = async () => {
    setError(null);
    try {
      const prompt = await invoke<string>("plan_session_prompt", { name: workspace, goal });
      onPlanInTerminal(prompt);
      onClose();
    } catch (e) {
      setError({ message: String(e), retry: inTerminal });
    }
  };

  const close = () => {
    if (running) cancelRun();
    onClose();
  };

  const choices = [
    {
      key: "interview",
      title: "Interview me first",
      tag: "Recommended",
      text: "The agent explores the repos and asks you questions in rounds, then drafts the plan with your decisions.",
      go: startInterview,
      always: false,
    },
    {
      key: "draft",
      title: "Draft it now",
      text: "The agent explores and writes the plan in one go. Unclear points become stated assumptions you can edit.",
      go: () => draft(""),
      always: false,
    },
    {
      key: "terminal",
      title: "Plan in a terminal",
      text: "Talk it through freely with the agent in a session. It writes PLAN.md when you're both done.",
      go: inTerminal,
      always: false,
    },
    {
      key: "blank",
      title: planExists ? "Open the current plan" : "Write it myself",
      text: planExists ? "Edit PLAN.md by hand." : "Start from the plan template: context, then tasks per repo.",
      go: writeMyself,
      always: true,
    },
  ];

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : close}>
      <div className="modal plan-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="plan-modal-head">
          <h3>{planExists ? "Replan this feature" : "Plan this feature"}</h3>
          {phase !== "setup" && (
            <span className="plan-modal-step">
              {phase === "drafting"
                ? "Drafting the plan"
                : phase === "summary"
                  ? "Review the decisions"
                  : `Interview · round ${Math.max(1, rounds + (phase === "thinking" ? 1 : 0))}`}
            </span>
          )}
        </div>

        <div className="modal-body plan-modal-body">
          {error && (
            <div className="plan-error">
              <span>{error.message}</span>
              <button className="btn-mini" onClick={() => error.retry()}>
                Try again
              </button>
            </div>
          )}

          {phase === "setup" && (
            <>
              {card && (
                <div className="plan-card-context">
                  <span className="tag tag-info">{card.id}</span>
                  <span className="plan-card-title">{card.title}</span>
                </div>
              )}
              <label className="plan-goal">
                <span>{card ? "Anything to add?" : "What should this feature do?"}</span>
                <textarea
                  value={goal}
                  autoFocus
                  onChange={(e) => setGoal(e.target.value)}
                  placeholder={
                    card
                      ? "Optional: approach, constraints, what to skip… (the card is the starting point)"
                      : "Describe the feature: what it should do, for whom, constraints, what to leave out…"
                  }
                />
              </label>
              {resumable && (
                <div className="plan-resume">
                  <span>
                    You have an interview in progress ({answers.length} answer{answers.length === 1 ? "" : "s"}).
                  </span>
                  <button className="btn-mini" onClick={resume}>
                    Resume
                  </button>
                  <button className="btn-mini secondary" onClick={discard}>
                    Discard
                  </button>
                </div>
              )}
              {planExists && <p className="plan-note">Drafting again replaces PLAN.md; the current one is kept in .orbit/plans.</p>}
              <div className="plan-choices">
                {choices.map((c) => {
                  const blocked = needsGoal && !c.always;
                  return (
                    <button
                      key={c.key}
                      type="button"
                      className={`btn-plain plan-choice ${c.tag ? "plan-choice-main" : ""}`}
                      disabled={blocked}
                      title={blocked ? "Describe the feature first" : undefined}
                      onClick={c.go}
                    >
                      <span className="plan-choice-title">
                        {c.title}
                        {c.tag && <span className="plan-choice-tag">{c.tag}</span>}
                      </span>
                      <span className="plan-choice-text">{c.text}</span>
                    </button>
                  );
                })}
              </div>
            </>
          )}

          {phase === "thinking" && (
            <AgentFeed
              items={feed}
              startedAt={startedAt}
              waiting={rounds === 0 ? "Exploring the repositories before asking…" : "Reading your answers…"}
            />
          )}

          {phase === "asking" && round && question && (
            <div className="grill-step">
              <div className="grill-progress">
                Question {round.current + 1} of {round.questions.length}
              </div>
              <p className="grill-question">{question.text}</p>
              {question.options.length > 0 && (
                <div className="grill-options">
                  {question.options.map((opt) => (
                    <button
                      type="button"
                      key={opt.label}
                      className={`chip grill-opt ${opt.recommended ? "grill-opt-rec" : ""} ${value === opt.label ? "chip-active" : ""}`}
                      title={opt.description || undefined}
                      onClick={() => setValue(opt.label)}
                    >
                      {opt.recommended && <span className="grill-rec-mark">★</span>}
                      {opt.label}
                    </button>
                  ))}
                  {question.options[0]?.description && (
                    <p className="grill-opt-desc">
                      {question.options.find((o) => o.label === value)?.description ||
                        question.options.find((o) => o.recommended)?.description}
                    </p>
                  )}
                </div>
              )}
              <textarea
                className="grill-answer"
                autoFocus
                value={value}
                placeholder="Your answer: pick an option above or write your own"
                onChange={(e) => setValue(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                    e.preventDefault();
                    submitAnswer();
                  }
                }}
              />
              {answers.length > 0 && (
                <div className="grill-history">
                  {answers.slice(-3).map((a, i) => (
                    <div className="grill-history-item" key={i}>
                      <span className="grill-history-q">{a.question}</span>
                      <span className="grill-history-a">{a.answer}</span>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {phase === "summary" && (
            <div className="plan-summary">
              <p className="plan-note">What the plan will follow. Edit anything before drafting.</p>
              <textarea className="plan-summary-text" value={summary ?? ""} onChange={(e) => setSummary(e.target.value)} />
            </div>
          )}

          {phase === "drafting" && <AgentFeed items={feed} startedAt={startedAt} waiting="Exploring the repositories to write the plan…" />}
        </div>

        <div className="modal-footer">
          {running ? (
            <button type="button" className="secondary danger-outline" onClick={cancelRun}>
              Stop
            </button>
          ) : (
            <button type="button" className="secondary" onClick={close} title={answers.length ? "Your answers are kept" : undefined}>
              Close
            </button>
          )}
          {phase === "asking" && (
            <>
              <button
                type="button"
                className="secondary"
                onClick={finishNow}
                disabled={answers.length === 0}
                title="Skip the remaining questions: the agent summarizes what's decided"
              >
                I'm ready
              </button>
              <button type="button" onClick={submitAnswer} disabled={!value.trim()}>
                {round && round.current + 1 < round.questions.length ? "Next question" : "Submit round"}
              </button>
            </>
          )}
          {phase === "summary" && (
            // The summary alone drops the nuance of each answer: the drafter
            // gets the full interview too.
            <button type="button" onClick={() => draft(`${summary ?? ""}\n\nFull interview:\n${asSummary(answers)}`)}>
              Draft the plan
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
