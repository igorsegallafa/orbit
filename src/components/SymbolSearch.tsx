import { useEffect, useRef, useState } from "react";
import type * as Monaco from "monaco-editor";
import { SymbolHit, openLocation, searchSymbols } from "../lib/lsp/manager";
import { toast } from "./Toast";

interface Props {
  /** A model of the project: its language server answers the search. */
  model: Monaco.editor.ITextModel;
  onClose: () => void;
}

/** LSP SymbolKind → short tag shown next to each hit. */
const KIND: Record<number, string> = {
  2: "module", 3: "namespace", 4: "package", 5: "class", 6: "method", 7: "property", 8: "field", 9: "constructor",
  10: "enum", 11: "interface", 12: "function", 13: "variable", 14: "constant", 22: "enum member", 23: "struct",
  24: "event", 25: "operator", 26: "type param",
};

/** Ctrl+T: symbols across the whole project, from its language server. */
export function SymbolSearch({ model, onClose }: Props) {
  const [query, setQuery] = useState("");
  const [hits, setHits] = useState<SymbolHit[] | null>(null);
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!query.trim()) {
      setHits(null);
      return;
    }
    let dead = false;
    const t = window.setTimeout(() => {
      searchSymbols(model, query.trim()).then((h) => {
        if (dead) return;
        setHits(h);
        setActive(0);
      });
    }, 180);
    return () => {
      dead = true;
      window.clearTimeout(t);
    };
  }, [query, model]);

  useEffect(() => {
    listRef.current?.querySelector(".symbol-hit.active")?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const open = (h: SymbolHit) => {
    if (!h.ctx) {
      toast.info(`${h.name} is outside this repository`, { description: `${h.file}:${h.line}` });
      return;
    }
    onClose();
    openLocation(h.ctx, h.line, h.column);
  };

  return (
    <div className="modal-overlay find-overlay" onMouseDown={onClose}>
      <div className="symbol-search" onMouseDown={(e) => e.stopPropagation()}>
        <input
          autoFocus
          placeholder="Go to symbol in project (class, function, variable…)"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            else if (e.key === "ArrowDown") {
              e.preventDefault();
              setActive((a) => Math.min(a + 1, (hits?.length ?? 1) - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(a - 1, 0));
            } else if (e.key === "Enter" && hits?.[active]) open(hits[active]);
          }}
        />
        <div className="symbol-list" ref={listRef}>
          {hits === null ? (
            <div className="symbol-empty">Type to search the project's symbols.</div>
          ) : hits.length === 0 ? (
            <div className="symbol-empty">No symbol matches "{query}".</div>
          ) : (
            hits.map((h, i) => (
              <button
                key={`${h.file}:${h.line}:${h.name}:${i}`}
                className={`btn-plain symbol-hit ${i === active ? "active" : ""}`}
                onMouseEnter={() => setActive(i)}
                onClick={() => open(h)}
              >
                <span className="symbol-kind">{KIND[h.kind] ?? "symbol"}</span>
                <span className="symbol-name">{h.name}</span>
                {h.container && <span className="symbol-container">{h.container}</span>}
                <span className="symbol-file">
                  {h.ctx ? h.ctx.path : h.file}:{h.line}
                </span>
              </button>
            ))
          )}
        </div>
      </div>
    </div>
  );
}
