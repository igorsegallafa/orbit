import { useEffect, useRef, useState } from "react";
import { ReviewEvent } from "../types/review";
import { CheckBox } from "./CheckBox";

const OPTIONS: { event: ReviewEvent; title: string; hint: string }[] = [
  { event: "COMMENT", title: "Comment", hint: "General feedback without explicit approval." },
  { event: "APPROVE", title: "Approve", hint: "Give your approval to merge these changes." },
  { event: "REQUEST_CHANGES", title: "Request changes", hint: "Feedback that must be addressed before merging." },
];

/** Review submission popover: summary, verdict and the pending comments. */
export function SubmitReview({
  summaryKey,
  pending,
  ownPr,
  busy,
  onSubmit,
  onDiscard,
  onClose,
}: {
  /** localStorage key keeping the summary draft of this PR. */
  summaryKey: string;
  pending: number;
  ownPr: boolean;
  busy: boolean;
  onSubmit: (event: ReviewEvent, body: string) => void;
  onDiscard: () => void;
  onClose: () => void;
}) {
  const [event, setEvent] = useState<ReviewEvent>("COMMENT");
  const [body, setBody] = useState(() => {
    try {
      return localStorage.getItem(summaryKey) ?? "";
    } catch {
      return "";
    }
  });
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    try {
      localStorage.setItem(summaryKey, body);
    } catch {
      // storage unavailable: the summary still lives in state
    }
  }, [body, summaryKey]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as HTMLElement;
      // The Review button toggles the popover itself.
      if (t.closest?.(".rv-review-btn")) return;
      if (!busy && ref.current && !ref.current.contains(t)) onClose();
    };
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && !busy && onClose();
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [busy, onClose]);

  const needsText = event !== "APPROVE" && !body.trim() && pending === 0;

  return (
    <div className="rv-submit" ref={ref}>
      <div className="rv-submit-title">Finish your review</div>
      <textarea
        autoFocus
        rows={4}
        value={body}
        placeholder="Leave a summary (optional for approvals)"
        onChange={(e) => setBody(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !needsText && !busy) {
            e.preventDefault();
            onSubmit(event, body);
          }
        }}
      />
      <div className="rv-submit-options">
        {OPTIONS.map((o) => {
          const disabled = ownPr && o.event !== "COMMENT";
          return (
            <div
              key={o.event}
              className={`choice ${event === o.event ? "choice-on" : ""} ${disabled ? "choice-disabled" : ""}`}
              onClick={() => !disabled && setEvent(o.event)}
              title={disabled ? "You can't approve or request changes on your own pull request" : undefined}
            >
              <CheckBox label={o.title} checked={event === o.event} disabled={disabled} onChange={() => !disabled && setEvent(o.event)} />
              <span>
                <strong>{o.title}</strong>
                <small>{o.hint}</small>
              </span>
            </div>
          );
        })}
      </div>
      {ownPr && <div className="rv-submit-note">Only Comment is available on your own PR.</div>}
      <div className="rv-submit-foot">
        {pending > 0 ? (
          <button className="btn-link rv-discard" disabled={busy} onClick={onDiscard}>
            Discard {pending} pending
          </button>
        ) : (
          <span />
        )}
        <span className="rv-submit-count">
          {pending} pending comment{pending === 1 ? "" : "s"}
        </span>
        <button disabled={busy || needsText} onClick={() => onSubmit(event, body)}>
          {busy ? "Submitting…" : "Submit review"}
        </button>
      </div>
    </div>
  );
}
