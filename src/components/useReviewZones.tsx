import { ReactNode, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { editor as Monaco } from "monaco-editor";
import { Commentable, Side, hunkOf } from "../types/review";

type DiffEditor = Monaco.IStandaloneDiffEditor;
type CodeEditor = Monaco.ICodeEditor;
type MonacoNs = typeof import("monaco-editor");

/** Something rendered inline under a line of the diff. */
export interface ZoneItem {
  key: string;
  side: Side;
  line: number;
  node: ReactNode;
}

/** Lines to tint (comment ranges, the range being selected). */
export interface RangeMark {
  side: Side;
  start: number;
  end: number;
  kind: "thread" | "draft" | "selecting";
}

interface Entry {
  editor: CodeEditor;
  line: number;
  zoneId: string;
  zone: Monaco.IViewZone;
  host: HTMLDivElement;
  ro: ResizeObserver;
}

/**
 * Review layer on a Monaco DiffEditor: React content in view zones under
 * lines, a "+" on commentable lines (click, drag across lines, or click
 * inside a multi-line selection) and tinted comment ranges. LEFT (base) side
 * items only exist in split view, where the original editor is visible.
 */
export function useReviewZones({
  items,
  marks,
  commentable,
  split,
  onRequestComment,
}: {
  items: ZoneItem[];
  marks: RangeMark[];
  commentable: Commentable | undefined;
  split: boolean;
  onRequestComment: (side: Side, start: number, end: number) => void;
}) {
  const [diff, setDiff] = useState<DiffEditor | null>(null);
  const monacoRef = useRef<MonacoNs | null>(null);
  const entries = useRef(new Map<string, Entry>());
  const [, rerender] = useState(0);
  const latest = useRef({ commentable, onRequestComment, split });
  latest.current = { commentable, onRequestComment, split };
  const [selecting, setSelecting] = useState<RangeMark | null>(null);

  const onMount = useCallback((editor: DiffEditor, monaco: MonacoNs) => {
    monacoRef.current = monaco;
    entries.current = new Map();
    setDiff(editor);
  }, []);

  const editorFor = useCallback(
    (side: Side): CodeEditor | null => {
      if (!diff) return null;
      if (side === "RIGHT") return diff.getModifiedEditor();
      return split ? diff.getOriginalEditor() : null;
    },
    [diff, split],
  );

  // Sync view zones with the items: add new, move changed, drop stale.
  useLayoutEffect(() => {
    if (!diff) return;
    const map = entries.current;
    const wanted = new Map(items.map((it) => [it.key, it]));
    let changed = false;

    for (const [key, e] of map) {
      const it = wanted.get(key);
      if (!it || editorFor(it.side) !== e.editor || it.line !== e.line) {
        e.ro.disconnect();
        e.editor.changeViewZones((a) => a.removeZone(e.zoneId));
        map.delete(key);
        changed = true;
      }
    }

    for (const it of items) {
      if (map.has(it.key)) continue;
      const editor = editorFor(it.side);
      if (!editor) continue;
      const dom = document.createElement("div");
      dom.className = "rv-zone";
      const host = document.createElement("div");
      host.className = "rv-zone-inner";
      // Monaco listens on its container and would take focus and keys away
      // from the textareas in here; wheel still scrolls the editor.
      for (const type of ["mousedown", "pointerdown", "keydown", "keyup", "keypress"]) {
        host.addEventListener(type, (ev) => ev.stopPropagation());
      }
      dom.appendChild(host);
      const zone: Monaco.IViewZone = { afterLineNumber: it.line, heightInPx: 60, domNode: dom, suppressMouseDown: true };
      let zoneId = "";
      editor.changeViewZones((a) => {
        zoneId = a.addZone(zone);
      });
      const ro = new ResizeObserver(() => {
        const h = Math.ceil(host.getBoundingClientRect().height) + 10;
        if (Math.abs(h - (zone.heightInPx ?? 0)) > 1) {
          zone.heightInPx = h;
          editor.changeViewZones((a) => a.layoutZone(zoneId));
        }
      });
      ro.observe(host);
      map.set(it.key, { editor, line: it.line, zoneId, zone, host, ro });
      changed = true;
    }
    if (changed) rerender((n) => n + 1);
  }, [diff, items, editorFor]);

  // Tear down on editor change/unmount.
  useEffect(() => {
    const map = entries.current;
    return () => {
      for (const e of map.values()) e.ro.disconnect();
      map.clear();
    };
  }, [diff]);

  // Range tints.
  useEffect(() => {
    if (!diff) return;
    const all = selecting ? [...marks, selecting] : marks;
    const collections = (["RIGHT", "LEFT"] as Side[]).map((side) => {
      const ed = editorFor(side);
      if (!ed) return null;
      return ed.createDecorationsCollection(
        all
          .filter((m) => m.side === side)
          .map((m) => ({
            range: { startLineNumber: m.start, startColumn: 1, endLineNumber: m.end, endColumn: 1 },
            options: { isWholeLine: true, className: `rv-range rv-range-${m.kind}`, linesDecorationsClassName: `rv-range-bar rv-range-bar-${m.kind}` },
          })),
      );
    });
    return () => collections.forEach((c) => c?.clear());
  }, [diff, marks, selecting, editorFor]);

  // "+" on hover, click / drag / selection to start a comment.
  useEffect(() => {
    const monaco = monacoRef.current;
    if (!diff || !monaco) return;
    const T = monaco.editor.MouseTargetType;
    const disposers: { dispose(): void }[] = [];

    (["RIGHT", "LEFT"] as Side[]).forEach((side) => {
      const ed = editorFor(side);
      if (!ed) return;
      const plus = ed.createDecorationsCollection();
      let hover = 0;
      let dragFrom = 0;

      const canComment = (line: number) => !!hunkOf(latest.current.commentable, side, line);
      const clampToHunk = (from: number, to: number): [number, number] | null => {
        const hunk = hunkOf(latest.current.commentable, side, to);
        if (!hunk) return null;
        const a = Math.max(Math.min(from, to), hunk[0]);
        const b = Math.min(Math.max(from, to), hunk[1]);
        return [a, b];
      };
      const showPlus = (line: number) => {
        if (line === hover) return;
        hover = line;
        plus.set(
          line && canComment(line)
            ? [{ range: { startLineNumber: line, startColumn: 1, endLineNumber: line, endColumn: 1 }, options: { glyphMarginClassName: "rv-add-glyph" } }]
            : [],
        );
      };

      disposers.push(
        ed.onMouseMove((e) => {
          const line = e.target.position?.lineNumber ?? 0;
          const overZone = e.target.type === T.CONTENT_VIEW_ZONE || e.target.type === T.GUTTER_VIEW_ZONE;
          showPlus(overZone ? 0 : line);
          if (dragFrom && line) {
            const r = clampToHunk(dragFrom, line);
            if (r) setSelecting({ side, start: r[0], end: r[1], kind: "selecting" });
          }
        }),
        ed.onMouseLeave(() => showPlus(0)),
        ed.onMouseDown((e) => {
          const line = e.target.position?.lineNumber ?? 0;
          if (e.target.type !== T.GUTTER_GLYPH_MARGIN || !line || !canComment(line)) return;
          dragFrom = line;
          setSelecting({ side, start: line, end: line, kind: "selecting" });
        }),
        ed.onMouseUp((e) => {
          if (!dragFrom) return;
          const from = dragFrom;
          dragFrom = 0;
          setSelecting(null);
          const to = e.target.position?.lineNumber ?? from;
          let range = clampToHunk(from, to);
          // A click inside a multi-line selection comments on the selection.
          const sel = ed.getSelection();
          if (from === to && sel && sel.startLineNumber !== sel.endLineNumber && from >= sel.startLineNumber && from <= sel.endLineNumber) {
            const endLine = sel.endColumn === 1 ? sel.endLineNumber - 1 : sel.endLineNumber;
            range = clampToHunk(sel.startLineNumber, Math.max(sel.startLineNumber, endLine));
          }
          if (range) latest.current.onRequestComment(side, range[0], range[1]);
        }),
        { dispose: () => plus.clear() },
      );
    });
    return () => disposers.forEach((d) => d.dispose());
  }, [diff, editorFor]);

  const portals = items.map((it) => {
    const e = entries.current.get(it.key);
    return e ? createPortal(it.node, e.host, it.key) : null;
  });

  return { onMount, portals };
}
