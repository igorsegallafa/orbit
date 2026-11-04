// Build / Run state shared by the title bar widget and the bottom panel:
// the selected run configuration per workspace, the current build (output,
// problems) and the running program's terminal. One build and one run at a
// time, as in Rider.
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import * as monaco from "monaco-editor";
import type { TerminalTab } from "../components/TerminalPane";
import { interruptTerminal } from "../components/TerminalPane";
import { comparablePath, rebase } from "./lsp/paths";
import { Problem, parseProblem, problemKey } from "./problems";

export interface ResolvedRun {
  name: string;
  kind: string;
  preset: string | null;
  buildDir: string | null;
  buildLabel: string;
  program: string | null;
  args: string[];
  cwd: string;
  env: Record<string, string>;
  /** Debug adapter ("auto", "gdb", "lldb-dap" or a path). */
  debugger: string;
}

export interface RunSetup {
  root: string;
  linkedRoot: string;
  configs: ResolvedRun[];
  presets: string[];
}

/** A run configuration of a repo in a workspace. */
export interface RunTarget {
  workspace: string;
  repo: string;
  name: string;
  /** Chosen in the widget; null = the configuration's own or automatic. */
  preset: string | null;
}

export interface OutputLine {
  stream: "out" | "err" | "info";
  text: string;
  /** The problem this line reports (the panel makes it a link). */
  problem?: BuildProblem;
}

/** A problem with the editor tab it belongs to, when it's a repo file. */
export interface BuildProblem extends Problem {
  open: { workspace: string; repo: string; path: string } | null;
}

export type BuildStatus = "idle" | "running" | "ok" | "failed" | "cancelled";
export type PanelView = "build" | "run" | "problems" | "debug";

interface State {
  build: { key: string; target: RunTarget; status: BuildStatus; lines: OutputLine[]; problems: BuildProblem[]; startedAt: number } | null;
  run: { tab: TerminalTab; target: RunTarget; exited: boolean } | null;
  panel: { open: boolean; view: PanelView };
}

const MAX_LINES = 5000;
const IS_WINDOWS = navigator.userAgent.includes("Windows");

let state: State = { build: null, run: null, panel: { open: false, view: "build" } };
const listeners = new Set<() => void>();

function set(next: Partial<State>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

export function useRunState(): State {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
  );
}

export const runKey = (t: RunTarget) => `${t.workspace}/${t.repo}/${t.name}`;

// ---------- selection (remembered per workspace) ----------

const SELECTION_KEY = "orbit.run.selection";

export function savedTarget(workspace: string): RunTarget | null {
  try {
    const all = JSON.parse(localStorage.getItem(SELECTION_KEY) ?? "{}");
    return all[workspace] ?? null;
  } catch {
    return null;
  }
}

export function saveTarget(t: RunTarget) {
  try {
    const all = JSON.parse(localStorage.getItem(SELECTION_KEY) ?? "{}");
    all[t.workspace] = t;
    localStorage.setItem(SELECTION_KEY, JSON.stringify(all));
  } catch {
    // Not remembered: the widget picks the first configuration next time.
  }
}

export function loadSetup(workspace: string, repo: string, preset: string | null): Promise<RunSetup> {
  return invoke<RunSetup>("run_setup", { workspace, repo, preset });
}

// ---------- panel ----------

export function showPanel(view: PanelView) {
  set({ panel: { open: true, view } });
}

export function togglePanel() {
  set({ panel: { ...state.panel, open: !state.panel.open } });
}

// ---------- build ----------

let pending: { key: string; lines: OutputLine[]; problems: Map<string, BuildProblem> } | null = null;
let flushTimer: number | null = null;
let setupFor: RunSetup | null = null;

/** Output arrives line by line; the panel redraws at most every 100 ms. */
function flush() {
  flushTimer = null;
  if (!pending || !state.build || state.build.key !== pending.key) return;
  const lines = state.build.lines.concat(pending.lines);
  pending.lines = [];
  set({
    build: {
      ...state.build,
      lines: lines.length > MAX_LINES ? lines.slice(lines.length - MAX_LINES) : lines,
      problems: [...pending.problems.values()],
    },
  });
  applyMarkers();
}

let listening: Promise<unknown> | null = null;
function ensureListening() {
  listening ??= listen<{ key: string; stream: OutputLine["stream"]; line: string }>("run-output", (e) => {
    if (!pending || e.payload.key !== pending.key) return;
    const line: OutputLine = { stream: e.payload.stream, text: e.payload.line };
    if (line.stream !== "info") {
      const p = parseProblem(line.text, state.build?.target ? buildDirOf(state.build.target) : null);
      if (p) {
        const k = problemKey(p);
        if (!pending.problems.has(k)) pending.problems.set(k, { ...p, open: openTarget(p) });
        line.problem = pending.problems.get(k);
      }
    }
    pending.lines.push(line);
    flushTimer ??= window.setTimeout(flush, 100);
  });
  return listening;
}

function configOf(t: RunTarget): ResolvedRun | undefined {
  return setupFor?.configs.find((c) => c.name === t.name);
}

function buildDirOf(t: RunTarget): string | null {
  return configOf(t)?.buildDir ?? null;
}

/** The repo file a problem points at, as an editor tab. */
function openTarget(p: Problem): BuildProblem["open"] {
  const b = state.build;
  if (!p.file || !b || !setupFor) return null;
  const rel = rebase(p.file, setupFor.root, "", IS_WINDOWS);
  if (rel === null) return null;
  return { workspace: b.target.workspace, repo: b.target.repo, path: rel.replace(/^[\\/]+/, "").replace(/\\/g, "/") };
}

export function isBusy(): boolean {
  return state.build?.status === "running";
}

/** Builds a configuration; resolves true when it succeeded. */
export async function build(target: RunTarget, setup: RunSetup): Promise<boolean> {
  if (isBusy()) return false;
  await ensureListening();
  setupFor = setup;
  const key = runKey(target);
  pending = { key, lines: [], problems: new Map() };
  set({ build: { key, target, status: "running", lines: [], problems: [], startedAt: Date.now() }, panel: { open: true, view: "build" } });
  applyMarkers();
  let status: BuildStatus;
  try {
    const exit = await invoke<{ success: boolean; cancelled: boolean }>("run_build", {
      workspace: target.workspace,
      repo: target.repo,
      name: target.name,
      preset: target.preset,
      key,
    });
    status = exit.cancelled ? "cancelled" : exit.success ? "ok" : "failed";
  } catch (e) {
    pending.lines.push({ stream: "err", text: String(e) });
    status = "failed";
  }
  // Let the last events land before the final redraw.
  await new Promise((r) => window.setTimeout(r, 150));
  flush();
  if (state.build?.key === key) set({ build: { ...state.build, status } });
  if (status === "failed" && (state.build?.problems.length ?? 0) > 0) showPanel("problems");
  return status === "ok";
}

/** Builds, then starts the program in the panel's Run terminal. */
export async function buildAndRun(target: RunTarget, setup: RunSetup) {
  const config = setup.configs.find((c) => c.name === target.name);
  if (!config?.program) {
    await build(target, setup);
    return;
  }
  if (state.run && !state.run.exited) stop();
  if (!(await build(target, setup))) return;
  set({
    run: {
      target,
      exited: false,
      tab: {
        id: `run:${Date.now()}`,
        workspace: target.workspace,
        label: config.name,
        sessionName: config.name,
        cmd: config.program,
        args: config.args,
        cwd: config.cwd,
        env: config.env,
      },
    },
    panel: { open: true, view: "run" },
  });
}

export function runExited() {
  if (state.run && !state.run.exited) set({ run: { ...state.run, exited: true } });
}

/** Stops the build, or the running program (Ctrl+C). */
export function stop() {
  if (state.build?.status === "running") {
    void invoke("run_cancel", { key: state.build.key });
    return;
  }
  if (state.run && !state.run.exited) interruptTerminal(state.run.tab.id);
}

export function clearRun() {
  set({ run: null });
}

// ---------- editor markers ----------

const OWNER = "build";

function problemUri(p: BuildProblem): string | null {
  if (!p.open || !setupFor) return null;
  const sep = setupFor.linkedRoot.includes("\\") ? "\\" : "/";
  return monaco.Uri.file(`${setupFor.linkedRoot}${sep}${p.open.path.split("/").join(sep)}`).toString();
}

/** Build problems as squiggles in open editors (Monaco owner "build"). */
function applyMarkers(only?: monaco.editor.ITextModel) {
  const byUri = new Map<string, BuildProblem[]>();
  for (const p of state.build?.problems ?? []) {
    const uri = problemUri(p);
    if (uri) byUri.set(comparablePath(uri, IS_WINDOWS), [...(byUri.get(comparablePath(uri, IS_WINDOWS)) ?? []), p]);
  }
  for (const model of only ? [only] : monaco.editor.getModels()) {
    const list = byUri.get(comparablePath(model.uri.toString(), IS_WINDOWS)) ?? [];
    monaco.editor.setModelMarkers(
      model,
      OWNER,
      list.map((p) => {
        const line = Math.min(Math.max(p.line, 1), model.getLineCount());
        return {
          severity: p.severity === "error" ? monaco.MarkerSeverity.Error : monaco.MarkerSeverity.Warning,
          message: p.message,
          code: p.code,
          source: "build",
          startLineNumber: line,
          startColumn: p.column || 1,
          endLineNumber: line,
          endColumn: model.getLineMaxColumn(line),
        };
      }),
    );
  }
}

monaco.editor.onDidCreateModel((m) => applyMarkers(m));
