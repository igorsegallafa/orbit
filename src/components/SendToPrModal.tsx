import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrDetail, PrFileDiff } from "../types/config";
import { ReviewData, hunkOf } from "../types/review";
import { CheckBox } from "./CheckBox";
import { toast } from "./Toast";
import { Note, anchorNote } from "../lib/notes";

type PrRef = { repo: string; number: number; url: string };

/** A note checked against the PR: where it lands on the pushed code, or why it can't go. */
interface Candidate {
  note: Note;
  pr: PrRef;
  ownerRepo: string;
  headSha: string;
  start: number;
  end: number;
  problem: string | null;
}

interface Props {
  workspace: string;
  prRefs: PrRef[];
  notes: Note[];
  /** Notes posted to GitHub (to drop from REVIEW.md when asked). */
  onSent: (ids: string[]) => void;
  onError: (msg: string) => void;
  onClose: () => void;
}

/**
 * Posts open notes as one review per PR. GitHub only takes comments on lines
 * of the PR's diff at its pushed head, so each note is found again in the
 * pushed file (by its code); notes on unpushed code or outside the diff are
 * listed with the reason and left out.
 */
export function SendToPrModal({ workspace, prRefs, notes, onSent, onError, onClose }: Props) {
  const [cands, setCands] = useState<Candidate[] | null>(null);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [removeSent, setRemoveSent] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let stale = false;
    (async () => {
      const out: Candidate[] = [];
      for (const pr of prRefs) {
        const mine = notes.filter((n) => n.repo === pr.repo);
        try {
          const ownerRepo = await invoke<string>("ws_owner_repo", { workspace, repo: pr.repo });
          const [detail, review] = await Promise.all([
            invoke<PrDetail>("pr_detail", { ownerRepo, number: pr.number }),
            invoke<ReviewData>("pr_review_data", { ownerRepo, number: pr.number, withLines: true }),
          ]);
          const heads = new Map<string, string | null>();
          for (const n of mine) {
            if (!heads.has(n.path)) {
              const d = await invoke<PrFileDiff>("pr_file_diff", { ownerRepo, headSha: detail.headSha, baseSha: detail.baseSha, path: n.path }).catch(() => null);
              heads.set(n.path, d ? d.modified : null);
            }
            const head = heads.get(n.path) ?? null;
            const at = anchorNote(n, head);
            const hunk = hunkOf(review.commentable[n.path], "RIGHT", at.end);
            const problem =
              head === null || !detail.files.some((f) => f.path === n.path)
                ? "file isn't part of the PR"
                : at.lost
                  ? "code isn't pushed yet"
                  : !hunk || at.start < hunk[0]
                    ? "outside the PR's changed lines"
                    : null;
            out.push({ note: n, pr, ownerRepo, headSha: detail.headSha, start: at.start, end: at.end, problem });
          }
        } catch (e) {
          for (const n of mine) out.push({ note: n, pr, ownerRepo: "", headSha: "", start: n.start, end: n.end, problem: String(e) });
        }
      }
      if (stale) return;
      setCands(out);
      setPicked(new Set(out.filter((c) => !c.problem).map((c) => c.note.id)));
    })();
    return () => {
      stale = true;
    };
  }, [workspace, prRefs, notes]);

  const send = async () => {
    if (!cands) return;
    setBusy(true);
    const sent: string[] = [];
    try {
      for (const pr of prRefs) {
        const mine = cands.filter((c) => c.pr.number === pr.number && c.pr.repo === pr.repo && !c.problem && picked.has(c.note.id));
        if (!mine.length) continue;
        await invoke("pr_submit_review", {
          ownerRepo: mine[0].ownerRepo,
          number: pr.number,
          commitId: mine[0].headSha,
          event: "COMMENT",
          body: "",
          comments: mine.map((c) => ({
            path: c.note.path,
            side: "RIGHT",
            line: c.end,
            startLine: c.start < c.end ? c.start : null,
            body: c.note.body,
          })),
        });
        sent.push(...mine.map((c) => c.note.id));
        toast.success(`Review posted on ${pr.repo} #${pr.number}`, {
          description: `${mine.length} comment${mine.length === 1 ? "" : "s"}`,
          action: { label: "Open on GitHub", onClick: () => openUrl(pr.url).catch(() => null) },
        });
      }
      if (removeSent && sent.length) onSent(sent);
      onClose();
    } catch (e) {
      onError(String(e));
      if (removeSent && sent.length) onSent(sent);
    } finally {
      setBusy(false);
    }
  };

  const ready = cands?.filter((c) => !c.problem) ?? [];
  const count = ready.filter((c) => picked.has(c.note.id)).length;

  return (
    <div className="modal-overlay" onMouseDown={busy ? undefined : onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>Send notes to the pull request</h3>
          <p className="muted">Posted as a review comment on each note's lines. Only code that is pushed and part of the PR's diff can be commented on.</p>
          {cands === null ? (
            <div className="table-loading">
              <span className="spinner" /> Checking notes against the PR…
            </div>
          ) : (
            <div className="stp-list">
              {cands.map((c) => (
                <div key={c.note.id} className={`stp-row ${c.problem ? "stp-off" : ""}`}>
                  <CheckBox
                    checked={!c.problem && picked.has(c.note.id)}
                    disabled={!!c.problem}
                    label="Send this note"
                    onChange={(on) =>
                      setPicked((s) => {
                        const n = new Set(s);
                        if (on) n.add(c.note.id);
                        else n.delete(c.note.id);
                        return n;
                      })
                    }
                  />
                  <div className="stp-main">
                    <div className="stp-loc">
                      {c.pr.repo} #{c.pr.number} · {c.note.path}:{c.start === c.end ? c.start : `${c.start}–${c.end}`}
                    </div>
                    <div className="stp-body">{c.note.body.split("\n")[0]}</div>
                    {c.problem && <div className="stp-why">{c.problem}</div>}
                  </div>
                </div>
              ))}
            </div>
          )}
          <label className="stp-remove">
            <CheckBox checked={removeSent} onChange={setRemoveSent} label="Remove sent notes from REVIEW.md" />
            Remove sent notes from REVIEW.md
          </label>
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="button" onClick={send} disabled={busy || count === 0}>
            {busy ? (
              <>
                <span className="spinner" /> Posting…
              </>
            ) : (
              `Post ${count} comment${count === 1 ? "" : "s"}`
            )}
          </button>
        </div>
      </div>
    </div>
  );
}
