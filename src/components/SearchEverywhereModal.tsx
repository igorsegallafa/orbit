import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileTypeIcon } from "./FileIcons";
import { SearchIcon } from "./Icons";
import { SYMBOL_KIND } from "./SymbolSearch";
import { Workspace } from "../types/config";
import { Command, listCommands } from "../lib/commands";
import { rankPaths, fuzzyScore } from "../lib/fuzzy";
import { SymbolHit, searchWorkspaceSymbols } from "../lib/lsp/manager";

interface FileEntry {
  repo: string;
  path: string;
}

interface TextMatch {
  repo: string;
  path: string;
  line: number;
  col: number;
  text: string;
}

export type SearchTab = "all" | "files" | "symbols" | "actions" | "text";

const TABS: { id: SearchTab; label: string }[] = [
  { id: "all", label: "All" },
  { id: "files", label: "Files" },
  { id: "symbols", label: "Symbols" },
  { id: "actions", label: "Actions" },
  { id: "text", label: "Text" },
];

type Item =
  | { kind: "file"; file: FileEntry }
  | { kind: "symbol"; hit: SymbolHit }
  | { kind: "action"; cmd: Command }
  | { kind: "text"; match: TextMatch };

interface Props {
  workspace: Workspace;
  initialTab?: SearchTab;
  onOpenFile: (repo: string, path: string) => void;
  /** Opens a file at a position (symbols, text matches). */
  onOpenAt: (repo: string, path: string, line: number, column: number, length: number) => void;
  onClose: () => void;
}

/** Results per section in the All tab. */
const ALL_LIMIT = { files: 8, symbols: 8, actions: 5 };

/**
 * JetBrains-style Search Everywhere (double Shift; Ctrl+Shift+A opens it on
 * Actions): files, symbols from the running language servers, actions, and
 * text in files. Tab / Shift+Tab switch tabs.
 */
export function SearchEverywhereModal({ workspace, initialTab = "all", onOpenFile, onOpenAt, onClose }: Props) {
  const [tab, setTab] = useState<SearchTab>(initialTab);
  const [files, setFiles] = useState<FileEntry[] | null>(null);
  const [query, setQuery] = useState("");
  const [symbols, setSymbols] = useState<{ q: string; hits: SymbolHit[]; servers: number } | null>(null);
  const [text, setText] = useState<{ q: string; matches: TextMatch[]; truncated: boolean } | null>(null);
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);
  const q = query.trim();

  // The file list, once per open.
  useEffect(() => {
    let cancelled = false;
    invoke<FileEntry[]>("list_workspace_files", { workspace: workspace.name })
      .then((e) => !cancelled && setFiles(e))
      .catch(() => !cancelled && setFiles([]));
    return () => {
      cancelled = true;
    };
  }, [workspace.name]);

  // Symbols (language servers) and text (git grep) as you type, debounced.
  const wantSymbols = tab === "all" || tab === "symbols";
  useEffect(() => {
    if (!wantSymbols || q.length < 2) return;
    let dead = false;
    const t = window.setTimeout(() => {
      searchWorkspaceSymbols(workspace.name, q).then((r) => !dead && setSymbols({ q, ...r }));
    }, 180);
    return () => {
      dead = true;
      window.clearTimeout(t);
    };
  }, [q, wantSymbols, workspace.name]);

  useEffect(() => {
    if (tab !== "text" || q.length < 2) return;
    let dead = false;
    const t = window.setTimeout(() => {
      invoke<{ matches: TextMatch[]; truncated: boolean }>("search_workspace", {
        workspace: workspace.name,
        query: { query: q, caseSensitive: false, wholeWord: false, regex: false, mask: "", repos: [] },
      })
        .then((r) => !dead && setText({ q, ...r }))
        .catch(() => !dead && setText({ q, matches: [], truncated: false }));
    }, 250);
    return () => {
      dead = true;
      window.clearTimeout(t);
    };
  }, [q, tab, workspace.name]);

  const sections = useMemo(() => {
    const out: { title: string; tab: SearchTab; items: Item[] }[] = [];
    const lower = q.toLowerCase();
    const all = tab === "all";
    if (all || tab === "files") {
      const list = rankPaths(files ?? [], (f) => `${f.repo}/${f.path}`, q, all ? ALL_LIMIT.files : 60);
      out.push({ title: "Files", tab: "files", items: list.map((file) => ({ kind: "file", file })) });
    }
    if ((all || tab === "symbols") && symbols && q.length >= 2) {
      const hits = symbols.hits.filter((h) => h.ctx).slice(0, all ? ALL_LIMIT.symbols : 100);
      out.push({ title: "Symbols", tab: "symbols", items: hits.map((hit) => ({ kind: "symbol", hit })) });
    }
    if (all || tab === "actions") {
      const cmds = listCommands()
        .map((cmd) => ({ cmd, score: lower ? fuzzyScore(cmd.title.toLowerCase(), lower) : 1 }))
        .filter((c) => c.score > 0)
        .sort((a, b) => b.score - a.score)
        .slice(0, all ? (lower ? ALL_LIMIT.actions : 0) : 100);
      out.push({ title: "Actions", tab: "actions", items: cmds.map(({ cmd }) => ({ kind: "action", cmd })) });
    }
    if (tab === "text" && text && q.length >= 2) {
      out.push({ title: "Text", tab: "text", items: text.matches.slice(0, 200).map((match) => ({ kind: "text", match })) });
    }
    return out.filter((s) => s.items.length > 0);
  }, [tab, files, symbols, text, q]);

  const items = useMemo(() => sections.flatMap((s) => s.items), [sections]);

  useEffect(() => setSelected(0), [q, tab]);
  useEffect(() => {
    listRef.current?.querySelector(`[data-idx="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const choose = (it: Item) => {
    onClose();
    switch (it.kind) {
      case "file":
        return onOpenFile(it.file.repo, it.file.path);
      case "symbol":
        return it.hit.ctx && onOpenAt(it.hit.ctx.repo, it.hit.ctx.path, it.hit.line, it.hit.column, 0);
      case "action":
        // After the modal is gone: actions may open their own.
        return window.setTimeout(() => it.cmd.run(), 0);
      case "text":
        return onOpenAt(it.match.repo, it.match.path, it.match.line, it.match.col, q.length);
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "Tab") {
      e.preventDefault();
      const i = TABS.findIndex((t) => t.id === tab);
      setTab(TABS[(i + (e.shiftKey ? TABS.length - 1 : 1)) % TABS.length].id);
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, items.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (items[selected]) choose(items[selected]);
    }
  };

  const placeholder: Record<SearchTab, string> = {
    all: "Search files, symbols and actions…",
    files: "Search files…",
    symbols: "Search classes, functions, variables…",
    actions: "Search actions…",
    text: "Search text in files…",
  };

  const empty = () => {
    if (tab === "files" || tab === "all") {
      if (files === null) return <><span className="spinner" /> Indexing files…</>;
    }
    if (tab === "symbols") {
      if (q.length < 2) return "Type at least 2 characters";
      if (symbols?.servers === 0) return "No language server running in this workspace: open a source file to start one";
    }
    if (tab === "text" && q.length < 2) return "Type at least 2 characters";
    return q ? `Nothing matches “${q}”` : "Nothing to show";
  };

  let idx = 0;
  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="search-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="search-tabs" role="tablist">
          {TABS.map((t) => (
            <button
              key={t.id}
              role="tab"
              aria-selected={tab === t.id}
              className={`btn-plain search-tab${tab === t.id ? " search-tab-active" : ""}`}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => setTab(t.id)}
            >
              {t.label}
            </button>
          ))}
          <span className="search-tabs-hint">Tab to switch</span>
        </div>
        <div className="search-input-row">
          <span className="search-icon">
            <SearchIcon size={15} />
          </span>
          <input
            autoFocus
            className="search-input"
            value={query}
            placeholder={placeholder[tab]}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKey}
          />
          <span className="search-hint">{workspace.name.replace(/^@/, "")}</span>
        </div>
        <div className="search-list" ref={listRef}>
          {items.length === 0 ? (
            <div className="search-empty">{empty()}</div>
          ) : (
            sections.map((s) => (
              <div key={s.title}>
                {tab === "all" && (
                  <button className="btn-plain search-section" onMouseDown={(e) => e.preventDefault()} onClick={() => setTab(s.tab)}>
                    {s.title}
                    <span className="search-section-more">more ›</span>
                  </button>
                )}
                {s.items.map((it) => {
                  const i = idx++;
                  return (
                    <button
                      key={i}
                      data-idx={i}
                      className={`search-row ${i === selected ? "search-row-active" : ""}`}
                      onClick={() => choose(it)}
                      onMouseMove={() => i !== selected && setSelected(i)}
                    >
                      <Row item={it} />
                    </button>
                  );
                })}
              </div>
            ))
          )}
          {tab === "text" && text?.truncated && <div className="search-empty">Showing the first matches: refine the search</div>}
        </div>
      </div>
    </div>
  );
}

function Row({ item }: { item: Item }) {
  switch (item.kind) {
    case "file": {
      const name = item.file.path.split("/").pop() ?? item.file.path;
      return (
        <>
          <FileTypeIcon name={name} />
          <span className="search-row-name">{name}</span>
          <span className="search-row-path">
            {item.file.repo}/{item.file.path}
          </span>
        </>
      );
    }
    case "symbol": {
      const h = item.hit;
      return (
        <>
          <span className="search-kind">{SYMBOL_KIND[h.kind] ?? "symbol"}</span>
          <span className="search-row-name">{h.name}</span>
          {h.container && <span className="search-row-container">{h.container}</span>}
          <span className="search-row-path">
            {h.ctx ? `${h.ctx.path.split("/").pop()}:${h.line}` : ""}
          </span>
        </>
      );
    }
    case "action":
      return (
        <>
          <span className="search-kind">action</span>
          <span className="search-row-name">{item.cmd.title}</span>
          {item.cmd.keys && <span className="search-row-keys">{item.cmd.keys}</span>}
        </>
      );
    case "text": {
      const m = item.match;
      return (
        <>
          <span className="search-row-text">{m.text.trim()}</span>
          <span className="search-row-path">
            {m.path.split("/").pop()}:{m.line}
          </span>
        </>
      );
    }
  }
}
