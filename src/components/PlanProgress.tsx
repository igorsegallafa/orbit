import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

interface PlanTask {
  text: string;
  done: boolean;
}

interface Props {
  workspace: string;
  /** Re-read tasks when this key changes (tab focus, plan regenerated). */
  refreshKey?: number;
  onOpenPlan: () => void;
  onError: (msg: string) => void;
}

/**
 * PLAN.md task tracker on the workspace home: checkable list + progress bar
 * reflecting `- [ ]` / `- [x]` lines. Checking a box writes back to the file,
 * so the agent and the user share the same source of truth.
 */
export function PlanProgress({ workspace, refreshKey, onOpenPlan, onError }: Props) {
  const [tasks, setTasks] = useState<PlanTask[] | null>(null);

  const load = useCallback(async () => {
    try {
      setTasks(await invoke<PlanTask[]>("plan_tasks", { name: workspace }));
    } catch (e) {
      onError(String(e));
    }
  }, [workspace, onError]);

  useEffect(() => {
    load();
  }, [load, refreshKey]);

  // Agents mark checkboxes as they work — keep the list fresh while the
  // workspace tab is open. Cheap: one small file read every 5s.
  useEffect(() => {
    const t = globalThis.setInterval(load, 5000);
    return () => globalThis.clearInterval(t);
  }, [load]);

  const done = tasks?.filter((t) => t.done).length ?? 0;
  const total = tasks?.length ?? 0;
  const pct = total > 0 ? Math.round((done / total) * 100) : 0;

  const toggle = async (index: number, next: boolean) => {
    // Optimistic flip; the backend write keeps PLAN.md in sync.
    setTasks((ts) => (ts ? ts.map((t, i) => (i === index ? { ...t, done: next } : t)) : ts));
    try {
      await invoke("set_plan_task", { name: workspace, index, done: next });
    } catch (e) {
      onError(String(e));
      load(); // revert on failure
    }
  };

  if (tasks === null || total === 0) return null;

  return (
    <div className="card plan-progress">
      <div className="plan-progress-head">
        <h3>Plan · progress</h3>
        <button className="link" onClick={onOpenPlan}>
          open PLAN.md
        </button>
      </div>

      <div className="plan-progress-bar-wrap">
        <div className="plan-progress-bar">
          <div className="plan-progress-fill" style={{ width: `${pct}%` }} />
        </div>
        <span className="plan-progress-label">
          {done}/{total} · {pct}%
        </span>
      </div>

      <div className="plan-tasks">
        {tasks.map((t, i) => (
          <label key={i} className={`plan-task ${t.done ? "plan-task-done" : ""}`}>
            <input
              type="checkbox"
              checked={t.done}
              onChange={(e) => toggle(i, e.target.checked)}
            />
            <span className="plan-task-text">{t.text}</span>
          </label>
        ))}
      </div>
    </div>
  );
}