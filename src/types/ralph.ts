export interface Story {
  id: string;
  title: string;
  description: string;
  acceptanceCriteria: string[];
  priority: number;
  passes: boolean;
  notes: string;
  [extra: string]: unknown;
}

export interface Prd {
  project: string;
  branchName: string;
  description: string;
  userStories: Story[];
  [extra: string]: unknown;
}

export interface RunConfig {
  maxIterations: number;
  untilComplete: boolean;
  waitOnLimit: boolean;
  limitWaitMin: number;
  maxHours: number;
  stallAfter: number;
  stopAfterStory?: string | null;
  agent?: string | null;
  model?: string | null;
  push: boolean;
  iterationTimeoutMin: number;
  extraInstructions: string;
}

export type StopReason =
  | { kind: "complete" | "maxIterations" | "stalled" | "limit" | "deadline" | "stopped" | "paused" }
  | { kind: "storyReached"; detail: string }
  | { kind: "failed"; detail: string };

/** One entry of the live feed (`ralph-event`). */
export interface RalphEvent {
  kind:
    | "run_start"
    | "iteration_start"
    | "text"
    | "tool"
    | "log"
    | "story_passed"
    | "iteration_end"
    | "push"
    | "limit_wait"
    | "stopped";
  ts: number;
  [field: string]: any;
}

export interface RunState {
  id: string;
  repo: string;
  running: boolean;
  startedAt: number;
  finishedAt: number | null;
  reason: StopReason | null;
  iteration: number;
  currentStory: string | null;
  passed: number;
  total: number;
  commits: number;
  costUsd: number;
  limitUntil: number | null;
  config: RunConfig;
  events: RalphEvent[];
}

export interface RalphState {
  prd: Prd | null;
  progress: string;
  run: RunState | null;
  prompt: string;
  promptCustom: boolean;
  defaultConfig: RunConfig;
}

export function reasonLabel(r: StopReason | null): string {
  if (!r) return "";
  switch (r.kind) {
    case "complete":
      return "All stories pass";
    case "storyReached":
      return `Reached ${r.detail}`;
    case "maxIterations":
      return "Iteration limit reached";
    case "stalled":
      return "Stalled: no commits in the last iterations";
    case "limit":
      return "Stopped on usage limit";
    case "deadline":
      return "Time budget used";
    case "failed":
      return `Failed: ${r.detail}`;
    case "stopped":
      return "Stopped";
    case "paused":
      return "Paused";
  }
}
