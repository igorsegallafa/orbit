import { useEffect, useRef, useState } from "react";
import { toast } from "./Toast";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CardDetail, GrillOption, Workspace } from "../types/config";
import { Prd } from "../types/ralph";

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
  questionId: string;
  question: string;
  answer: string;
}

type Phase = "brief" | "interview" | "summary" | "writing";

interface Props {
  workspace: Workspace;
  repo: string;
  onCreated: (prd: Prd) => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

/** Brief → optional clarifying interview → agent writes the markdown PRD
 *  and scripts/ralph/prd.json (prd + ralph skills, in one step). */
export function RalphPrdModal({ workspace, repo, onCreated, onClose, onError }: Props) {
  const [phase, setPhase] = useState<Phase>("brief");
  const [brief, setBrief] = useState("");
  const [round, setRound] = useState<Round | null>(null);
  const [answers, setAnswers] = useState<Answer[]>([]);
  const [current, setCurrent] = useState(0);
  const [value, setValue] = useState("");
  const [loading, setLoading] = useState(false);
  const [summary, setSummary] = useState("");
  const [log, setLog] = useState<string[]>([]);
  const roundsDone = useRef(0);
  const logRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const card = workspace.card;
    if (!card) return;
    setBrief(`${card.title}\n\n${card.url}`);
    invoke<CardDetail>("integration_fetch_card", { kind: card.kind, id: card.id })
      .then((d) => setBrief(`${d.title}\n\n${d.description}`.trim()))
      .catch(() => null);
  }, [workspace.card]);

  useEffect(() => {
    const off = listen<string>("ralph-prd-progress", (e) => setLog((l) => [...l.slice(-400), e.payload]));
    return () => {
      off.then((f) => f());
    };
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [log]);

  const ask = async (list: Answer[], forceDone = false) => {
    setLoading(true);
    try {
      const r = await invoke<Round>("ralph_interview", {
        workspace: workspace.name,
        repo,
        brief,
        answers: list.map((a) => [a.question, a.answer]),
        roundsDone: roundsDone.current,
        maxRounds: forceDone ? roundsDone.current + 1 : null,
      });
      if (r.done || r.questions.length === 0) {
        setSummary(r.summary || list.map((a) => `- ${a.question}\n  → ${a.answer}`).join("\n"));
        setPhase("summary");
      } else {
        roundsDone.current += 1;
        setRound(r);
        setCurrent(0);
        setValue(r.questions[0].options.find((o) => o.recommended)?.label ?? "");
        setPhase("interview");
      }
    } catch (e) {
      onError(String(e));
      if (forceDone) {
        setSummary(list.map((a) => `- ${a.question}\n  → ${a.answer}`).join("\n"));
        setPhase("summary");
      }
    } finally {
      setLoading(false);
    }
  };

  const question = round?.questions[current];

  const submit = () => {
    if (!question || !value.trim()) return;
    const next = [...answers, { questionId: question.id, question: question.text, answer: value.trim() }];
    setAnswers(next);
    if (current + 1 < (round?.questions.length ?? 0)) {
      const q = round!.questions[current + 1];
      setCurrent(current + 1);
      setValue(q.options.find((o) => o.recommended)?.label ?? "");
    } else {
      ask(next);
    }
  };

  const generate = async (decisions: string) => {
    setPhase("writing");
    setLog([]);
    try {
      const prd = await invoke<Prd>("ralph_generate_prd", {
        workspace: workspace.name,
        repo,
        brief,
        decisions,
      });
      onCreated(prd);
      toast.success("PRD ready", { description: `${prd.userStories.length} stories for ${repo}. Review them, then start Ralph.` });
      onClose();
    } catch (e) {
      onError(String(e));
      setPhase(summary ? "summary" : "brief");
    }
  };

  const busy = phase === "writing" || loading;

  return (
    <div className="modal-overlay" onMouseDown={busy ? undefined : onClose}>
      <div className="modal modal-grill" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>New PRD · {repo}</h3>
        </div>
        <div className="modal-body grill-body">
          {phase === "brief" && (
            <div className="grill-step">
              <p className="grill-summary-intro">
                Describe the feature. The agent can interview you first to settle scope, then writes
                <code> tasks/prd-{workspace.name}.md</code> and <code>scripts/ralph/prd.json</code> with small, ordered
                user stories.
              </p>
              <textarea
                className="grill-answer ralph-brief"
                autoFocus
                value={brief}
                placeholder="What should be built, for whom, and why?"
                onChange={(e) => setBrief(e.target.value)}
              />
              <div className="grill-step-actions">
                <button className="secondary" onClick={onClose}>
                  Cancel
                </button>
                <button className="secondary" disabled={!brief.trim() || loading} onClick={() => generate("")}>
                  Write PRD now
                </button>
                <button disabled={!brief.trim() || loading} onClick={() => ask([])}>
                  {loading ? "Thinking…" : "Interview me first"}
                </button>
              </div>
            </div>
          )}

          {phase === "interview" &&
            (loading ? (
              <div className="grill-loading">
                <span className="spinner" /> Thinking of the next questions…
              </div>
            ) : (
              question && (
                <div className="grill-step">
                  <div className="grill-progress">
                    Round {roundsDone.current} · question {current + 1} of {round?.questions.length}
                  </div>
                  <p className="grill-question">{question.text}</p>
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
                    <p className="grill-opt-desc">
                      {question.options.find((o) => o.label === value)?.description ??
                        question.options.find((o) => o.recommended)?.description}
                    </p>
                  </div>
                  <textarea
                    className="grill-answer"
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                        e.preventDefault();
                        submit();
                      }
                    }}
                  />
                  <div className="grill-step-actions">
                    <button className="secondary danger-outline" onClick={onClose}>
                      Cancel
                    </button>
                    <button className="secondary" disabled={answers.length === 0} onClick={() => ask(answers, true)}>
                      I'm ready
                    </button>
                    <button disabled={!value.trim()} onClick={submit}>
                      {current + 1 < (round?.questions.length ?? 0) ? "Next question" : "Submit round"}
                    </button>
                  </div>
                </div>
              )
            ))}

          {phase === "summary" && (
            <div className="grill-summary">
              <p className="grill-summary-intro">Decisions (editable). Confirm to write the PRD.</p>
              <textarea className="grill-answer ralph-brief" value={summary} onChange={(e) => setSummary(e.target.value)} />
              <div className="grill-step-actions">
                <button className="secondary danger-outline" onClick={onClose}>
                  Cancel
                </button>
                <button onClick={() => generate(summary)}>Write PRD</button>
              </div>
            </div>
          )}

          {phase === "writing" && (
            <>
              <div className="grill-loading">
                <span className="spinner" /> Writing the PRD and splitting it into stories…
                <button
                  className="btn-mini danger-outline"
                  onClick={() => invoke("ralph_cancel_generate", { workspace: workspace.name, repo })}
                >
                  Cancel
                </button>
              </div>
              {log.length > 0 && (
                <div className="plan-log" ref={logRef}>
                  {log.map((l, i) => (
                    <div className="plan-log-line" key={i}>
                      {l}
                    </div>
                  ))}
                </div>
              )}
            </>
          )}
        </div>
      </div>
    </div>
  );
}
