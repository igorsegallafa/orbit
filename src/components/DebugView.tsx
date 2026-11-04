// The Debug tab of the bottom panel: controls, call stack, variables and
// the debug console (program output and expression evaluation).
import { useState } from "react";
import { PlayIcon, StopIcon } from "./Icons";
import {
  Variable,
  evaluate,
  loadVariables,
  pause,
  resume,
  selectFrame,
  selectThread,
  stepInto,
  stepOut,
  stepOver,
  stopDebugging,
  useDebugState,
} from "../lib/dap/session";

export function DebugView() {
  const s = useDebugState();
  const [expr, setExpr] = useState("");
  const stopped = s.status === "stopped";
  const live = s.status === "running" || s.status === "stopped" || s.status === "starting";

  const btn = (label: string, keys: string, onClick: () => void, enabled: boolean, icon: React.ReactNode) => (
    <button className="btn-plain dbg-btn" title={`${label} (${keys})`} aria-label={label} disabled={!enabled} onClick={onClick}>
      {icon}
    </button>
  );

  return (
    <div className="dbg-view">
      <div className="dbg-toolbar">
        {btn("Resume", "F9", resume, stopped, <PlayIcon size={12} />)}
        {btn("Pause", "", pause, s.status === "running", <span className="dbg-icon">❚❚</span>)}
        {btn("Step Over", "F8", stepOver, stopped, <span className="dbg-icon">↷</span>)}
        {btn("Step Into", "F7", stepInto, stopped, <span className="dbg-icon">↓</span>)}
        {btn("Step Out", "Shift+F8", stepOut, stopped, <span className="dbg-icon">↑</span>)}
        {btn("Stop", "Ctrl+F2", stopDebugging, live, <StopIcon size={11} />)}
        <span className="dbg-state">
          {s.status === "starting" ? "Starting…" : s.status === "running" ? "Running" : stopped ? `Paused${s.stopReason ? ` · ${s.stopReason}` : ""}` : s.status === "ended" ? "Ended" : ""}
        </span>
      </div>
      <div className="dbg-columns">
        <div className="dbg-frames">
          {s.threads.length > 1 && (
            <select className="dbg-thread" value={s.threadId ?? ""} onChange={(e) => void selectThread(Number(e.target.value))}>
              {s.threads.map((t) => (
                <option key={t.id} value={t.id}>
                  {t.name || `Thread ${t.id}`}
                </option>
              ))}
            </select>
          )}
          {s.frames.length === 0 && <div className="dbg-empty">{stopped ? "No frames" : "The call stack shows when the program pauses"}</div>}
          {s.frames.map((f) => (
            <button
              key={f.id}
              className={`btn-plain dbg-frame${f.id === s.frameId ? " dbg-frame-active" : ""}${f.open ? "" : " dbg-frame-external"}`}
              onClick={() => void selectFrame(f.id)}
            >
              <span className="dbg-frame-name">{f.name}</span>
              <span className="dbg-frame-loc">{f.file ? `${f.file.split(/[\\/]/).pop()}:${f.line}` : ""}</span>
            </button>
          ))}
        </div>
        <div className="dbg-vars">
          {s.scopes.length === 0 && <div className="dbg-empty">{stopped ? "No variables" : "Variables show when the program pauses"}</div>}
          {s.scopes.map((sc) => (
            <VarNode key={sc.ref} v={{ name: sc.name, value: "", ref: sc.ref }} depth={0} scope vars={s.vars} />
          ))}
        </div>
      </div>
      <div className="dbg-console">
        <div className="dbg-console-lines">
          {s.console.map((l, i) => (
            <div key={i} className={`dbg-line dbg-line-${l.kind}`}>
              {l.kind === "input" ? `› ${l.text}` : l.text}
            </div>
          ))}
        </div>
        <input
          className="dbg-eval"
          placeholder={stopped ? "Evaluate expression (Enter)" : "Pause the program to evaluate expressions"}
          disabled={!stopped}
          value={expr}
          spellCheck={false}
          onChange={(e) => setExpr(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && expr.trim()) {
              void evaluate(expr);
              setExpr("");
            }
          }}
        />
      </div>
    </div>
  );
}

function VarNode({ v, depth, scope, vars }: { v: Variable; depth: number; scope?: boolean; vars: Record<number, Variable[]> }) {
  const [open, setOpen] = useState(!!scope);
  const children = vars[v.ref];
  const toggle = () => {
    if (v.ref <= 0) return;
    if (!open) void loadVariables(v.ref);
    setOpen(!open);
  };
  return (
    <>
      <button className={`btn-plain dbg-var${scope ? " dbg-scope" : ""}`} style={{ paddingLeft: 8 + depth * 14 }} onClick={toggle}>
        <span className="dbg-var-chevron">{v.ref > 0 ? (open ? "▾" : "▸") : ""}</span>
        <span className="dbg-var-name">{v.name}</span>
        {!scope && <span className="dbg-var-value">{v.value}</span>}
        {!scope && v.type && <span className="dbg-var-type">{v.type}</span>}
      </button>
      {open && children?.map((c, i) => <VarNode key={`${c.name}:${i}`} v={c} depth={depth + 1} vars={vars} />)}
      {open && v.ref > 0 && !children && <div className="dbg-empty" style={{ paddingLeft: 22 + depth * 14 }}>Loading…</div>}
    </>
  );
}
