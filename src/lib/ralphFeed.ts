import { RalphEvent, RunState } from "../types/ralph";

const MAX_EVENTS = 600;

/** Applies a live `ralph-event` to the run snapshot (mirrors the backend's
 *  RunState updates so counters move without refetching). */
export function applyEvent(run: RunState, ev: RalphEvent): RunState {
  const events = [...run.events, ev].slice(-MAX_EVENTS);
  const next: RunState = { ...run, events };
  switch (ev.kind) {
    case "iteration_start":
      next.iteration = ev.n;
      next.currentStory = ev.story?.id ?? null;
      next.limitUntil = null;
      break;
    case "iteration_end":
      next.commits += ev.commits ?? 0;
      next.costUsd += ev.costUsd ?? 0;
      next.passed = ev.passed ?? next.passed;
      next.total = ev.total ?? next.total;
      break;
    case "limit_wait":
      next.limitUntil = ev.until;
      break;
    case "stopped":
      next.running = false;
      next.finishedAt = ev.ts;
      next.reason = ev.reason;
      next.limitUntil = null;
      break;
  }
  return next;
}

export interface IterationGroup {
  n: number;
  story: { id: string; title: string } | null;
  events: RalphEvent[];
  end: RalphEvent | null;
}

/** Splits the flat feed into one group per iteration (events before the
 *  first iteration go to group 0). */
export function groupByIteration(events: RalphEvent[]): IterationGroup[] {
  const groups: IterationGroup[] = [];
  let cur: IterationGroup = { n: 0, story: null, events: [], end: null };
  for (const ev of events) {
    if (ev.kind === "iteration_start") {
      if (cur.events.length || cur.n > 0) groups.push(cur);
      cur = { n: ev.n, story: ev.story ?? null, events: [], end: null };
    } else if (ev.kind === "iteration_end") {
      cur.end = ev;
    } else {
      cur.events.push(ev);
    }
  }
  if (cur.events.length || cur.n > 0) groups.push(cur);
  return groups;
}

export function formatDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${s % 60}s`;
  return `${Math.floor(m / 60)}h ${m % 60}m`;
}
