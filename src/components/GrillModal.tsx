import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CardRef, GrillOption, Workspace } from "../types/config";
import { Skeleton } from "./Skeleton";

interface GrillQuestion {
  id: string;
  text: string;
  options: GrillOption[];
}

interface GrillRound {
  done: boolean;
  questions: GrillQuestion[];
  summary: string;
}

interface Answer {
  questionId: string;
  question: string;
  answer: string;
}

function answersToSummary(list: Answer[]): string {
  if (list.length === 0) return "- No answers were given.";
  return list
    .map((a) => `- ${a.question}\n  → ${a.answer}`)
    .join("\n");
}

interface Props {
  workspace: Workspace;
  card: CardRef;
  onPlanReady: () => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

/**
 * Native grill-me interview (UI stepper): each round the agent returns
 * questions as structured JSON; the user answers in a step-by-step form.
 * When the agent signals done, a summary is confirmed and the plan is
 * generated headlessly with the decisions baked in.
 */
export function GrillModal({ workspace, card, onPlanReady, onClose, onError }: Props) {
  const [round, setRound] = useState<GrillRound | null>(null);
  const [answers, setAnswers] = useState<Answer[]>([]);
  const [current, setCurrent] = useState(0); // index within the round's questions
  const [value, setValue] = useState("");
  const [roundsDone, setRoundsDone] = useState(0);
  // Authoritative round count (state can be stale inside quick successive
  // submits; the ref is what fetches actually use).
  const roundsDoneRef = useRef(0);
  const [loading, setLoading] = useState(true);
  const [phase, setPhase] = useState<"interview" | "summary" | "writing" | "done">("interview");

  // Build (questionId, answer) pairs the backend expects.
  const pairAnswers = useCallback(
    (list: Answer[]) => list.map((a) => [a.questionId, a.answer] as [string, string]),
    []
  );

  const fetchRound = useCallback(
    async (list: Answer[]) => {
      setLoading(true);
      try {
        const r = await invoke<GrillRound>("grill_step", {
          name: workspace.name,
          card,
          answers: pairAnswers(list),
          roundsDone: roundsDoneRef.current,
          maxRounds: null,
        });
        setRound(r);
        setCurrent(0);
        setValue("");
        if (r.done) {
          setPhase("summary");
        } else {
          roundsDoneRef.current += 1;
          setRoundsDone(roundsDoneRef.current);
        }
      } catch (e) {
        onError(String(e));
      } finally {
        setLoading(false);
      }
    },
    [workspace.name, card, pairAnswers, onError]
  );

  // First round on mount
  useEffect(() => {
    fetchRound([]);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const question = round?.questions[current];

  const finishNow = async () => {
    if (answers.length === 0) {
      onError("Answer at least one question before finishing, or Cancel.");
      return;
    }
    setLoading(true);
    try {
      // One last agent call: summarize the decisions so far, no new questions.
      const r = await invoke<GrillRound>("grill_step", {
        name: workspace.name,
        card,
        answers: pairAnswers(answers),
        roundsDone: roundsDoneRef.current,
        maxRounds: roundsDoneRef.current + 1, // force done on this call
      });
      setRound({
        done: true,
        questions: [],
        summary: r.done && r.summary ? r.summary : answersToSummary(answers),
      });
      setPhase("summary");
    } catch (e) {
      // Agent call failed — still let the user proceed with a raw summary.
      setRound({ done: true, questions: [], summary: answersToSummary(answers) });
      setPhase("summary");
    } finally {
      setLoading(false);
    }
  };

  const submitAnswer = () => {
    if (!question) return;
    const next = [...answers, { questionId: question.id, question: question.text, answer: value.trim() }];
    setAnswers(next);
    if (current + 1 < (round?.questions.length ?? 0)) {
      setCurrent(current + 1);
      setValue("");
    } else {
      // Round finished — fetch the next one with all answers so far.
      fetchRound(next);
    }
  };

  const writePlan = async () => {
    setPhase("writing");
    try {
      await invoke("generate_plan_decisions", {
        name: workspace.name,
        card,
        decisions: round?.summary ?? answers.map((a) => `- ${a.question}\n  → ${a.answer}`).join("\n"),
      });
      setPhase("done");
      onPlanReady();
      onClose();
    } catch (e) {
      onError(String(e));
      setPhase("summary");
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={phase === "writing" ? undefined : onClose}>
      <div className="modal modal-grill" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>
            Interview · {card.id}
            {phase === "interview"
              ? ` · round ${Math.max(1, roundsDone + (loading ? 1 : 0))}${loading ? "…" : ""}`
              : ""}
          </h3>

        </div>

        <div className="modal-body grill-body">
          {phase === "interview" && (
            <>
              {loading ? (
                <div className="grill-loading">
                  <span className="spinner" /> Thinking of the next questions…
                </div>
              ) : question ? (
                <div className="grill-step">
                  <div className="grill-progress">
                    Question {current + 1} of {round?.questions.length}
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
                    placeholder="Type your answer… (you can pick a suggestion above)"
                    onChange={(e) => setValue(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                        e.preventDefault();
                        submitAnswer();
                      }
                    }}
                  />
                  <div className="grill-step-actions">
                    <button className="secondary danger-outline" onClick={onClose}>
                      Cancel
                    </button>
                    <button
                      className="secondary"
                      onClick={finishNow}
                      disabled={loading || answers.length === 0}
                      title="Skip the remaining rounds: the agent summarizes what was answered and you confirm the plan"
                    >
                      I'm ready
                    </button>
                    <button onClick={submitAnswer} disabled={!value.trim()}>
                      {current + 1 < (round?.questions.length ?? 0) ? "Next question" : "Submit round"}
                    </button>
                  </div>
                </div>
              ) : (
                <div className="grill-loading">
                  <Skeleton w="80%" h={14} />
                  <Skeleton w="60%" h={14} />
                </div>
              )}
            </>
          )}

          {phase === "summary" && (
            <div className="grill-summary">
              <p className="grill-summary-intro">
                Interview complete — here's what got decided. Confirm to write the plan.
              </p>
              <pre className="grill-summary-text">{round?.summary}</pre>
              <div className="grill-step-actions">
                <button className="secondary danger-outline" onClick={onClose}>
                  Cancel
                </button>
                <button onClick={writePlan}>Write plan</button>
              </div>
            </div>
          )}

          {phase === "writing" && (
            <div className="grill-loading">
              <span className="spinner" /> Writing PLAN.md with your decisions…
            </div>
          )}
        </div>

        {answers.length > 0 && phase === "interview" && (
          <div className="grill-history">
            {answers.slice(-4).map((a, i) => (
              <div className="grill-history-item" key={i}>
                <span className="grill-history-q">{a.question}</span>
                <span className="grill-history-a">{a.answer}</span>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}