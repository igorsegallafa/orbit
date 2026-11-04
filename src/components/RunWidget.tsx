// Rider-style run widget for the title bar: pick a run configuration (and
// CMake preset), then Build (Ctrl+F9), Run (Shift+F10) or Stop (Ctrl+F2).
// Configurations come from each repo's .orbit/run.json.
import { useEffect, useMemo, useRef, useState } from "react";
import { Workspace } from "../types/config";
import { BugIcon, HammerIcon, PlayIcon, StopIcon } from "./Icons";
import { isDebugging, pause, resume, startDebugging, stepInto, stepOut, stepOver, stopDebugging, useDebugState } from "../lib/dap/session";
import { Select } from "./Select";
import { tooltip } from "./Tooltip";
import { useCommands } from "../lib/commands";
import {
  RunSetup,
  RunTarget,
  build,
  buildAndRun,
  loadSetup,
  saveTarget,
  savedTarget,
  stop,
  useRunState,
} from "../lib/runStore";

interface Props {
  workspace: Workspace | null;
}

export function RunWidget({ workspace }: Props) {
  const state = useRunState();
  const debug = useDebugState();
  // Every repo of the workspace that has run configurations.
  const [setups, setSetups] = useState<Record<string, RunSetup>>({});
  const [target, setTarget] = useState<RunTarget | null>(null);
  const wsName = workspace?.name ?? null;
  const repos = workspace?.repos.join("|") ?? "";

  const reload = async (preferred: RunTarget | null) => {
    if (!workspace) return;
    const found: Record<string, RunSetup> = {};
    await Promise.all(
      workspace.repos.map(async (repo) => {
        const preset = preferred?.repo === repo ? preferred.preset : null;
        const setup = await loadSetup(workspace.name, repo, preset).catch(() => null);
        if (setup?.configs.length) found[repo] = setup;
      }),
    );
    setSetups(found);
    const valid = (t: RunTarget | null) => !!t && !!found[t.repo]?.configs.some((c) => c.name === t.name);
    if (valid(preferred)) setTarget(preferred);
    else {
      const repo = Object.keys(found)[0];
      setTarget(repo ? { workspace: workspace.name, repo, name: found[repo].configs[0].name, preset: null } : null);
    }
  };

  useEffect(() => {
    setSetups({});
    setTarget(null);
    if (wsName) void reload(savedTarget(wsName));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsName, repos]);

  const setup = target ? setups[target.repo] : undefined;
  const config = setup?.configs.find((c) => c.name === target?.name);
  const building = state.build?.status === "running";
  const running = !!state.run && !state.run.exited;

  const options = useMemo(() => {
    const multi = Object.keys(setups).length > 1;
    return Object.entries(setups).flatMap(([repo, s]) =>
      s.configs.map((c) => ({ value: `${repo}\u0000${c.name}`, label: multi ? `${repo} · ${c.name}` : c.name })),
    );
  }, [setups]);

  const choose = (next: RunTarget) => {
    saveTarget(next);
    if (next.repo !== target?.repo || next.preset !== target?.preset) void reload(next);
    else setTarget(next);
  };

  // Shortcuts (JetBrains): Ctrl+F9 build, Shift+F10 run, Ctrl+F2 stop.
  const actions = useRef({ build: () => {}, run: () => {}, debug: () => {} });
  actions.current = {
    build: () => target && setup && void build(target, setup),
    run: () => target && setup && void buildAndRun(target, setup),
    debug: () => target && setup && void startDebugging(target, setup),
  };
  const debugRef = useRef(debug.status);
  debugRef.current = debug.status;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      const paused = isDebugging() && debugRef.current === "stopped";
      if (e.key === "F9" && ctrl && !e.shiftKey) actions.current.build();
      else if (e.key === "F10" && e.shiftKey && !ctrl) actions.current.run();
      else if (e.key === "F9" && e.shiftKey && !ctrl) actions.current.debug();
      else if (e.key === "F2" && ctrl) {
        if (isDebugging()) stopDebugging();
        else stop();
      }
      // While paused in the debugger (JetBrains keys).
      else if (paused && e.key === "F9" && !ctrl && !e.shiftKey) resume();
      else if (paused && e.key === "F8" && !ctrl && !e.shiftKey) stepOver();
      else if (paused && e.key === "F7" && !ctrl && !e.shiftKey) stepInto();
      else if (paused && e.key === "F8" && e.shiftKey && !ctrl) stepOut();
      else return;
      e.preventDefault();
      e.stopPropagation();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, []);

  const ready = () => !!target && !!setup && !!config;
  useCommands([
    { id: "run.build", title: `Build${config ? ` '${config.name}'` : ""}`, keys: "Ctrl+F9", enabled: () => ready() && !building, run: () => actions.current.build() },
    { id: "run.run", title: `Run${config ? ` '${config.name}'` : ""}`, keys: "Shift+F10", enabled: () => ready() && !!config?.program && !building, run: () => actions.current.run() },
    { id: "run.debug", title: `Debug${config ? ` '${config.name}'` : ""}`, keys: "Shift+F9", enabled: () => ready() && !!config?.program && !building && !isDebugging(), run: () => actions.current.debug() },
    { id: "run.stop", title: "Stop", keys: "Ctrl+F2", enabled: () => building || running || isDebugging(), run: () => (isDebugging() ? stopDebugging() : stop()) },
    { id: "debug.pause", title: "Pause Program", enabled: () => debug.status === "running", run: pause },
  ]);

  if (!workspace || !target || !setup || !config) return null;
  const hint = (text: string) => ({
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  });

  return (
    <div className="run-widget" onMouseDown={(e) => e.stopPropagation()}>
      <Select
        className="run-select"
        value={`${target.repo}\u0000${target.name}`}
        options={options}
        onChange={(v) => {
          const [repo, name] = v.split("\u0000");
          choose({ workspace: workspace.name, repo, name, preset: repo === target.repo ? target.preset : null });
        }}
        listWidth={240}
      />
      {setup.presets.length > 0 && config.kind === "cmake" && (
        <Select
          className="run-select run-preset"
          value={config.preset ?? ""}
          options={setup.presets.map((p) => ({ value: p, label: p }))}
          onChange={(p) => choose({ ...target, preset: p })}
          listWidth={260}
        />
      )}
      <button className="run-btn" disabled={building} aria-label="Build" {...hint(`Build (Ctrl+F9)\n${config.buildLabel}`)} onClick={actions.current.build}>
        {building ? <span className="spinner" /> : <HammerIcon size={14} />}
      </button>
      <button
        className="run-btn run-play"
        disabled={building || !config.program}
        aria-label="Run"
        {...hint(config.program ? `Run (Shift+F10)\n${config.program}` : "No program to run in this configuration")}
        onClick={actions.current.run}
      >
        <PlayIcon size={13} />
      </button>
      <button
        className="run-btn run-debug"
        disabled={building || !config.program || isDebugging()}
        aria-label="Debug"
        {...hint(config.program ? "Debug (Shift+F9)" : "No program to debug in this configuration")}
        onClick={actions.current.debug}
      >
        <BugIcon size={14} />
      </button>
      <button
        className="run-btn run-stop"
        disabled={!building && !running && !isDebugging()}
        aria-label="Stop"
        {...hint("Stop (Ctrl+F2)")}
        onClick={() => (isDebugging() ? stopDebugging() : stop())}
      >
        <StopIcon size={12} />
      </button>
    </div>
  );
}
