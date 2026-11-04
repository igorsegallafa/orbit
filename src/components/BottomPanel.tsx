// Bottom tool window (JetBrains style) under the editor tabs: Build output,
// the running program's terminal, and the build's Problems. Opens when a
// build starts; the run terminal stays mounted while hidden so the program
// keeps running.
import { memo, useEffect, useMemo, useRef, useState } from "react";
import { TerminalPane } from "./TerminalPane";
import { XIcon } from "./Icons";
import { BuildProblem, OutputLine, PanelView, clearRun, runExited, showPanel, togglePanel, useRunState } from "../lib/runStore";

interface Props {
  /** Opens a repo file at a position (an editor tab). */
  onOpenFile: (workspace: string, repo: string, path: string, line: number, column: number) => void;
  onError: (msg: string) => void;
}

const HEIGHT_KEY = "orbit.panel.height";
const readHeight = () => {
  try {
    return Number(localStorage.getItem(HEIGHT_KEY)) || 240;
  } catch {
    return 240;
  }
};

export function BottomPanel({ onOpenFile, onError }: Props) {
  const { build, run, panel } = useRunState();
  const [height, setHeight] = useState(readHeight);
  const outputRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  // Follow the output while it streams, unless scrolled up to read.
  useEffect(() => {
    const el = outputRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [build?.lines, panel.view, panel.open]);

  if (!build && !run) return null;

  const errors = build?.problems.filter((p) => p.severity === "error").length ?? 0;
  const warnings = (build?.problems.length ?? 0) - errors;
  const status = build?.status;

  const startResize = (e: React.MouseEvent) => {
    e.preventDefault();
    const startY = e.clientY;
    const start = height;
    let next = start;
    const onMove = (ev: MouseEvent) => {
      next = Math.min(Math.max(start + startY - ev.clientY, 120), window.innerHeight - 200);
      setHeight(next);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      try {
        localStorage.setItem(HEIGHT_KEY, String(Math.round(next)));
      } catch {
        // Height not remembered.
      }
    };
    document.body.style.cursor = "row-resize";
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  };

  const tab = (view: PanelView, label: React.ReactNode) => (
    <button
      className={`btn-plain panel-tab${panel.view === view && panel.open ? " panel-tab-active" : ""}`}
      onClick={() => (panel.view === view && panel.open ? togglePanel() : showPanel(view))}
    >
      {label}
    </button>
  );

  const open = (p: BuildProblem) => p.open && onOpenFile(p.open.workspace, p.open.repo, p.open.path, p.line, p.column);

  return (
    <section className={`bottom-panel${panel.open ? "" : " bottom-panel-closed"}`} style={panel.open ? { height } : undefined}>
      {panel.open && <div className="panel-resizer" onMouseDown={startResize} />}
      <header className="panel-header">
        {build &&
          tab(
            "build",
            <>
              <span className={`panel-status panel-status-${status}`} />
              Build
            </>,
          )}
        {run &&
          tab(
            "run",
            <>
              <span className={`panel-status ${run.exited ? "panel-status-idle" : "panel-status-running"}`} />
              Run · {run.tab.sessionName}
            </>,
          )}
        {build &&
          tab(
            "problems",
            <>
              Problems
              {errors > 0 && <span className="panel-count panel-count-error">{errors}</span>}
              {warnings > 0 && <span className="panel-count panel-count-warning">{warnings}</span>}
            </>,
          )}
        <span className="panel-spacer" />
        {build && panel.view === "build" && panel.open && <span className="panel-meta">{statusText(build.status, build.startedAt)}</span>}
        {run && run.exited && panel.view === "run" && panel.open && (
          <button className="btn-plain panel-action" onClick={clearRun}>
            Clear
          </button>
        )}
        <button className="btn-plain panel-action" aria-label={panel.open ? "Hide panel" : "Show panel"} onClick={togglePanel}>
          {panel.open ? <XIcon size={12} /> : "▴"}
        </button>
      </header>
      <div className="panel-body" hidden={!panel.open}>
        {build && (
          <div
            ref={outputRef}
            className="panel-output"
            hidden={panel.view !== "build"}
            onScroll={(e) => {
              const el = e.currentTarget;
              stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
            }}
          >
            {build.lines.map((l, i) => (
              <Line key={i} line={l} onOpen={open} />
            ))}
          </div>
        )}
        {run && (
          <div className="panel-run" hidden={panel.view !== "run"}>
            <TerminalPane
              key={run.tab.id}
              tab={run.tab}
              onError={onError}
              onStatusChange={(s) => s === "exited" && runExited()}
              active={panel.open && panel.view === "run"}
            />
          </div>
        )}
        {build && panel.view === "problems" && <ProblemList problems={build.problems} onOpen={open} />}
      </div>
    </section>
  );
}

function statusText(status: string, startedAt: number) {
  const secs = Math.round((Date.now() - startedAt) / 1000);
  switch (status) {
    case "running":
      return "Building…";
    case "ok":
      return `Build succeeded · ${secs}s`;
    case "failed":
      return "Build failed";
    case "cancelled":
      return "Build cancelled";
    default:
      return "";
  }
}

const Line = memo(function Line({ line, onOpen }: { line: OutputLine; onOpen: (p: BuildProblem) => void }) {
  const problem = line.problem;
  const text = line.text.replace(/\x1b\[[0-9;]*m/g, "");
  return (
    <div className={`out-line out-${line.stream}${problem ? ` out-${problem.severity}` : ""}`}>
      {problem?.open ? (
        <button className="btn-plain out-link" onClick={() => onOpen(problem)}>
          {text}
        </button>
      ) : (
        text
      )}
    </div>
  );
});

function ProblemList({ problems, onOpen }: { problems: BuildProblem[]; onOpen: (p: BuildProblem) => void }) {
  const groups = useMemo(() => {
    const by = new Map<string, BuildProblem[]>();
    for (const p of [...problems].sort((a, b) => (a.severity === b.severity ? 0 : a.severity === "error" ? -1 : 1))) {
      const k = p.open ? `${p.open.repo}/${p.open.path}` : (p.file ?? "Linker");
      by.set(k, [...(by.get(k) ?? []), p]);
    }
    return [...by];
  }, [problems]);
  if (!problems.length) return <div className="panel-empty">No problems in the last build.</div>;
  return (
    <div className="panel-problems">
      {groups.map(([file, list]) => (
        <div key={file} className="problem-group">
          <div className="problem-file">{file}</div>
          {list.map((p, i) => (
            <button key={i} className="btn-plain problem-row" disabled={!p.open} onClick={() => onOpen(p)}>
              <span className={`problem-icon problem-${p.severity}`}>{p.severity === "error" ? "⨯" : "⚠"}</span>
              <span className="problem-message">{p.message}</span>
              {p.code && <span className="problem-code">{p.code}</span>}
              {p.line > 0 && (
                <span className="problem-loc">
                  {p.line}:{p.column}
                </span>
              )}
            </button>
          ))}
        </div>
      ))}
    </div>
  );
}
