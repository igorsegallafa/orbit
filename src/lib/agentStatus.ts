// Live agent status derived from raw terminal output. claude/opencode render
// spinner lines like "✽ Thinking…", "Running tests…", "Editing file.rs" —
// we match those verb patterns on each output chunk and remember the most
// recent one. Best-effort by design: agents change wording between versions,
// so matches are fuzzy and fall back to "idle"/"busy".
export type AgentStatus = "idle" | "busy" | "thinking" | "running" | "editing" | "waiting" | "exited";

/** Lifecycle reported by the agent itself (Claude Code hooks), when available. */
export type HookState = "working" | "waiting" | "done";

interface Pattern {
  status: Extract<AgentStatus, "thinking" | "running" | "editing">;
  // Matches the verb printed by the spinner, case-insensitive.
  regex: RegExp;
}

const PATTERNS: Pattern[] = [
  { status: "thinking", regex: /\b(thinking|pondering|reflecting|analyz(ing|e)|considering|planning)\b/i },
  { status: "running", regex: /\b(running|executing|testing|building|installing|compiling|linting|fetch(ing)?)\b/i },
  { status: "editing", regex: /\b(editing|writing|updating|patching|creating|modifying|refactoring)\b/i },
];

// Chunks arriving within this window mark the session as active.
const ACTIVITY_WINDOW_MS = 2500;

// A matched verb only counts while fresh: agents keep streaming output
// (answers, redraws) long after the "✽ Thinking…" spinner line scrolled
// away. Without this, the last verb sticks forever and the spinner never
// stops even when the agent is done.
const VERB_WINDOW_MS = 4000;

export class AgentStatusTracker {
  private lastStatus: AgentStatus = "idle";
  private lastVerbAt = 0;
  private lastActivity = 0;
  private exited = false;
  private hook: HookState | null = null;

  /** Authoritative state from agent hooks; overrides the output heuristics. */
  setHook(state: HookState): void {
    this.hook = state;
  }

  hasHooks(): boolean {
    return this.hook !== null;
  }

  feed(data: string): void {
    // Scan the last few lines of the chunk for spinner verbs.
    const lines = data.split(/[\r\n]+/).filter(Boolean).slice(-8);
    for (const line of lines) {
      // Strip ANSI escapes so escape codes don't break the word matching.
      const clean = line.replace(/\x1b\[[0-9;?]*[A-Za-z]/g, "");
      for (const p of PATTERNS) {
        if (p.regex.test(clean)) {
          this.lastStatus = p.status;
          this.lastVerbAt = Date.now();
          break;
        }
      }
    }
    this.lastActivity = Date.now();
  }

  markExited(): void {
    this.exited = true;
  }

  status(): AgentStatus {
    if (this.exited) return "exited";
    if (this.hook === "waiting") return "waiting";
    const now = Date.now();
    // Long silent thinking still counts as working when the agent says so.
    if (now - this.lastActivity > ACTIVITY_WINDOW_MS) return this.hook === "working" ? "busy" : "idle";
    // No fresh verb: output is flowing (answers, redraws) but the agent is
    // not visibly "thinking/running/editing" — show generic busy instead of
    // a stale spinner.
    if (now - this.lastVerbAt > VERB_WINDOW_MS) return "busy";
    return this.lastStatus;
  }
}