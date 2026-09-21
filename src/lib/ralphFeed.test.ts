import { describe, expect, it } from "vitest";
import { applyEvent, formatDuration, groupByIteration } from "./ralphFeed";
import { RalphEvent, RunState } from "../types/ralph";

const run: RunState = {
  id: "1-web",
  repo: "web",
  running: true,
  startedAt: 0,
  finishedAt: null,
  reason: null,
  iteration: 0,
  currentStory: "US-001",
  passed: 0,
  total: 3,
  commits: 0,
  costUsd: 0,
  limitUntil: null,
  config: {} as RunState["config"],
  events: [],
};

const ev = (e: Partial<RalphEvent> & { kind: RalphEvent["kind"] }): RalphEvent => ({ ts: 1, ...e }) as RalphEvent;

describe("applyEvent", () => {
  it("tracks iterations, commits, cost and stories", () => {
    let r = applyEvent(run, ev({ kind: "iteration_start", n: 1, story: { id: "US-001", title: "A" } }));
    r = applyEvent(r, ev({ kind: "iteration_end", n: 1, commits: 2, costUsd: 0.5, passed: 1, total: 3 }));
    r = applyEvent(r, ev({ kind: "iteration_start", n: 2, story: { id: "US-002", title: "B" } }));
    r = applyEvent(r, ev({ kind: "iteration_end", n: 2, commits: 1, costUsd: 0.25, passed: 2, total: 3 }));
    expect(r.iteration).toBe(2);
    expect(r.currentStory).toBe("US-002");
    expect(r.commits).toBe(3);
    expect(r.costUsd).toBeCloseTo(0.75);
    expect(r.passed).toBe(2);
    expect(r.events).toHaveLength(4);
    expect(run.events).toHaveLength(0);
  });

  it("limit wait sets a countdown that the next iteration clears; stop ends the run", () => {
    let r = applyEvent(run, ev({ kind: "limit_wait", until: 999 }));
    expect(r.limitUntil).toBe(999);
    r = applyEvent(r, ev({ kind: "iteration_start", n: 1 }));
    expect(r.limitUntil).toBeNull();
    r = applyEvent(r, ev({ kind: "stopped", ts: 50, reason: { kind: "complete" } }));
    expect(r.running).toBe(false);
    expect(r.finishedAt).toBe(50);
    expect(r.reason).toEqual({ kind: "complete" });
  });
});

describe("groupByIteration", () => {
  it("groups the feed per iteration with its end event", () => {
    const groups = groupByIteration([
      ev({ kind: "run_start" }),
      ev({ kind: "iteration_start", n: 1, story: { id: "US-001", title: "A" } }),
      ev({ kind: "tool", name: "Edit", summary: "a.ts" }),
      ev({ kind: "iteration_end", n: 1, commits: 1 }),
      ev({ kind: "iteration_start", n: 2 }),
      ev({ kind: "text", text: "hi" }),
    ]);
    expect(groups.map((g) => g.n)).toEqual([0, 1, 2]);
    expect(groups[1].events.map((e) => e.kind)).toEqual(["tool"]);
    expect(groups[1].end?.commits).toBe(1);
    expect(groups[2].end).toBeNull();
  });
});

describe("formatDuration", () => {
  it("formats seconds, minutes and hours", () => {
    expect(formatDuration(42_000)).toBe("42s");
    expect(formatDuration(125_000)).toBe("2m 5s");
    expect(formatDuration(3_900_000)).toBe("1h 5m");
  });
});
