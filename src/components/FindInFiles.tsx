import { Fragment, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import Editor, { OnMount } from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";
import { Workspace } from "../types/config";
import { SearchIcon } from "./Icons";
import { tooltip } from "./Tooltip";
import { defineMonacoThemes, useMonacoTheme } from "../lib/theme";

export interface SearchMatch {
  repo: string;
  path: string;
  line: number;
  col: number;
  text: string;
}

interface SearchResult {
  matches: SearchMatch[];
  truncated: boolean;
}

interface FindOptions {
  query: string;
  caseSensitive: boolean;
  wholeWord: boolean;
  regex: boolean;
  mask: string;
  /** "" = every repo. */
  repo: string;
}

/** Opens a match in the editor (tab + line + selection). */
export type OpenMatch = (m: SearchMatch, length: number) => void;

const OPTS_KEY = "orbit.find.options";

function loadOpts(): FindOptions {
  const base: FindOptions = { query: "", caseSensitive: false, wholeWord: false, regex: false, mask: "", repo: "" };
  try {
    return { ...base, ...JSON.parse(localStorage.getItem(OPTS_KEY) ?? "{}"), query: "" };
  } catch {
    return base;
  }
}

/** Same semantics as the backend, to highlight every hit on a line. */
function matcher(o: FindOptions): RegExp | null {
  if (!o.query) return null;
  try {
    const src = o.regex ? o.query : o.query.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    return new RegExp(o.wholeWord ? `\\b(?:${src})\\b` : src, o.caseSensitive ? "g" : "gi");
  } catch {
    return null;
  }
}

function ranges(text: string, re: RegExp | null, fallbackCol: number, fallbackLen: number): [number, number][] {
  const out: [number, number][] = [];
  if (re) {
    re.lastIndex = 0;
    let m: RegExpExecArray | null;
    while ((m = re.exec(text)) && out.length < 50) {
      if (m[0].length === 0) {
        re.lastIndex++;
        continue;
      }
      out.push([m.index, m.index + m[0].length]);
    }
  }
  if (!out.length) out.push([fallbackCol - 1, fallbackCol - 1 + fallbackLen]);
  return out;
}

/** `context`: chars kept before the first hit, so a hit far right in a
 *  long line stays visible (the rest is elided with "…"). */
function Highlighted({ text, hits, context = 40 }: { text: string; hits: [number, number][]; context?: number }) {
  // Leading indentation is noise in a result row.
  const indent = text.length - text.trimStart().length;
  const lead = Math.max(indent, (hits[0]?.[0] ?? 0) - context);
  const parts: React.ReactNode[] = lead > indent ? [<Fragment key="el">…</Fragment>] : [];
  let at = lead;
  hits.forEach(([a, b], i) => {
    if (b <= at) return;
    if (a > at) parts.push(<Fragment key={`t${i}`}>{text.slice(at, a)}</Fragment>);
    parts.push(<mark key={`m${i}`}>{text.slice(Math.max(a, at), b)}</mark>);
    at = b;
  });
  parts.push(<Fragment key="rest">{text.slice(at)}</Fragment>);
  return <>{parts}</>;
}

/** Debounced search state of the popup. */
function useFind(workspace: Workspace) {
  const [opts, setOpts] = useState<FindOptions>(loadOpts);
  const [result, setResult] = useState<SearchResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const seq = useRef(0);

  useEffect(() => {
    try {
      const { query: _q, ...rest } = opts;
      localStorage.setItem(OPTS_KEY, JSON.stringify(rest));
    } catch {
      // per-viewer convenience only
    }
  }, [opts]);

  useEffect(() => {
    const id = ++seq.current;
    if (!opts.query) {
      setResult(null);
      setError(null);
      setLoading(false);
      return;
    }
    setLoading(true);
    const t = window.setTimeout(() => {
      invoke<SearchResult>("search_workspace", {
        workspace: workspace.name,
        query: { query: opts.query, caseSensitive: opts.caseSensitive, wholeWord: opts.wholeWord, regex: opts.regex, mask: opts.mask, repos: opts.repo ? [opts.repo] : [] },
      })
        .then((r) => {
          if (id !== seq.current) return;
          setResult(r);
          setError(null);
        })
        .catch((e) => {
          if (id !== seq.current) return;
          setResult(null);
          setError(String(e));
        })
        .finally(() => id === seq.current && setLoading(false));
    }, 220);
    return () => window.clearTimeout(t);
  }, [workspace.name, opts]);

  const re = useMemo(() => matcher(opts), [opts]);
  const hitsOf = useCallback((m: SearchMatch) => ranges(m.text, re, m.col, opts.regex ? 1 : opts.query.length), [re, opts.regex, opts.query]);
  const files = useMemo(() => new Set((result?.matches ?? []).map((m) => `${m.repo}/${m.path}`)).size, [result]);
  const set = (patch: Partial<FindOptions>) => setOpts((o) => ({ ...o, ...patch }));

  return { opts, set, result, loading, error, hitsOf, files };
}

function Toggles({ opts, set }: { opts: FindOptions; set: (p: Partial<FindOptions>) => void }) {
  const t = (key: "caseSensitive" | "wholeWord" | "regex", label: string, tip: string) => (
    <button
      className={`find-toggle ${opts[key] ? "on" : ""}`}
      aria-pressed={opts[key]}
      aria-label={tip}
      onMouseEnter={(e) => tooltip.show(tip, e)}
      onMouseLeave={() => tooltip.hide()}
      onClick={() => set({ [key]: !opts[key] })}
    >
      {label}
    </button>
  );
  return (
    <span className="find-toggles">
      {t("caseSensitive", "Aa", "Match case (Alt+C)")}
      {t("wholeWord", "W", "Words (Alt+W)")}
      {t("regex", ".*", "Regex (Alt+X)")}
    </span>
  );
}

function summary(f: ReturnType<typeof useFind>): string {
  if (f.error) return f.error;
  if (!f.opts.query) return "";
  if (f.loading && !f.result) return "Searching…";
  const n = f.result?.matches.length ?? 0;
  if (n === 0) return "No matches";
  return `${f.result?.truncated ? `${n}+` : n} match${n === 1 ? "" : "es"} in ${f.files} file${f.files === 1 ? "" : "s"}`;
}

function optionKeys(e: React.KeyboardEvent, f: ReturnType<typeof useFind>) {
  if (!e.altKey) return false;
  const k = e.key.toLowerCase();
  if (k === "c") f.set({ caseSensitive: !f.opts.caseSensitive });
  else if (k === "w") f.set({ wholeWord: !f.opts.wholeWord });
  else if (k === "x") f.set({ regex: !f.opts.regex });
  else return false;
  e.preventDefault();
  return true;
}

function ScopeBar({ workspace, f }: { workspace: Workspace; f: ReturnType<typeof useFind> }) {
  return (
    <div className="find-scope">
      {workspace.repos.length > 1 && (
        <span className="find-chips">
          {["", ...workspace.repos].map((r) => (
            <button key={r || "all"} className={`find-chip ${f.opts.repo === r ? "on" : ""}`} onClick={() => f.set({ repo: r })}>
              {r || "All repos"}
            </button>
          ))}
        </span>
      )}
      <input
        className="find-mask"
        value={f.opts.mask}
        placeholder="File mask, e.g. *.ts, !*.test.ts"
        spellCheck={false}
        onChange={(e) => f.set({ mask: e.target.value })}
      />
    </div>
  );
}

/** IntelliJ-style Find in Files popup: results on top, live preview below. */
export function FindPopup({ workspace, initialQuery, onOpen, onClose }: { workspace: Workspace; initialQuery?: string; onOpen: OpenMatch; onClose: () => void }) {
  const f = useFind(workspace);
  const [sel, setSel] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const matches = f.result?.matches ?? [];
  const current = matches[Math.min(sel, matches.length - 1)];

  useEffect(() => {
    if (initialQuery) f.set({ query: initialQuery });
    inputRef.current?.select();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => setSel(0), [f.result]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-idx="${sel}"]`)?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  const open = (m: SearchMatch | undefined) => {
    if (!m) return;
    const [a, b] = f.hitsOf(m)[0];
    onOpen({ ...m, col: a + 1 }, b - a);
    onClose();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (optionKeys(e, f)) return;
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 1, matches.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 1, 0));
    } else if (e.key === "PageDown") {
      e.preventDefault();
      setSel((s) => Math.min(s + 10, matches.length - 1));
    } else if (e.key === "PageUp") {
      e.preventDefault();
      setSel((s) => Math.max(s - 10, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      open(current);
    }
  };

  // Portal: an animated ancestor would make the fixed overlay relative to
  // the content area instead of the window, pushing the popup off-center.
  return createPortal(
    <div className="modal-overlay find-overlay" onMouseDown={onClose}>
      <div className={`find-popup ${matches.length ? "" : "idle"}`} onMouseDown={(e) => e.stopPropagation()} onKeyDown={onKey}>
        <div className="find-head">
          <span className="find-title">Find in Files</span>
          <span className={`find-summary ${f.error ? "error" : ""}`}>
            {f.loading && f.result ? <span className="spinner" /> : null}
            {summary(f)}
          </span>
        </div>
        <div className="find-inputrow">
          <SearchIcon size={14} />
          <input
            ref={inputRef}
            autoFocus
            className="find-input"
            value={f.opts.query}
            placeholder={`Search in ${workspace.name}`}
            spellCheck={false}
            onChange={(e) => f.set({ query: e.target.value })}
          />
          <Toggles opts={f.opts} set={f.set} />
        </div>
        <ScopeBar workspace={workspace} f={f} />
        <div className="find-results" ref={listRef}>
          {matches.map((m, i) => (
            <button
              key={`${m.repo}/${m.path}:${m.line}:${i}`}
              data-idx={i}
              className={`find-row ${i === sel ? "on" : ""}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setSel(i)}
              onDoubleClick={() => open(m)}
            >
              <span className="find-row-text">
                <Highlighted text={m.text} hits={f.hitsOf(m)} />
              </span>
              <span className="find-row-loc">
                {m.path.split("/").pop()} <span className="find-row-line">{m.line}</span>
              </span>
            </button>
          ))}
          {f.opts.query && !f.loading && !f.error && matches.length === 0 && <div className="find-empty">No matches in {f.opts.repo || "the workspace"}</div>}
          {!f.opts.query && <div className="find-empty">Type to search every repo of {workspace.name}. Enter opens the match.</div>}
        </div>
        {matches.length > 0 && <FindPreview workspace={workspace.name} match={current} hits={current ? f.hitsOf(current) : []} allMatches={matches} hitsOf={f.hitsOf} onOpen={() => open(current)} />}
        <div className="find-foot">
          <span>↑↓ navigate · Enter open · Alt+C / W / X options</span>
          {current && (
            <span className="find-foot-path mono">
              {current.repo}/{current.path}:{current.line}
            </span>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}

const PREVIEW_MAX = 1_048_576;

function FindPreview({
  workspace,
  match,
  hits,
  allMatches,
  hitsOf,
  onOpen,
}: {
  workspace: string;
  match: SearchMatch | undefined;
  hits: [number, number][];
  allMatches: SearchMatch[];
  hitsOf: (m: SearchMatch) => [number, number][];
  onOpen: () => void;
}) {
  const monacoTheme = useMonacoTheme();
  const [content, setContent] = useState<{ key: string; text: string } | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const cache = useRef(new Map<string, string>());
  const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);
  const decos = useRef<Monaco.editor.IEditorDecorationsCollection | null>(null);
  const key = match ? `${match.repo}/${match.path}` : "";

  useEffect(() => {
    if (!match) return;
    const cached = cache.current.get(key);
    if (cached !== undefined) {
      setContent({ key, text: cached });
      setFailed(null);
      return;
    }
    let live = true;
    invoke<string>("read_file", { workspace, repo: match.repo, path: match.path })
      .then((text) => {
        if (!live) return;
        if (text.length <= PREVIEW_MAX) cache.current.set(key, text);
        setContent({ key, text });
        setFailed(null);
      })
      .catch((e) => live && setFailed(String(e)));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  // Highlight every hit of this file, reveal the selected one.
  useEffect(() => {
    const ed = editorRef.current;
    if (!ed || !match || content?.key !== key) return;
    const fileHits = allMatches.filter((m) => m.repo === match.repo && m.path === match.path);
    decos.current?.clear();
    decos.current = ed.createDecorationsCollection([
      { range: { startLineNumber: match.line, startColumn: 1, endLineNumber: match.line, endColumn: 1 }, options: { isWholeLine: true, className: "find-preview-line" } },
      ...fileHits.flatMap((m) =>
        (m === match ? hits : hitsOf(m)).map(([a, b]) => ({
          range: { startLineNumber: m.line, startColumn: a + 1, endLineNumber: m.line, endColumn: b + 1 },
          options: { inlineClassName: m === match ? "find-preview-hit on" : "find-preview-hit" },
        })),
      ),
    ]);
    ed.revealLineInCenter(match.line);
  }, [content, match, key, hits, allMatches, hitsOf]);

  const onMount: OnMount = (ed) => {
    editorRef.current = ed;
    // Double click in the preview jumps into the real editor there.
    ed.onMouseDown((e) => {
      if (e.event.detail === 2) onOpen();
    });
    setContent((c) => (c ? { ...c } : c));
  };

  if (!match) return <div className="find-preview find-preview-empty" />;
  return (
    <div className="find-preview">
      {failed ? (
        <div className="find-empty">{failed}</div>
      ) : content?.key === key ? (
        <Editor
          height="100%"
          theme={monacoTheme}
          beforeMount={defineMonacoThemes}
          path={`find-preview/${key}`}
          value={content.text}
          onMount={onMount}
          options={{
            readOnly: true,
            domReadOnly: true,
            fontSize: 12,
            minimap: { enabled: false },
            scrollBeyondLastLine: false,
            automaticLayout: true,
            renderLineHighlight: "none",
            lineNumbersMinChars: 4,
            folding: false,
            contextmenu: false,
            scrollbar: { verticalScrollbarSize: 8, horizontalScrollbarSize: 8 },
          }}
        />
      ) : (
        <div className="find-empty">
          <span className="spinner" />
        </div>
      )}
    </div>
  );
}
