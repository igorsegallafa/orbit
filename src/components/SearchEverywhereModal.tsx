import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { FileTypeIcon } from "./FileIcons";
import { SearchIcon } from "./Icons";
import { Workspace } from "../types/config";

interface Entry {
  repo: string;
  path: string;
}

interface Props {
  workspace: Workspace;
  onOpenFile: (repo: string, path: string) => void;
  onClose: () => void;
}

/**
 * JetBrains-style Search Everywhere, triggered by double-shift. Fuzzy-matches
 * file names across all repos of the focused workspace; arrows/enter navigate.
 */
export function SearchEverywhereModal({ workspace, onOpenFile, onClose }: Props) {
  const [entries, setEntries] = useState<Entry[] | null>(null);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  // Fetch the file list once per open
  useEffect(() => {
    let cancelled = false;
    invoke<Entry[]>("list_workspace_files", { workspace: workspace.name })
      .then((e) => {
        if (!cancelled) setEntries(e);
      })
      .catch(() => {
        if (!cancelled) setEntries([]);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const results = useMemo(() => {
    if (!entries) return [];
    const q = query.trim().toLowerCase();
    if (!q) return entries.slice(0, 50);
    return entries
      .map((e) => ({ e, score: fuzzyScore(e.path.toLowerCase(), q) }))
      .filter((r) => r.score > 0)
      .sort((a, b) => b.score - a.score)
      .slice(0, 50)
      .map((r) => r.e);
  }, [entries, query]);

  useEffect(() => {
    setSelected(0);
  }, [query]);

  // Keep the selected row in view
  useEffect(() => {
    listRef.current?.querySelector(`[data-idx="${selected}"]`)?.scrollIntoView({ block: "nearest" });
  }, [selected]);

  const open = (entry: Entry) => {
    onOpenFile(entry.repo, entry.path);
    onClose();
  };

  const onKey = (e: React.KeyboardEvent) => {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, results.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (results[selected]) open(results[selected]);
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="search-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="search-input-row">
          <span className="search-icon"><SearchIcon size={15} /></span>
          <input
            autoFocus
            className="search-input"
            value={query}
            placeholder="Search files everywhere…"
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={onKey}
          />
          <span className="search-hint">{workspace.name}</span>
        </div>
        <div className="search-list" ref={listRef}>
          {entries === null ? (
            <div className="search-loading">
              <span className="spinner" /> Indexing files…
            </div>
          ) : results.length === 0 ? (
            <div className="search-empty">No files match “{query}”</div>
          ) : (
            results.map((r, i) => {
              const name = r.path.split("/").pop() ?? r.path;
              return (
                <button
                  key={`${r.repo}/${r.path}`}
                  data-idx={i}
                  className={`search-row ${i === selected ? "search-row-active" : ""}`}
                  onClick={() => open(r)}
                  onMouseEnter={() => setSelected(i)}
                >
                  <FileTypeIcon name={name} />
                  <span className="search-row-name">{name}</span>
                  <span className="search-row-path">
                    {r.repo}/{r.path}
                  </span>
                </button>
              );
            })
          )}
        </div>
      </div>
    </div>
  );
}

/** Simple subsequence fuzzy score: longer runs of consecutive matches and
 *  matches near word boundaries score higher. 0 = no match. */
function fuzzyScore(s: string, q: string): number {
  if (s === q) return 1000;
  let si = 0;
  let score = 0;
  let streak = 0;
  for (const ch of q) {
    if (si >= s.length) return 0;
    const idx = s.indexOf(ch, si);
    if (idx === -1) return 0;
    streak = idx === si ? streak + 1 : 0;
    score += 1 + streak * 2;
    if (idx === 0 || "/-._".includes(s[idx - 1] ?? "")) score += 3;
    si = idx + 1;
  }
  return score;
}