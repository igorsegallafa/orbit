import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useFsChanges } from "./useFsChanges";

/** A review note: a comment on local code, kept as a task in REVIEW.md. */
export interface Note {
  id: string;
  done: boolean;
  repo: string;
  path: string;
  start: number;
  end: number;
  body: string;
  snippet: string;
}

export interface NotesFile {
  /** Absolute path of REVIEW.md. */
  file: string;
  notes: Note[];
}

/** REVIEW.md of a workspace, kept fresh when anyone (an agent) edits it. */
export function useNotes(workspace: string, onError: (msg: string) => void) {
  const [data, setData] = useState<NotesFile | null>(null);

  const reload = useCallback(() => {
    invoke<NotesFile>("notes_list", { workspace })
      .then(setData)
      .catch((e) => onError(String(e)));
  }, [workspace, onError]);

  useEffect(() => {
    setData(null);
    reload();
  }, [reload]);

  useFsChanges(workspace, null, (c) => {
    if (c.paths.includes("REVIEW.md") || c.paths.length === 0) reload();
  });

  const run = useCallback(
    async (cmd: string, args: Record<string, unknown>) => {
      try {
        setData(await invoke<NotesFile>(cmd, { workspace, ...args }));
        return true;
      } catch (e) {
        onError(String(e));
        reload();
        return false;
      }
    },
    [workspace, onError, reload],
  );

  return {
    data,
    notes: data?.notes ?? [],
    add: (n: Omit<Note, "id" | "done">) => run("notes_add", { ...n }),
    setDone: (id: string, done: boolean) => run("notes_update", { id, done }),
    edit: (id: string, body: string) => run("notes_update", { id, body }),
    remove: (ids: string[]) => run("notes_delete", { ids }),
  };
}

/** Where a note's code is now: its snippet, found again after edits shifted
 *  it. `lost` when the code isn't there any more (the note stays at its old
 *  line, flagged). */
export function anchorNote(note: Note, text: string | null): { start: number; end: number; moved: boolean; lost: boolean } {
  const at = { start: note.start, end: note.end, moved: false, lost: false };
  if (text === null || !note.snippet.trim()) return at;
  const lines = text.split("\n");
  const want = note.snippet.split("\n");
  const fits = (from: number) => want.every((w, k) => (lines[from + k] ?? "").trimEnd() === w.trimEnd());
  if (fits(note.start - 1)) return at;
  let best = -1;
  for (let i = 0; i + want.length <= lines.length; i++) {
    if (fits(i) && (best < 0 || Math.abs(i - (note.start - 1)) < Math.abs(best - (note.start - 1)))) best = i;
  }
  if (best < 0) {
    const last = Math.max(1, lines.length);
    return { start: Math.min(note.start, last), end: Math.min(note.end, last), moved: false, lost: true };
  }
  return { start: best + 1, end: best + want.length, moved: true, lost: false };
}

/** Prompt that has an agent work through the open notes. */
export function notesPrompt(file: string): string {
  return `Read the review notes in ${file} and address every open item ("- [ ]"): each one points at a file and lines (relative to that file's folder), says what to change, and shows the code it was left on. Tick each item ("- [x]") in that file as you finish it. Commit nothing unless asked.`;
}
