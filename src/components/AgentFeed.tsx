import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { TOOL_KIND, inlineMarkdown } from "./RalphView";

/** One step of a live agent run (agent::live's `agent-feed` event). */
export interface FeedItem {
  kind: "tool" | "text" | "thinking" | "log";
  name?: string;
  text: string;
}

/** Steps of the live run `run`, from when it's set; cleared when it changes. */
export function useAgentFeed(run: string | null): FeedItem[] {
  const [items, setItems] = useState<FeedItem[]>([]);
  useEffect(() => {
    setItems([]);
    if (!run) return;
    const off = listen<{ run: string } & FeedItem>("agent-feed", (e) => {
      if (e.payload.run !== run) return;
      const { kind, name, text } = e.payload;
      setItems((all) => [...all.slice(-300), { kind, name, text }]);
    });
    return () => {
      off.then((f) => f());
    };
  }, [run]);
  return items;
}

function elapsed(secs: number): string {
  const m = Math.floor(secs / 60);
  return m > 0 ? `${m}m ${String(secs % 60).padStart(2, "0")}s` : `${secs}s`;
}

const VERB: Record<string, string> = {
  read: "Reading",
  search: "Searching",
  edit: "Writing",
  run: "Running",
};

/**
 * What an agent is doing, live: a status line (current step + elapsed
 * time) over the list of its steps — files read, searches, commands, notes.
 * `waiting` shows before the first step arrives.
 */
export function AgentFeed({ items, startedAt, waiting = "Starting the agent…" }: { items: FeedItem[]; startedAt: number; waiting?: string }) {
  const [now, setNow] = useState(Date.now());
  const listRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);

  // Follow new steps unless the user scrolled up to read.
  useEffect(() => {
    const el = listRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [items]);

  // Structured replies (the interview's JSON) aren't notes for the reader.
  const shown = items.filter((i) => !(i.kind === "text" && /^\s*(\{|```json)/.test(i.text)));
  const lastTool = [...items].reverse().find((i) => i.kind === "tool");
  const tools = items.filter((i) => i.kind === "tool").length;
  const last = items[items.length - 1];
  const status =
    last?.kind === "thinking"
      ? "Thinking…"
      : lastTool
        ? `${VERB[TOOL_KIND[lastTool.name ?? ""] ?? ""] ?? lastTool.name} ${lastTool.text}`.trim()
        : items.length
          ? "Thinking…"
          : waiting;

  return (
    <div className="agent-feed">
      <div className="agent-feed-status">
        <span className="spinner" />
        <span className="agent-feed-now" title={status}>
          {status}
        </span>
        <span className="agent-feed-meta">
          {tools > 0 && `${tools} step${tools === 1 ? "" : "s"} · `}
          {elapsed(Math.max(0, Math.floor((now - startedAt) / 1000)))}
        </span>
      </div>
      <div
        className="agent-feed-list"
        ref={listRef}
        onScroll={(e) => {
          const el = e.currentTarget;
          stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
        }}
      >
        {shown.length === 0 ? (
          <div className="agent-feed-empty">The agent's steps (files it reads, searches, commands) show up here as it works.</div>
        ) : (
          shown.map((it, i) =>
            it.kind === "tool" ? (
              <div key={i} className="rv-line rv-tool">
                <span className={`rv-chip rv-chip-${TOOL_KIND[it.name ?? ""] ?? "other"}`}>{it.name}</span>
                <span className="rv-tool-summary" title={it.text}>
                  {it.text}
                </span>
              </div>
            ) : it.kind === "thinking" ? (
              <div key={i} className="rv-line agent-feed-thinking" title={it.text}>
                {it.text.replace(/\s+/g, " ")}
              </div>
            ) : it.kind === "text" ? (
              <div key={i} className="rv-line rv-text">
                {inlineMarkdown(it.text)}
              </div>
            ) : (
              <div key={i} className="rv-line rv-log">
                {it.text}
              </div>
            )
          )
        )}
      </div>
    </div>
  );
}
