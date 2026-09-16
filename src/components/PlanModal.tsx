import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { CardDetail, CardRef, Workspace } from "../types/config";

interface Props {
  workspace: Workspace;
  card: CardRef;
  onOpenPlan: () => void;
  /** Opens the native grill-me interview stepper (claude/opencode). */
  onStartInterview: () => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

interface PlanEvent {
  status: "line" | "done" | "error";
  line: string;
}

/**
 * Plan generator modal: shows the linked card context and runs the configured
 * agent (Settings → AI) to produce PLAN.md in the workspace root, streaming
 * the agent's output live. Closes itself and opens the plan on success.
 */
export function PlanModal({ workspace, card, onOpenPlan, onStartInterview, onClose, onError }: Props) {
  const [detail, setDetail] = useState<CardDetail | null>(null);
  const [running, setRunning] = useState(false);
  const [log, setLog] = useState<string[]>([]);
  const logRef = useRef<HTMLDivElement>(null);
  // Refs the stream listener uses so it can act without stale closures.
  const onOpenPlanRef = useRef(onOpenPlan);
  onOpenPlanRef.current = onOpenPlan;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  // Load card detail (description) + AI settings for context
  useEffect(() => {
    let cancelled = false;
    invoke<CardDetail>("integration_fetch_card", { kind: card.kind, id: card.id })
      .then((d) => !cancelled && setDetail(d))
      .catch((e) => !cancelled && onError(String(e)));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [card.id]);

  // Stream agent progress; auto-close + open the plan when it finishes.
  useEffect(() => {
    const off = listen<PlanEvent>("plan-progress", (event) => {
      const p = event.payload;
      if (p.status === "done") {
        setRunning(false);
        // Auto: close modal and open the freshly generated plan.
        onCloseRef.current();
        onOpenPlanRef.current();
      } else if (p.status === "error") {
        setLog((l) => [...l, p.line]);
        setRunning(false);
      } else {
        setLog((l) => [...l.slice(-400), p.line]);
      }
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  // Autoscroll the log
  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [log]);

  // Stepper interview — works for every agent (headless rounds).
  const startInterview = () => {
    onStartInterview();
    onCloseRef.current();
  };

  const generate = async () => {
    setRunning(true);
    setLog([]);
    try {
      await invoke("generate_plan", { name: workspace.name, card });
    } catch (e) {
      onError(String(e));
      setRunning(false);
    }
  };

  const cancel = async () => {
    try {
      await invoke("cancel_plan");
      setLog((l) => [...l, "[orbit] cancelled"]);
    } catch (e) {
      onError(String(e));
    }
    setRunning(false);
  };

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-plan" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>
            Plan · {workspace.name}
            {detail?.state ? ` · ${detail.state}` : ""}
          </h3>
        </div>

        <div className="modal-body">
          <div className="plan-card-context">
            <span className="tag tag-info">{card.id}</span>
            <span className="plan-card-title">{detail?.title ?? card.title}</span>
            {card.url && (
              <a className="plan-card-link" href={card.url} onClick={(e) => e.preventDefault()}>
                open card ↗
              </a>
            )}
          </div>
          {detail && detail.description && (
            <p className="plan-card-desc">
              {detail.description.slice(0, 280)}
              {detail.description.length > 280 ? "…" : ""}
            </p>
          )}

          <div className="plan-log" ref={logRef}>
            {log.length === 0 && !running && (
              <span className="plan-log-hint">
                Generate a PLAN.md from this card using the agent configured in Settings → AI.
              </span>
            )}
            {running && log.length === 0 && (
              <span className="plan-log-hint">
                <span className="spinner" /> starting agent…
              </span>
            )}
            {log.map((line, i) => (
              <div className="plan-log-line" key={i}>
                {line}
              </div>
            ))}
          </div>
        </div>

        <div className="modal-footer">
          {running ? (
            <button className="secondary danger-outline" onClick={cancel}>
              Cancel
            </button>
          ) : (
            <button className="secondary" onClick={onClose}>
              Close
            </button>
          )}
          {!running && !log.length && (
            <>
              <button className="secondary" onClick={startInterview} disabled={!detail}
                title="Interview me first: the agent asks questions in rounds to confirm behaviors, then writes the plan">
                Interview first
              </button>
              <button onClick={generate} disabled={running || !detail}>
                Generate plan
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}