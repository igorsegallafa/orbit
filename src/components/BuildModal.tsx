import { useEffect, useRef, useState } from "react";
import { toast } from "./Toast";
import { CheckBox } from "./CheckBox";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Config } from "../types/config";

interface BuildResult {
  key: string;
  status: "ok" | "skipped" | "failed" | "cancelled";
  logPath: string | null;
}

type RowStatus = "idle" | "queued" | "running" | BuildResult["status"];

interface Props {
  workspace: string;
  repos: string[];
  onClose: () => void;
  onError: (msg: string) => void;
}

const LABEL: Record<RowStatus, string> = {
  idle: "",
  queued: "queued",
  running: "building…",
  ok: "built",
  skipped: "up to date",
  failed: "failed",
  cancelled: "stopped",
};

/** Builds the workspace repos that have a build command, one after the
 *  other, with the live output of the current one. */
export function BuildModal({ workspace, repos, onClose, onError }: Props) {
  const [buildable, setBuildable] = useState<string[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [force, setForce] = useState(false);
  const [status, setStatus] = useState<Record<string, RowStatus>>({});
  const [logPaths, setLogPaths] = useState<Record<string, string>>({});
  const [current, setCurrent] = useState<string | null>(null);
  const [log, setLog] = useState<string[]>([]);
  const stopRef = useRef(false);
  const logRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    invoke<Config>("get_config")
      .then((cfg) => {
        const b = repos.filter((r) => cfg.services.find((s) => s.name === r)?.build);
        setBuildable(b);
        setSelected(new Set(b));
      })
      .catch((e) => onError(String(e)));
  }, []);

  useEffect(() => {
    const off = listen<{ key: string; line: string }>("build-output", (e) => {
      setLog((l) => [...l.slice(-2000), e.payload.line]);
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight });
  }, [log]);

  const running = current !== null;

  const run = async () => {
    const queue = (buildable ?? []).filter((r) => selected.has(r));
    const finalStatus: Record<string, RowStatus> = {};
    stopRef.current = false;
    setLogPaths({});
    setStatus(Object.fromEntries(queue.map((r) => [r, "queued" as RowStatus])));
    for (const repo of queue) {
      if (stopRef.current) {
        setStatus((s) => ({ ...s, [repo]: "cancelled" }));
        continue;
      }
      setCurrent(repo);
      setLog([]);
      setStatus((s) => ({ ...s, [repo]: "running" }));
      try {
        const r = await invoke<BuildResult>("build_repo", { workspace, repo, force });
        finalStatus[repo] = r.status;
        setStatus((s) => ({ ...s, [repo]: r.status }));
        if (r.logPath) setLogPaths((p) => ({ ...p, [repo]: r.logPath! }));
      } catch (e) {
        finalStatus[repo] = "failed";
        setStatus((s) => ({ ...s, [repo]: "failed" }));
        setLog((l) => [...l, String(e)]);
      }
    }
    setCurrent(null);
    const results = Object.values(finalStatus);
    const failed = results.filter((r) => r === "failed").length;
    const built = results.filter((r) => r === "ok").length;
    const skipped = results.filter((r) => r === "skipped").length;
    const summary = [built && `${built} built`, skipped && `${skipped} up to date`, failed && `${failed} failed`].filter(Boolean).join(" · ");
    if (failed) toast.error("Build failed", { description: summary });
    else if (results.length) toast.success("Build finished", { description: summary });
  };

  const stop = () => {
    stopRef.current = true;
    if (current) invoke("cancel_build", { workspace, repo: current }).catch(() => null);
  };

  const toggle = (repo: string) =>
    setSelected((s) => {
      const n = new Set(s);
      if (n.has(repo)) n.delete(repo);
      else n.add(repo);
      return n;
    });

  return (
    <div className="modal-overlay" onMouseDown={running ? undefined : onClose}>
      <div className="modal modal-plan" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Build</h3>
        </div>
        <div className="modal-body">
          {buildable !== null && buildable.length === 0 && (
            <p className="muted">No repository in this workspace has a build command. Add one in Settings → Repositories.</p>
          )}
          <div className="build-rows">
            {(buildable ?? []).map((repo) => {
              const st = status[repo] ?? "idle";
              return (
                <label key={repo} className="modal-row modal-row-check">
                  <input type="checkbox" checked={selected.has(repo)} disabled={running} onChange={() => toggle(repo)} />
                  <span className="modal-row-name">{repo}</span>
                  {st === "running" && <span className="spinner" />}
                  {st !== "idle" && <span className={`tag build-st-${st}`}>{LABEL[st]}</span>}
                  {logPaths[repo] && (
                    <span className="mono truncate build-log-path" title={logPaths[repo]}>
                      log: {logPaths[repo]}
                    </span>
                  )}
                </label>
              );
            })}
          </div>
          {(log.length > 0 || running) && (
            <div className="plan-log" ref={logRef}>
              {current && <span className="plan-log-hint">{current}</span>}
              {log.map((l, i) => (
                <div className="plan-log-line" key={i}>
                  {l}
                </div>
              ))}
            </div>
          )}
        </div>
        <div className="modal-footer">
          <label className="check-item build-force">
            <CheckBox label="Rebuild even if nothing changed" checked={force} disabled={running} onChange={setForce} />
            Rebuild even if nothing changed
          </label>
          {running ? (
            <button className="danger-outline" onClick={stop}>
              Stop
            </button>
          ) : (
            <>
              <button className="secondary" onClick={onClose}>
                Close
              </button>
              <button disabled={selected.size === 0} onClick={run}>
                Build {selected.size > 0 ? `(${selected.size})` : ""}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
