import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Workspace } from "../types/config";
import { AgentStatus } from "../lib/agentStatus";
import { Select } from "./Select";
import { StatusIndicator } from "./StatusIndicator";
import { ConfirmDialog } from "./ConfirmDialog";
import { toast } from "./Toast";
import { PlusIcon, TrashIcon } from "./Icons";

const AGENTS = [
  { value: "claude", label: "Claude Code" },
  { value: "opencode", label: "OpenCode" },
  { value: "omp", label: "omp" },
];

interface Contestant {
  agent: string;
  model: string;
}

/** Starts one agent session in a workspace with a prompt (race: edits auto-approved). */
export type StartSession = (workspace: string, agent: string, model: string, prompt: string, sessionName: string) => void;

/** Unique, readable variant labels: claude, claude-2, opencode… */
function labels(cs: Contestant[]): string[] {
  const seen = new Map<string, number>();
  return cs.map((c) => {
    const n = (seen.get(c.agent) ?? 0) + 1;
    seen.set(c.agent, n);
    return n === 1 ? c.agent : `${c.agent}-${n}`;
  });
}

/** Same task, several agents, each in its own variant workspace. */
export function RaceModal({
  workspace,
  onStartSession,
  onStarted,
  onClose,
}: {
  workspace: Workspace;
  onStartSession: StartSession;
  onStarted: () => void;
  onClose: () => void;
}) {
  const [prompt, setPrompt] = useState("");
  const [cs, setCs] = useState<Contestant[]>([]);
  const [models, setModels] = useState<Record<string, string[]>>({});

  useEffect(() => {
    invoke<{ agent: string; model: string }>("get_ai_settings")
      .then((ai) => {
        const other = ai.agent === "claude" ? "opencode" : "claude";
        setCs([{ agent: ai.agent, model: ai.model }, { agent: other, model: "" }]);
      })
      .catch(() => setCs([{ agent: "claude", model: "" }, { agent: "opencode", model: "" }]));
    for (const a of AGENTS) {
      invoke<string[]>("list_models", { agentName: a.value })
        .then((m) => setModels((all) => ({ ...all, [a.value]: m })))
        .catch(() => null);
    }
  }, []);

  const set = (i: number, patch: Partial<Contestant>) => setCs((all) => all.map((c, j) => (j === i ? { ...c, ...patch } : c)));
  const ready = prompt.trim() && cs.length >= 2 && cs.every((c) => c.model.trim());

  const start = async () => {
    const names = labels(cs);
    // The agent runs at the workspace root; the repos are subfolders of it.
    const task = `${prompt.trim()}

(Repositories live in these subfolders: ${workspace.repos.map((r) => `${r}/`).join(", ")}. Only change files inside them.)`;
    onClose();
    const t = toast.loading(`Starting a race in ${workspace.name}…`, { description: `${cs.length} agents` });
    let started = 0;
    for (let i = 0; i < cs.length; i++) {
      const c = cs[i];
      try {
        toast.update(t, "loading", `Starting a race in ${workspace.name}…`, { description: `Preparing ${names[i]} (${i + 1}/${cs.length})` });
        const v = await invoke<Workspace>("race_create_variant", { parent: workspace.name, label: names[i], agent: `${c.agent} · ${c.model}` });
        onStartSession(v.name, c.agent, c.model, task, names[i]);
        started++;
      } catch (e) {
        toast.error(`Couldn't start ${names[i]}`, { description: String(e) });
      }
    }
    onStarted();
    if (started) toast.update(t, "success", `Race started: ${started} agent${started === 1 ? "" : "s"}`, { description: "Compare them in the workspace when they finish" });
    else toast.dismiss(t);
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal modal-race" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Race agents · {workspace.name}</h3>
        </div>
        <div className="modal-body run-body">
          <p className="race-intro">
            Each agent gets its own copy of this workspace ({workspace.repos.join(", ")}), starting from the current <span className="mono">{workspace.branch}</span>, and works on the same task. Compare what
            they did, then merge the winner back. Claude agents auto-approve file edits; commands still ask.
          </p>
          <div className="field">
            <span className="field-label">Task</span>
            <textarea
              rows={5}
              autoFocus
              value={prompt}
              placeholder="e.g. Add retry with exponential backoff to the HTTP client, with tests."
              onChange={(e) => setPrompt(e.target.value)}
            />
          </div>
          <section className="run-section">
            <h4>Agents</h4>
            <div className="race-rows">
              {cs.map((c, i) => (
                <div key={i} className="race-row">
                  <Select value={c.agent} options={AGENTS} onChange={(v) => set(i, { agent: v, model: "" })} />
                  <Select
                    value={c.model}
                    options={[{ value: "", label: "Pick a model" }, ...(models[c.agent] ?? []).map((m) => ({ value: m, label: m }))]}
                    onChange={(v) => set(i, { model: v })}
                  />
                  <button className="icon-button icon-button-danger" aria-label="Remove agent" disabled={cs.length <= 2} onClick={() => setCs((all) => all.filter((_, j) => j !== i))}>
                    <TrashIcon size={13} />
                  </button>
                </div>
              ))}
            </div>
            {cs.length < 4 && (
              <button className="btn-link race-add" onClick={() => setCs((all) => [...all, { agent: "claude", model: "" }])}>
                <PlusIcon size={12} /> Add agent
              </button>
            )}
          </section>
        </div>
        <div className="modal-footer">
          <button className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button disabled={!ready} onClick={start}>
            Start {cs.length} agents
          </button>
        </div>
      </div>
    </div>
  );
}

interface Stats {
  files: number;
  insertions: number;
  deletions: number;
  commits: number;
}

/** Variants of a race side by side: status, what changed, pick the winner. */
export function RaceSection({
  workspace,
  statusOf,
  onOpenWorkspace,
  onChanged,
  onBeforeRemove,
  onError,
}: {
  workspace: Workspace;
  statusOf: (ws: string) => AgentStatus | null;
  onOpenWorkspace: (name: string) => void;
  onChanged: () => void;
  onBeforeRemove: (names: string[]) => void;
  onError: (msg: string) => void;
}) {
  const [variants, setVariants] = useState<Workspace[]>([]);
  const [stats, setStats] = useState<Record<string, Stats>>({});
  const [confirm, setConfirm] = useState<null | { kind: "adopt"; v: Workspace } | { kind: "discard" }>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const all = await invoke<Workspace[]>("list_workspaces");
      const vs = all.filter((w) => w.variant_of === workspace.name);
      setVariants(vs);
      for (const v of vs) {
        invoke<Stats>("race_variant_stats", { name: v.name })
          .then((s) => setStats((m) => ({ ...m, [v.name]: s })))
          .catch(() => null);
      }
    } catch {
      setVariants([]);
    }
  }, [workspace.name]);

  useEffect(() => {
    load();
    const t = window.setInterval(load, 15_000);
    return () => window.clearInterval(t);
  }, [load]);

  if (variants.length === 0) return null;

  const run = async (fn: () => Promise<string[]>, ok: string) => {
    setBusy(true);
    onBeforeRemove(variants.map((v) => v.name));
    try {
      const notes = await fn();
      if (notes.length) toast.info(ok, { description: notes.join("\n") });
      else toast.success(ok);
      setConfirm(null);
      onChanged();
      load();
    } catch (e) {
      onError(String(e));
      setConfirm(null);
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="ws-section race">
      <div className="ws-section-head">
        <h3>Race · {variants.length} agents</h3>
        <button className="btn-mini danger-outline" disabled={busy} onClick={() => setConfirm({ kind: "discard" })}>
          Discard race
        </button>
      </div>
      <div className="race-grid">
        {variants.map((v) => {
          const st = stats[v.name];
          const status = statusOf(v.name);
          const [agent, model] = (v.agent ?? v.name).split(" · ");
          return (
            <div key={v.name} className="race-card">
              <div className="race-card-head">
                <strong>{agent}</strong>
                {model && <span className="race-model">{model}</span>}
              </div>
              <div className="race-status">
                {status ? <StatusIndicator status={status} withLabel /> : <span className="race-muted">no session open</span>}
              </div>
              <div className="race-stats">
                {st ? (
                  st.files === 0 ? (
                    <span className="race-muted">No changes yet</span>
                  ) : (
                    <>
                      <span>
                        {st.files} file{st.files === 1 ? "" : "s"}
                      </span>
                      <span className="git-stat-add">+{st.insertions}</span>
                      <span className="git-stat-del">−{st.deletions}</span>
                      {st.commits > 0 && (
                        <span className="race-muted">
                          · {st.commits} commit{st.commits === 1 ? "" : "s"}
                        </span>
                      )}
                    </>
                  )
                ) : (
                  <span className="race-muted">…</span>
                )}
              </div>
              <div className="race-actions">
                <button className="btn-mini secondary" onClick={() => onOpenWorkspace(v.name)}>
                  Open
                </button>
                <button className="btn-mini" disabled={busy || !st || st.files === 0} onClick={() => setConfirm({ kind: "adopt", v })}>
                  Pick winner
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {confirm?.kind === "adopt" && (
        <ConfirmDialog
          title={`Merge ${confirm.v.agent ?? confirm.v.name} into ${workspace.name}?`}
          message={`Its work (committed as a race result if needed) is merged into ${workspace.branch}. All ${variants.length} variant workspaces and their branches are then removed.`}
          confirmLabel="Merge winner"
          onConfirm={() => run(() => invoke<string[]>("race_adopt", { variant: confirm.v.name }), `Merged ${confirm.v.agent ?? confirm.v.name} into ${workspace.name}`)}
          onClose={() => setConfirm(null)}
        />
      )}
      {confirm?.kind === "discard" && (
        <ConfirmDialog
          title="Discard this race?"
          message={`Removes all ${variants.length} variant workspaces and their branches, including uncommitted work. ${workspace.name} is left untouched.`}
          confirmLabel="Discard race"
          danger
          onConfirm={() => run(() => invoke<string[]>("race_discard", { parent: workspace.name }), "Race discarded")}
          onClose={() => setConfirm(null)}
        />
      )}
    </section>
  );
}
