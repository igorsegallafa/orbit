// The debug session (one at a time, as in Rider): breakpoints, launching
// a run configuration under the debugger, where it stopped (threads,
// frames, variables) and the debug console. Shared by the run widget, the
// bottom panel and the editor gutter.
import { useSyncExternalStore } from "react";
import { DapClient } from "./client";
import { comparablePath, rebase } from "../lsp/paths";
import { RunSetup, RunTarget, build, showPanel } from "../runStore";

/* eslint-disable @typescript-eslint/no-explicit-any */

const IS_WINDOWS = navigator.userAgent.includes("Windows");

export interface Frame {
  id: number;
  name: string;
  line: number;
  column: number;
  /** The file as the editor knows it (workspace path), or as reported. */
  file: string | null;
  /** The repo file, when it is one (opens as a tab). */
  open: { workspace: string; repo: string; path: string } | null;
}

export interface Variable {
  name: string;
  value: string;
  type?: string;
  /** > 0: has children, loaded with `loadVariables`. */
  ref: number;
}

export interface ConsoleLine {
  kind: "out" | "err" | "info" | "input" | "result";
  text: string;
}

export type DebugStatus = "idle" | "starting" | "running" | "stopped" | "ended";

interface State {
  status: DebugStatus;
  target: RunTarget | null;
  threads: { id: number; name: string }[];
  threadId: number | null;
  frames: Frame[];
  frameId: number | null;
  scopes: { name: string; ref: number }[];
  /** Children by variablesReference (scopes and expanded variables). */
  vars: Record<number, Variable[]>;
  console: ConsoleLine[];
  stopReason: string | null;
  /** Breakpoint lines by file (the path editors use). */
  breakpoints: Record<string, number[]>;
}

const BP_KEY = "orbit.breakpoints";
const MAX_CONSOLE = 3000;

function loadBreakpoints(): Record<string, number[]> {
  try {
    return JSON.parse(localStorage.getItem(BP_KEY) ?? "{}");
  } catch {
    return {};
  }
}

let state: State = {
  status: "idle",
  target: null,
  threads: [],
  threadId: null,
  frames: [],
  frameId: null,
  scopes: [],
  vars: {},
  console: [],
  stopReason: null,
  breakpoints: loadBreakpoints(),
};
const listeners = new Set<() => void>();

function set(next: Partial<State>) {
  state = { ...state, ...next };
  listeners.forEach((l) => l());
}

export function getDebugState(): State {
  return state;
}

export function subscribeDebug(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useDebugState(): State {
  return useSyncExternalStore(subscribeDebug, () => state);
}

function log(kind: ConsoleLine["kind"], text: string) {
  const lines = state.console.concat(text.replace(/\n$/, "").split("\n").map((t) => ({ kind, text: t })));
  set({ console: lines.length > MAX_CONSOLE ? lines.slice(lines.length - MAX_CONSOLE) : lines });
}

// ---------- breakpoints ----------

const sameFile = (a: string, b: string) => comparablePath(a, IS_WINDOWS) === comparablePath(b, IS_WINDOWS);

export function breakpointsIn(file: string): number[] {
  const key = Object.keys(state.breakpoints).find((k) => sameFile(k, file));
  return key ? state.breakpoints[key] : [];
}

export function toggleBreakpoint(file: string, line: number) {
  const key = Object.keys(state.breakpoints).find((k) => sameFile(k, file)) ?? file;
  const lines = state.breakpoints[key] ?? [];
  const next = lines.includes(line) ? lines.filter((l) => l !== line) : [...lines, line].sort((a, b) => a - b);
  const all = { ...state.breakpoints };
  if (next.length) all[key] = next;
  else delete all[key];
  set({ breakpoints: all });
  try {
    localStorage.setItem(BP_KEY, JSON.stringify(all));
  } catch {
    // Kept for this run only.
  }
  if (client && (state.status === "running" || state.status === "stopped")) void sendBreakpoints(key);
}

// ---------- session ----------

let client: DapClient | null = null;
let roots: { real: string; linked: string } | null = null;
/** Opens a repo file at a line (set by App). */
let opener: ((ctx: NonNullable<Frame["open"]>, line: number, column: number) => void) | null = null;

export function setDebugOpener(fn: typeof opener) {
  opener = fn;
}

/** Editor path → the path the debugger knows (links resolved). */
function toDebugger(path: string): string {
  return (roots && rebase(path, roots.linked, roots.real, IS_WINDOWS)) ?? path;
}

function toEditor(path: string): string {
  return (roots && rebase(path, roots.real, roots.linked, IS_WINDOWS)) ?? path;
}

async function sendBreakpoints(file: string) {
  if (!client) return;
  const lines = state.breakpoints[file] ?? [];
  await client
    .request("setBreakpoints", { source: { path: toDebugger(file) }, breakpoints: lines.map((line) => ({ line })) })
    .catch((e) => log("err", `Breakpoints in ${file}: ${e.message}`));
}

/** Files under the session's repo (breakpoints elsewhere belong to others). */
function sessionFiles(): string[] {
  if (!roots) return [];
  return Object.keys(state.breakpoints).filter((f) => rebase(f, roots!.linked, "", IS_WINDOWS) !== null);
}

export function isDebugging(): boolean {
  return state.status === "starting" || state.status === "running" || state.status === "stopped";
}

/** Builds the configuration, then runs it under the debugger. */
export async function startDebugging(target: RunTarget, setup: RunSetup) {
  if (isDebugging()) return;
  const config = setup.configs.find((c) => c.name === target.name);
  if (!config?.program) return;
  set({ status: "starting", target, threads: [], threadId: null, frames: [], frameId: null, scopes: [], vars: {}, console: [], stopReason: null });
  if (!(await build(target, setup))) {
    set({ status: "idle" });
    return;
  }
  showPanel("debug");
  roots = { real: setup.root, linked: setup.linkedRoot };
  let c: DapClient;
  try {
    c = await DapClient.start(config.debugger || null, config.cwd);
  } catch (e) {
    log("err", String(e));
    set({ status: "ended" });
    return;
  }
  client = c;
  log("info", `${c.adapter.command} · ${config.program}`);

  c.on("output", (b) => b.category !== "telemetry" && log(b.category === "stderr" ? "err" : b.category === "console" ? "info" : "out", b.output ?? ""));
  c.on("stopped", (b) => void onStopped(b));
  c.on("continued", () => set({ status: "running", frames: [], frameId: null, scopes: [], vars: {}, stopReason: null }));
  c.on("exited", (b) => log("info", `Process exited with code ${b.exitCode}`));
  c.on("terminated", () => end());
  c.on("$exit", () => end());

  const initialized = new Promise<void>((resolve) => c.on("initialized", () => resolve()));
  try {
    await c.request("initialize", {
      clientID: "orbit",
      clientName: "Orbit",
      adapterID: c.adapter.kind,
      linesStartAt1: true,
      columnsStartAt1: true,
      pathFormat: "path",
      supportsVariableType: true,
    });
    const env = c.adapter.kind === "gdb" ? config.env : Object.entries(config.env).map(([k, v]) => `${k}=${v}`);
    // gdb answers launch only after configurationDone: don't wait for it here.
    const launched = c.request("launch", { program: config.program, args: config.args, cwd: config.cwd, env });
    launched.catch((e) => {
      log("err", `Launch failed: ${e.message}`);
      stopDebugging();
    });
    await initialized;
    await Promise.all(sessionFiles().map(sendBreakpoints));
    await c.request("configurationDone");
    await launched;
    if (state.status === "starting") set({ status: "running" });
  } catch (e) {
    log("err", String((e as Error).message ?? e));
    stopDebugging();
  }
}

function end() {
  if (state.status === "ended" || state.status === "idle") return;
  client?.kill();
  client = null;
  set({ status: "ended", frames: [], frameId: null, scopes: [], vars: {}, threads: [] });
  log("info", "Debug session ended");
}

async function onStopped(body: any) {
  if (!client) return;
  const threads: { id: number; name: string }[] = (await client.request("threads").catch(() => ({ threads: [] }))).threads ?? [];
  const threadId = body.threadId ?? threads[0]?.id ?? null;
  set({ status: "stopped", threads, threadId, stopReason: body.description ?? body.reason ?? null });
  if (threadId !== null) await selectThread(threadId, true);
}

export async function selectThread(threadId: number, reveal = false) {
  if (!client) return;
  const r = await client.request("stackTrace", { threadId, startFrame: 0, levels: 60 }).catch(() => ({ stackFrames: [] }));
  const frames: Frame[] = (r.stackFrames ?? []).map((f: any) => {
    const file = f.source?.path ? toEditor(f.source.path) : null;
    const rel = file && roots ? rebase(file, roots.linked, "", IS_WINDOWS) : null;
    const target = state.target;
    return {
      id: f.id,
      name: f.name,
      line: f.line,
      column: f.column || 1,
      file,
      open: rel !== null && target ? { workspace: target.workspace, repo: target.repo, path: rel.replace(/^[\\/]+/, "").replace(/\\/g, "/") } : null,
    };
  });
  set({ threadId, frames });
  // The innermost frame with source in the repo is where you are.
  const top = frames.find((f) => f.open) ?? frames[0];
  if (top) await selectFrame(top.id, reveal);
}

export async function selectFrame(frameId: number, reveal = true) {
  if (!client) return;
  set({ frameId, scopes: [], vars: {} });
  const frame = state.frames.find((f) => f.id === frameId);
  if (reveal && frame?.open) opener?.(frame.open, frame.line, frame.column);
  const r = await client.request("scopes", { frameId }).catch(() => ({ scopes: [] }));
  const scopes = (r.scopes ?? []).map((s: any) => ({ name: s.name, ref: s.variablesReference, expensive: s.expensive }));
  set({ scopes: scopes.map(({ name, ref }: { name: string; ref: number }) => ({ name, ref })) });
  await Promise.all(scopes.filter((s: any) => !s.expensive).map((s: any) => loadVariables(s.ref)));
}

export async function loadVariables(ref: number) {
  if (!client || ref <= 0 || state.vars[ref]) return;
  const r = await client.request("variables", { variablesReference: ref }).catch(() => ({ variables: [] }));
  const list: Variable[] = (r.variables ?? []).map((v: any) => ({ name: v.name, value: v.value, type: v.type, ref: v.variablesReference ?? 0 }));
  set({ vars: { ...state.vars, [ref]: list } });
}

export async function evaluate(expression: string) {
  if (!client || !expression.trim()) return;
  log("input", expression);
  try {
    const r = await client.request("evaluate", { expression, frameId: state.frameId ?? undefined, context: "repl" });
    log("result", r.result ?? "");
  } catch (e) {
    log("err", (e as Error).message);
  }
}

const step = (command: string) => () => {
  if (!client || state.threadId === null) return;
  void client.request(command, { threadId: state.threadId }).catch((e) => log("err", e.message));
};

export const resume = step("continue");
export const stepOver = step("next");
export const stepInto = step("stepIn");
export const stepOut = step("stepOut");

export function pause() {
  if (!client || state.status !== "running") return;
  const threadId = state.threadId ?? state.threads[0]?.id ?? 1;
  void client.request("pause", { threadId }).catch((e) => log("err", e.message));
}

/** Ends the session and the program. */
export function stopDebugging() {
  const c = client;
  if (!c) {
    if (state.status === "starting") set({ status: "idle" });
    return;
  }
  void c.request("disconnect", { terminateDebuggee: true }).catch(() => null);
  // Adapters that hang on disconnect are killed.
  window.setTimeout(() => c.alive && c.kill(), 1500);
}
