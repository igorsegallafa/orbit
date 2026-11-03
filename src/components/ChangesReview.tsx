import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { DiffEditor, type BeforeMount } from "@monaco-editor/react";
import type { editor as Monaco } from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";
import { GitChange, GitFileDiff, Workspace } from "../types/config";
import { Side } from "../types/review";
import { RangeMark, ZoneItem, useReviewZones } from "./useReviewZones";
import { Editor as CommentEditor } from "./ReviewThreads";
import { SafeMarkdown } from "./SafeMarkdown";
import { CheckBox } from "./CheckBox";
import { ContextMenu } from "./ContextMenu";
import { usePaneWidth } from "./PaneResizer";
import { STATUS_LABEL } from "./GitPanel";
import { ChevronIcon, PencilIcon, TrashIcon } from "./Icons";
import { SendToPrModal } from "./SendToPrModal";
import { toast } from "./Toast";
import { defineMonacoThemes, useMonacoTheme } from "../lib/theme";
import { useFsChanges } from "../lib/useFsChanges";
import { Note, anchorNote, notesPrompt, useNotes } from "../lib/notes";

const beforeMount: BeforeMount = (monaco) => defineMonacoThemes(monaco);

function langOf(path: string): string | undefined {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "ts" || ext === "tsx") return "typescript";
  if (ext === "js" || ext === "jsx") return "javascript";
  if (ext === "rs") return "rust";
  if (ext === "go") return "go";
  if (ext === "py") return "python";
  if (ext === "md") return "markdown";
  if (ext === "json" || ext === "yaml" || ext === "yml" || ext === "toml") return ext;
  return undefined;
}

function useLocal<T>(key: string, initial: T): [T, (v: T | ((p: T) => T)) => void] {
  const [v, setV] = useState<T>(() => {
    try {
      const raw = localStorage.getItem(key);
      return raw ? (JSON.parse(raw) as T) : initial;
    } catch {
      return initial;
    }
  });
  const set = useCallback(
    (next: T | ((p: T) => T)) =>
      setV((prev) => {
        const val = typeof next === "function" ? (next as (p: T) => T)(prev) : next;
        try {
          localStorage.setItem(key, JSON.stringify(val));
        } catch {
          // only this viewer's convenience is lost
        }
        return val;
      }),
    [key],
  );
  return [v, set];
}

const fileKey = (repo: string, path: string) => `${repo}/${path}`;

export interface AgentSession {
  id: string;
  name: string;
}

interface Props {
  workspace: Workspace;
  /** File to scroll to (set again with a new `at` to scroll again). */
  focus?: { repo: string; path: string; at: number };
  /** Agent sessions running in this workspace, to hand the notes to. */
  sessions: AgentSession[];
  onSendToSession: (id: string, prompt: string) => void;
  onNewSession: (prompt: string) => void;
  onOpenFile: (repo: string, path: string) => void;
  onError: (msg: string) => void;
}

/**
 * Review of a workspace's local changes, GitHub "Files changed" style: every
 * changed file of every repo in one scrolling list, refreshed as files
 * change on disk. Comments on lines or ranges become notes in REVIEW.md,
 * a checklist an agent works through; notes can also go to the PR.
 */
export function ChangesReview({ workspace, focus, sessions, onSendToSession, onNewSession, onOpenFile, onError }: Props) {
  const ws = workspace.name;
  const [changes, setChanges] = useState<Record<string, GitChange[] | null>>({});
  const [revs, setRevs] = useState<Record<string, number>>({});
  const [viewed, setViewed] = useLocal<string[]>(`orbit.changes.viewed.${ws}`, []);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [showDone, setShowDone] = useLocal<boolean>("orbit.changes.showDone", false);
  const [composer, setComposer] = useState<{ repo: string; path: string; start: number; end: number; snippet: string } | null>(null);
  const [fixMenu, setFixMenu] = useState<{ x: number; y: number } | null>(null);
  const [prOpen, setPrOpen] = useState(false);
  const notes = useNotes(ws, onError);
  const side = usePaneWidth("orbit.changes-side", 280, 200, 560);
  const scrollRef = useRef<HTMLDivElement>(null);
  const blocks = useRef(new Map<string, { el: HTMLDivElement | null; reveal: (line: number) => void }>());

  const loadRepo = useCallback(
    (repo: string) =>
      invoke<GitChange[]>("git_changes", { workspace: ws, repo })
        .then((c) => setChanges((all) => ({ ...all, [repo]: c })))
        .catch((e) => {
          setChanges((all) => ({ ...all, [repo]: all[repo] ?? [] }));
          onError(String(e));
        }),
    [ws, onError],
  );

  useEffect(() => {
    setChanges({});
    workspace.repos.forEach(loadRepo);
  }, [workspace.repos, loadRepo]);

  // Follow the disk: the touched repo's list, and the touched files' diffs.
  useFsChanges(ws, null, (c) => {
    if (!c.repo) {
      if (c.paths.length === 0) workspace.repos.forEach(loadRepo);
      return;
    }
    loadRepo(c.repo);
    setRevs((r) => {
      const next = { ...r };
      const all = c.git || c.paths.length === 0;
      for (const ch of changes[c.repo] ?? []) {
        if (all || c.paths.includes(ch.path)) next[fileKey(c.repo, ch.path)] = (next[fileKey(c.repo, ch.path)] ?? 0) + 1;
      }
      for (const p of c.paths) next[fileKey(c.repo, p)] = (next[fileKey(c.repo, p)] ?? 0) + 1;
      return next;
    });
  });

  const files = useMemo(
    () => workspace.repos.flatMap((repo) => (changes[repo] ?? []).map((c) => ({ repo, change: c, key: fileKey(repo, c.path) }))),
    [workspace.repos, changes],
  );
  const loading = workspace.repos.some((r) => changes[r] === undefined || changes[r] === null);
  const totals = files.reduce((t, f) => ({ add: t.add + f.change.added, del: t.del + f.change.deleted }), { add: 0, del: 0 });
  const open = notes.notes.filter((n) => !n.done);
  const listed = showDone ? notes.notes : open;

  const scrollToFile = useCallback((key: string, line?: number) => {
    const b = blocks.current.get(key);
    if (!b?.el) return;
    setCollapsed((c) => (c[key] ? { ...c, [key]: false } : c));
    b.el.scrollIntoView({ block: "start" });
    if (line) window.setTimeout(() => b.reveal(line), 120);
  }, []);

  // Opened on a file (from the dock): scroll there once it's listed.
  const lastFocus = useRef(0);
  useEffect(() => {
    if (!focus || focus.at === lastFocus.current) return;
    const key = fileKey(focus.repo, focus.path);
    if (!files.some((f) => f.key === key)) return;
    lastFocus.current = focus.at;
    requestAnimationFrame(() => scrollToFile(key));
  }, [focus, files, scrollToFile]);

  const jumpToNote = (n: Note) => {
    const key = fileKey(n.repo, n.path);
    if (files.some((f) => f.key === key)) scrollToFile(key, n.start);
    else onOpenFile(n.repo, n.path);
  };

  const addNote = async (body: string) => {
    if (!composer) return;
    const ok = await notes.add({ repo: composer.repo, path: composer.path, start: composer.start, end: composer.end, body, snippet: composer.snippet });
    if (ok) setComposer(null);
  };

  const prompt = notes.data ? notesPrompt(notes.data.file) : "";
  const prRepos = (workspace.prRefs ?? []).filter((p) => open.some((n) => n.repo === p.repo));

  return (
    <div className="commit-review pr-review chg">
      <div className="pr-review-side chg-side" style={{ width: side.width }}>
        <div className="rv-files-label">
          Changes <span>{loading ? "…" : `${files.length} file${files.length === 1 ? "" : "s"}`}</span>
        </div>
        {workspace.repos.map((repo) => {
          const list = changes[repo];
          if (!list?.length) return null;
          return (
            <div key={repo} className="chg-repo">
              {workspace.repos.length > 1 && <div className="chg-repo-name">{repo}</div>}
              {list.map((c) => {
                const key = fileKey(repo, c.path);
                const meta = STATUS_LABEL[c.status] ?? STATUS_LABEL.M;
                const count = open.filter((n) => n.repo === repo && n.path === c.path).length;
                return (
                  <button key={key} className={`tree-item git-change-row ${viewed.includes(key) ? "chg-viewed" : ""}`} onClick={() => scrollToFile(key)} title={c.path}>
                    <span className={`git-status-badge ${meta.cls}`}>{meta.letter}</span>
                    <span className="git-change-name">
                      {c.path.split("/").pop()}
                      {c.path.includes("/") && <span className="git-change-dir">{c.path.slice(0, c.path.lastIndexOf("/"))}</span>}
                    </span>
                    {count > 0 && <span className="chg-note-count">{count}</span>}
                  </button>
                );
              })}
            </div>
          );
        })}
        {!loading && files.length === 0 && <div className="git-empty">No changes — working trees clean</div>}

        <div className="rv-files-label chg-notes-label">
          Notes <span>{open.length} open</span>
          <button className={`btn-link ${showDone ? "on" : ""}`} onClick={() => setShowDone(!showDone)}>
            {showDone ? "Hide done" : "Show done"}
          </button>
        </div>
        {listed.length === 0 ? (
          <div className="git-empty">{notes.notes.length ? "All notes done" : "Click + next to a line (or drag across lines) to leave a note"}</div>
        ) : (
          listed.map((n) => (
            <div key={n.id} className={`chg-note-row ${n.done ? "done" : ""}`}>
              <CheckBox checked={n.done} onChange={(v) => notes.setDone(n.id, v)} label="Done" />
              <button className="chg-note-text" onClick={() => jumpToNote(n)} title={n.body}>
                <span className="chg-note-loc">
                  {n.path.split("/").pop()}:{n.start === n.end ? n.start : `${n.start}–${n.end}`}
                </span>
                <span className="chg-note-body">{n.body.split("\n")[0]}</span>
              </button>
            </div>
          ))
        )}
        {notes.notes.some((n) => n.done) && (
          <button className="btn-link chg-clear-done" onClick={() => notes.remove(notes.notes.filter((n) => n.done).map((n) => n.id))}>
            Clear done notes
          </button>
        )}
      </div>
      {side.handle}

      <div className="pr-diff-col">
        <div className="editor-filebar rv-filebar chg-toolbar">
          <span className="chg-summary">
            {files.length} file{files.length === 1 ? "" : "s"}
            <span className="git-stat-add">+{totals.add}</span>
            <span className="git-stat-del">−{totals.del}</span>
            {files.length > 0 && (
              <span className="chg-viewed-count">
                · {files.filter((f) => viewed.includes(f.key)).length}/{files.length} viewed
              </span>
            )}
          </span>
          <span className="editor-filebar-actions">
            {prRepos.length > 0 && (
              <button className="secondary btn-mini" onClick={() => setPrOpen(true)} title="Post open notes as a review on the pull request">
                Send to PR…
              </button>
            )}
            <button
              className="btn-mini"
              disabled={open.length === 0}
              title={open.length ? "Have an agent fix the open notes" : "No open notes"}
              onClick={(e) => {
                const r = e.currentTarget.getBoundingClientRect();
                setFixMenu({ x: r.left, y: r.bottom + 4 });
              }}
            >
              Fix {open.length || ""} note{open.length === 1 ? "" : "s"} with agent ▾
            </button>
          </span>
        </div>

        <div className="chg-scroll" ref={scrollRef}>
          {files.map((f) => (
            <FileBlock
              key={f.key}
              ws={ws}
              repo={f.repo}
              change={f.change}
              rev={revs[f.key] ?? 0}
              root={scrollRef}
              notes={(showDone ? notes.notes : open).filter((n) => n.repo === f.repo && n.path === f.change.path)}
              composer={composer && composer.repo === f.repo && composer.path === f.change.path ? composer : null}
              viewed={viewed.includes(f.key)}
              collapsed={!!collapsed[f.key] || viewed.includes(f.key)}
              register={(api) => {
                if (api) blocks.current.set(f.key, api);
                else blocks.current.delete(f.key);
              }}
              onToggle={() => {
                if (viewed.includes(f.key)) setViewed((v) => v.filter((x) => x !== f.key));
                else setCollapsed((c) => ({ ...c, [f.key]: !c[f.key] }));
              }}
              onViewed={(on) => setViewed((v) => (on ? [...v.filter((x) => x !== f.key), f.key] : v.filter((x) => x !== f.key)))}
              onRequestComment={(start, end, snippet) => setComposer({ repo: f.repo, path: f.change.path, start, end, snippet })}
              onAddNote={addNote}
              onCancelComposer={() => setComposer(null)}
              onSetDone={notes.setDone}
              onEditNote={notes.edit}
              onDeleteNote={(id) => notes.remove([id])}
              onOpenFile={() => onOpenFile(f.repo, f.change.path)}
            />
          ))}
          {!loading && files.length === 0 && (
            <div className="chg-empty">
              <strong>No local changes</strong>
              <span>Edits made by you or an agent show up here as they happen.</span>
            </div>
          )}
        </div>
      </div>

      {fixMenu && (
        <ContextMenu
          x={fixMenu.x}
          y={fixMenu.y}
          items={[
            ...sessions.map((s) => ({
              label: `Send to ${s.name}`,
              onSelect: () => {
                onSendToSession(s.id, prompt);
                toast.success(`Sent ${open.length} note${open.length === 1 ? "" : "s"} to ${s.name}`);
              },
            })),
            { label: "New agent session", onSelect: () => onNewSession(prompt) },
          ]}
          onClose={() => setFixMenu(null)}
        />
      )}
      {prOpen && (
        <SendToPrModal
          workspace={ws}
          prRefs={prRepos}
          notes={open}
          onSent={(ids) => notes.remove(ids)}
          onError={onError}
          onClose={() => setPrOpen(false)}
        />
      )}
    </div>
  );
}

interface BlockProps {
  ws: string;
  repo: string;
  change: GitChange;
  rev: number;
  root: React.RefObject<HTMLDivElement | null>;
  notes: Note[];
  composer: { start: number; end: number } | null;
  viewed: boolean;
  collapsed: boolean;
  register: (api: { el: HTMLDivElement | null; reveal: (line: number) => void } | null) => void;
  onToggle: () => void;
  onViewed: (on: boolean) => void;
  onRequestComment: (start: number, end: number, snippet: string) => void;
  onAddNote: (body: string) => Promise<void>;
  onCancelComposer: () => void;
  onSetDone: (id: string, done: boolean) => void;
  onEditNote: (id: string, body: string) => void;
  onDeleteNote: (id: string) => void;
  onOpenFile: () => void;
}

/** One file's diff: loads when scrolled near, sized to its content (the
 *  page scrolls, not the editor), with notes inline. */
function FileBlock(p: BlockProps) {
  const { ws, repo, change, rev, root, collapsed } = p;
  const monacoTheme = useMonacoTheme();
  const elRef = useRef<HTMLDivElement>(null);
  const [near, setNear] = useState(false);
  const [diff, setDiff] = useState<GitFileDiff | null>(null);
  const [height, setHeight] = useState(120);
  const editorRef = useRef<Monaco.IStandaloneDiffEditor | null>(null);

  useEffect(() => {
    const el = elRef.current;
    if (!el || near) return;
    const io = new IntersectionObserver((es) => es.some((e) => e.isIntersecting) && setNear(true), {
      root: root.current,
      rootMargin: "800px 0px",
    });
    io.observe(el);
    return () => io.disconnect();
  }, [near, root]);

  useEffect(() => {
    if (!near || collapsed) return;
    let stale = false;
    invoke<GitFileDiff>("git_file_diff", { workspace: ws, repo, path: change.path })
      .then((d) => !stale && setDiff((old) => (old && old.original === d.original && old.modified === d.modified ? old : d)))
      .catch(() => !stale && setDiff((old) => old ?? { original: "", modified: "" }));
    return () => {
      stale = true;
    };
  }, [near, collapsed, ws, repo, change.path, rev]);

  const lines = useMemo(() => (diff ? diff.modified.split("\n") : []), [diff]);
  const anchored = useMemo(() => p.notes.map((n) => ({ note: n, at: anchorNote(n, diff?.modified ?? null) })), [p.notes, diff]);

  const items: ZoneItem[] = useMemo(() => {
    const out: ZoneItem[] = anchored.map(({ note, at }) => ({
      key: `n:${note.id}`,
      side: "RIGHT" as Side,
      line: at.end,
      node: (
        <NoteCard
          note={note}
          moved={at.moved}
          lost={at.lost}
          onDone={(v) => p.onSetDone(note.id, v)}
          onEdit={(b) => p.onEditNote(note.id, b)}
          onDelete={() => p.onDeleteNote(note.id)}
        />
      ),
    }));
    if (p.composer) {
      const { start, end } = p.composer;
      out.push({
        key: "composer",
        side: "RIGHT",
        line: end,
        node: (
          <div className="rv-thread rv-composer">
            <div className="rv-composer-label">{start < end ? `Note on lines ${start}–${end}` : `Note on line ${end}`}</div>
            <CommentEditor
              placeholder="What should change here? (saved to REVIEW.md)"
              primary={{ label: "Add note", onClick: (t) => void p.onAddNote(t) }}
              onCancel={p.onCancelComposer}
            />
          </div>
        ),
      });
    }
    return out;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [anchored, p.composer]);

  const marks: RangeMark[] = useMemo(
    () => [
      ...anchored.filter(({ at }) => at.end > at.start).map(({ at }) => ({ side: "RIGHT" as Side, start: at.start, end: at.end, kind: "draft" as const })),
      ...(p.composer ? [{ side: "RIGHT" as Side, start: p.composer.start, end: p.composer.end, kind: "selecting" as const }] : []),
    ],
    [anchored, p.composer],
  );

  const zones = useReviewZones({
    items,
    marks,
    commentable: lines.length ? { right: [[1, lines.length]], left: [] } : undefined,
    split: false,
    onRequestComment: (_side, start, end) => p.onRequestComment(start, end, lines.slice(start - 1, end).join("\n")),
  });

  const onMount = (editor: Monaco.IStandaloneDiffEditor, monaco: typeof import("monaco-editor")) => {
    editorRef.current = editor;
    zones.onMount(editor, monaco);
    const mod = editor.getModifiedEditor();
    const fit = () => setHeight(Math.max(60, Math.min(mod.getContentHeight(), 20000)));
    mod.onDidContentSizeChange(fit);
    fit();
  };

  const register = p.register;
  useEffect(() => {
    register({
      el: elRef.current,
      reveal: (line) => {
        const ed = editorRef.current?.getModifiedEditor();
        const host = elRef.current;
        const scroller = root.current;
        if (!ed || !host || !scroller) return;
        const top = host.offsetTop + 34 + ed.getTopForLineNumber(line);
        scroller.scrollTo({ top: Math.max(0, top - 120), behavior: "smooth" });
      },
    });
    return () => register(null);
  }, [register, root]);

  const meta = STATUS_LABEL[change.status] ?? STATUS_LABEL.M;
  return (
    <div className="chg-file" ref={elRef}>
      <div className="chg-file-head">
        <button className="icon-button" onClick={p.onToggle} aria-label={collapsed ? "Expand" : "Collapse"}>
          <span className={`chg-chevron ${collapsed ? "" : "open"}`}>
            <ChevronIcon size={12} />
          </span>
        </button>
        <span className={`git-status-badge ${meta.cls}`}>{meta.letter}</span>
        <button className="chg-file-path btn-plain" onClick={p.onOpenFile} title="Open in editor">
          {change.path.includes("/") && <span className="rv-path-dir">{change.path.slice(0, change.path.lastIndexOf("/") + 1)}</span>}
          <span className="rv-path-name">{change.path.split("/").pop()}</span>
        </button>
        <span className="chg-file-repo">{repo}</span>
        <span className="git-change-stats">
          {change.added > 0 && <span className="git-stat-add">+{change.added}</span>}
          {change.deleted > 0 && <span className="git-stat-del">−{change.deleted}</span>}
        </span>
        <label className="chg-file-viewed">
          <CheckBox checked={p.viewed} onChange={p.onViewed} label="Viewed" />
          Viewed
        </label>
      </div>
      {!collapsed && (
        <div className="chg-file-body" style={{ height: diff ? height : 120 }}>
          {near && diff ? (
            <DiffEditor
              height="100%"
              theme={monacoTheme}
              beforeMount={beforeMount}
              onMount={onMount}
              language={langOf(change.path)}
              original={diff.original}
              modified={diff.modified}
              options={{
                readOnly: true,
                renderSideBySide: false,
                fontSize: 12.5,
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                automaticLayout: true,
                renderOverviewRuler: false,
                glyphMargin: true,
                diffWordWrap: "off",
                scrollbar: { vertical: "hidden", handleMouseWheel: false, alwaysConsumeMouseWheel: false },
                hideUnchangedRegions: { enabled: true, contextLineCount: 3, minimumLineCount: 6, revealLineCount: 20 },
              } as never}
              loading={<div className="table-loading"><span className="spinner" /> Loading diff…</div>}
            />
          ) : (
            <div className="table-loading">
              <span className="spinner" /> Loading diff…
            </div>
          )}
          {zones.portals}
        </div>
      )}
    </div>
  );
}

function NoteCard({
  note,
  moved,
  lost,
  onDone,
  onEdit,
  onDelete,
}: {
  note: Note;
  moved: boolean;
  lost: boolean;
  onDone: (done: boolean) => void;
  onEdit: (body: string) => void;
  onDelete: () => void;
}) {
  const [editing, setEditing] = useState(false);
  return (
    <div className={`rv-thread rv-thread-draft chg-note ${note.done ? "chg-note-done" : ""}`}>
      <div className="rv-comment">
        <div className="rv-comment-main">
          <div className="rv-comment-head">
            <CheckBox checked={note.done} onChange={onDone} label="Done" />
            <strong>Note</strong>
            {note.done && <span className="rv-pill">Done</span>}
            {moved && <span className="rv-pill" title={`Left on line ${note.start}; the code moved`}>moved</span>}
            {lost && <span className="rv-pill rv-pill-pending" title="The code this note was left on changed">code changed</span>}
            {note.start !== note.end && <span className="rv-comment-time">Lines {note.start}–{note.end}</span>}
            {!editing && (
              <span className="rv-comment-tools">
                <button className="icon-button" aria-label="Edit note" onClick={() => setEditing(true)}>
                  <PencilIcon size={12} />
                </button>
                <button className="icon-button icon-button-danger" aria-label="Delete note" onClick={onDelete}>
                  <TrashIcon size={12} />
                </button>
              </span>
            )}
          </div>
          {editing ? (
            <CommentEditor
              initial={note.body}
              placeholder="Edit note"
              primary={{
                label: "Save",
                onClick: (t) => {
                  onEdit(t);
                  setEditing(false);
                },
              }}
              onCancel={() => setEditing(false)}
            />
          ) : (
            <SafeMarkdown content={note.body} />
          )}
        </div>
      </div>
    </div>
  );
}
