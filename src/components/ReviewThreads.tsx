import { useEffect, useRef, useState } from "react";
import { Draft, ReviewComment, ReviewThread, timeAgo } from "../types/review";
import { SafeMarkdown } from "./SafeMarkdown";
import { CheckIcon, ChevronRightIcon, PencilIcon, TrashIcon } from "./Icons";

function Avatar({ url, name }: { url?: string; name: string }) {
  return url ? (
    <img className="rv-avatar" src={url} alt="" />
  ) : (
    <span className="rv-avatar rv-avatar-fallback">{name.slice(0, 1).toUpperCase()}</span>
  );
}

/** Textarea with Ctrl/Cmd+Enter for the primary action and Esc to cancel. */
function Editor({
  initial = "",
  placeholder,
  autoFocus = true,
  busy,
  primary,
  secondary,
  onCancel,
}: {
  initial?: string;
  placeholder: string;
  autoFocus?: boolean;
  busy?: boolean;
  primary: { label: string; onClick: (text: string) => void };
  secondary?: { label: string; onClick: (text: string) => void };
  onCancel?: () => void;
}) {
  const [text, setText] = useState(initial);
  const ref = useRef<HTMLTextAreaElement>(null);
  const empty = !text.trim();
  const latest = useRef({ text, empty, primary, onCancel });
  latest.current = { text, empty, primary, onCancel };
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    // After Monaco finishes handling the click that opened us (it refocuses
    // its own textarea on mouseup).
    const focusTimer = autoFocus ? window.setTimeout(() => el.focus(), 30) : 0;
    // Native listener: inside the diff, key events stop at the zone before
    // React's root handler would see them.
    const onKey = (e: KeyboardEvent) => {
      const l = latest.current;
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && !l.empty) {
        e.preventDefault();
        l.primary.onClick(l.text);
      } else if (e.key === "Escape" && l.onCancel) {
        e.preventDefault();
        l.onCancel();
      }
    };
    el.addEventListener("keydown", onKey);
    return () => {
      window.clearTimeout(focusTimer);
      el.removeEventListener("keydown", onKey);
    };
  }, [autoFocus]);
  return (
    <div className="rv-editor">
      <textarea
        ref={ref}
        value={text}
        rows={3}
        placeholder={placeholder}
        disabled={busy}
        onChange={(e) => setText(e.target.value)}
      />
      <div className="rv-editor-actions">
        <span className="rv-editor-hint">Markdown · Ctrl+Enter</span>
        {onCancel && (
          <button className="btn-mini secondary" disabled={busy} onClick={onCancel}>
            Cancel
          </button>
        )}
        {secondary && (
          <button className="btn-mini secondary" disabled={busy || empty} onClick={() => secondary.onClick(text)}>
            {secondary.label}
          </button>
        )}
        <button className="btn-mini" disabled={busy || empty} onClick={() => primary.onClick(text)}>
          {busy ? "Sending…" : primary.label}
        </button>
      </div>
    </div>
  );
}

function CommentView({
  c,
  onEdit,
  onDelete,
}: {
  c: ReviewComment;
  onEdit: (body: string) => Promise<void>;
  onDelete: () => Promise<void>;
}) {
  const [editing, setEditing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  return (
    <div className="rv-comment">
      <Avatar url={c.avatarUrl} name={c.author} />
      <div className="rv-comment-main">
        <div className="rv-comment-head">
          <strong>{c.author}</strong>
          <span className="rv-comment-time" title={new Date(c.createdAt).toLocaleString()}>
            {timeAgo(c.createdAt)}
          </span>
          {c.state === "PENDING" && <span className="rv-pill rv-pill-pending">Pending</span>}
          {c.isMine && !editing && (
            <span className="rv-comment-tools">
              <button className="icon-button" aria-label="Edit comment" onClick={() => setEditing(true)}>
                <PencilIcon size={12} />
              </button>
              {confirmDelete ? (
                <>
                  <button
                    className="btn-mini danger-outline"
                    disabled={busy}
                    onClick={async () => {
                      setBusy(true);
                      await onDelete().finally(() => setBusy(false));
                    }}
                  >
                    Delete
                  </button>
                  <button className="btn-mini secondary" onClick={() => setConfirmDelete(false)}>
                    Keep
                  </button>
                </>
              ) : (
                <button className="icon-button icon-button-danger" aria-label="Delete comment" onClick={() => setConfirmDelete(true)}>
                  <TrashIcon size={12} />
                </button>
              )}
            </span>
          )}
        </div>
        {editing ? (
          <Editor
            initial={c.body}
            placeholder="Edit comment"
            busy={busy}
            primary={{
              label: "Save",
              onClick: async (text) => {
                setBusy(true);
                await onEdit(text)
                  .then(() => setEditing(false))
                  .finally(() => setBusy(false));
              },
            }}
            onCancel={() => setEditing(false)}
          />
        ) : (
          <SafeMarkdown content={c.body} />
        )}
      </div>
    </div>
  );
}

export interface ThreadActions {
  reply: (thread: ReviewThread, body: string) => Promise<void>;
  resolve: (thread: ReviewThread, resolved: boolean) => Promise<void>;
  edit: (comment: ReviewComment, body: string) => Promise<void>;
  remove: (comment: ReviewComment) => Promise<void>;
}

/** An existing GitHub review thread. Resolved threads start collapsed. */
export function ThreadCard({ thread, actions }: { thread: ReviewThread; actions: ThreadActions }) {
  const [open, setOpen] = useState(!thread.isResolved);
  const [replying, setReplying] = useState(false);
  const [busy, setBusy] = useState(false);
  const first = thread.comments[0];
  const range = thread.startLine && thread.line && thread.startLine !== thread.line ? `Lines ${thread.startLine}–${thread.line}` : null;

  return (
    <div className={`rv-thread ${thread.isResolved ? "resolved" : ""}`}>
      <button className="rv-thread-head" onClick={() => setOpen((v) => !v)}>
        <ChevronRightIcon size={12} className={open ? "disclosure-chevron open" : "disclosure-chevron"} />
        {!open && first && <Avatar url={first.avatarUrl} name={first.author} />}
        <span className="rv-thread-title">
          {open ? (range ?? "Conversation") : first ? `${first.author}: ${first.body.split("\n")[0]}` : "Thread"}
        </span>
        {thread.comments.length > 1 && <span className="rv-thread-count">{thread.comments.length}</span>}
        {thread.isOutdated && <span className="rv-pill">Outdated</span>}
        {thread.isResolved && (
          <span className="rv-pill rv-pill-ok">
            <CheckIcon size={10} /> Resolved
          </span>
        )}
      </button>
      {open && (
        <>
          <div className="rv-thread-body">
            {thread.comments.map((c) => (
              <CommentView key={c.id} c={c} onEdit={(body) => actions.edit(c, body)} onDelete={() => actions.remove(c)} />
            ))}
          </div>
          <div className="rv-thread-foot">
            {replying ? (
              <Editor
                placeholder="Reply…"
                busy={busy}
                primary={{
                  label: "Reply",
                  onClick: async (text) => {
                    setBusy(true);
                    await actions
                      .reply(thread, text)
                      .then(() => setReplying(false))
                      .finally(() => setBusy(false));
                  },
                }}
                onCancel={() => setReplying(false)}
              />
            ) : (
              <>
                <button className="rv-reply-fake" onClick={() => setReplying(true)}>
                  Reply…
                </button>
                <button
                  className="btn-mini secondary"
                  disabled={busy}
                  onClick={async () => {
                    setBusy(true);
                    await actions.resolve(thread, !thread.isResolved).finally(() => setBusy(false));
                  }}
                >
                  {thread.isResolved ? "Unresolve" : "Resolve"}
                </button>
              </>
            )}
          </div>
        </>
      )}
    </div>
  );
}

/** A pending comment of the review in progress. */
export function DraftCard({
  draft,
  viewer,
  onChange,
  onDelete,
}: {
  draft: Draft;
  viewer: string;
  onChange: (body: string) => void;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  const range = draft.startLine && draft.startLine !== draft.line ? `Lines ${draft.startLine}–${draft.line}` : null;
  return (
    <div className="rv-thread rv-thread-draft">
      <div className="rv-comment">
        <Avatar name={viewer || "you"} />
        <div className="rv-comment-main">
          <div className="rv-comment-head">
            <strong>{viewer || "You"}</strong>
            <span className="rv-pill rv-pill-pending">Pending</span>
            {range && <span className="rv-comment-time">{range}</span>}
            {!editing && (
              <span className="rv-comment-tools">
                <button className="icon-button" aria-label="Edit pending comment" onClick={() => setEditing(true)}>
                  <PencilIcon size={12} />
                </button>
                <button className="icon-button icon-button-danger" aria-label="Delete pending comment" onClick={onDelete}>
                  <TrashIcon size={12} />
                </button>
              </span>
            )}
          </div>
          {editing ? (
            <Editor
              initial={draft.body}
              placeholder="Edit comment"
              primary={{
                label: "Save",
                onClick: (text) => {
                  onChange(text);
                  setEditing(false);
                },
              }}
              onCancel={() => setEditing(false)}
            />
          ) : (
            <SafeMarkdown content={draft.body} />
          )}
        </div>
      </div>
    </div>
  );
}

/** New comment on a line or range: add to the pending review or post now. */
export function Composer({
  label,
  reviewInProgress,
  onAddToReview,
  onCommentNow,
  onCancel,
}: {
  label: string;
  reviewInProgress: boolean;
  onAddToReview: (body: string) => void;
  onCommentNow: (body: string) => Promise<void>;
  onCancel: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const addToReview = { label: reviewInProgress ? "Add review comment" : "Start a review", onClick: onAddToReview };
  const now = {
    label: "Comment now",
    onClick: async (text: string) => {
      setBusy(true);
      await onCommentNow(text).finally(() => setBusy(false));
    },
  };
  return (
    <div className="rv-thread rv-composer">
      <div className="rv-composer-label">{label}</div>
      <Editor
        placeholder="Leave a comment"
        busy={busy}
        primary={reviewInProgress ? addToReview : now}
        secondary={reviewInProgress ? now : addToReview}
        onCancel={onCancel}
      />
    </div>
  );
}
