import { useState } from "react";
import { CheckBox } from "./CheckBox";
import { invoke } from "@tauri-apps/api/core";
import { Prd, RunConfig } from "../types/ralph";
import { Select } from "./Select";

interface Props {
  workspace: string;
  repo: string;
  prd: Prd;
  initial: RunConfig;
  prompt: string;
  promptCustom: boolean;
  onStart: (config: RunConfig) => void;
  onPromptSaved: () => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

const storageKey = (ws: string) => `orbit.ralph.config.${ws}`;

/** Last config used in this workspace (per-viewer convenience). */
export function rememberedConfig(ws: string, fallback: RunConfig): RunConfig {
  try {
    const raw = localStorage.getItem(storageKey(ws));
    return raw ? { ...fallback, ...JSON.parse(raw), stopAfterStory: null } : fallback;
  } catch {
    return fallback;
  }
}

export function RalphRunModal({ workspace, repo, prd, initial, prompt, promptCustom, onStart, onPromptSaved, onClose, onError }: Props) {
  const [c, setC] = useState<RunConfig>(initial);
  const [showPrompt, setShowPrompt] = useState(false);
  const [promptText, setPromptText] = useState(prompt);
  const set = <K extends keyof RunConfig>(k: K, v: RunConfig[K]) => setC((p) => ({ ...p, [k]: v }));
  const num = (v: string, min = 0) => Math.max(min, Number(v) || 0);
  const pending = prd.userStories.filter((s) => !s.passes).sort((a, b) => a.priority - b.priority);

  const start = () => {
    try {
      localStorage.setItem(storageKey(workspace), JSON.stringify(c));
    } catch {
      // storage unavailable: the config still applies to this run
    }
    onStart(c);
  };

  const savePrompt = async (text: string | null) => {
    try {
      await invoke("ralph_save_prompt", { workspace, prompt: text });
      onPromptSaved();
      if (text === null) setShowPrompt(false);
    } catch (e) {
      onError(String(e));
    }
  };

  const next = pending[0];

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal modal-run" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Start Ralph · {repo}</h3>
        </div>
        <div className="modal-body run-body">
          <div className="run-next">
            <span className="run-next-label">Next up</span>
            <span className="rv-story-id">{next?.id}</span>
            <span className="run-next-title">{next?.title}</span>
            <span className="run-next-count">
              {pending.length} of {prd.userStories.length} left
            </span>
          </div>

          <section className="run-section">
            <h4>Scope</h4>
            <div className="choice-row">
              <div className={`choice ${!c.untilComplete ? "choice-on" : ""}`} onClick={() => set("untilComplete", false)}>
                <CheckBox label="Fixed number of iterations" checked={!c.untilComplete} onChange={() => set("untilComplete", false)} />
                <span>
                  <strong>Fixed iterations</strong>
                  <small>Each iteration is one agent run, usually one story.</small>
                </span>
              </div>
              <div className={`choice ${c.untilComplete ? "choice-on" : ""}`} onClick={() => set("untilComplete", true)}>
                <CheckBox label="Until every story passes" checked={c.untilComplete} onChange={() => set("untilComplete", true)} />
                <span>
                  <strong>Until every story passes</strong>
                  <small>Still bounded by the safety limits below.</small>
                </span>
              </div>
            </div>
            <div className="form-grid">
              <div className="field">
                <span className="field-label">Iterations</span>
                <input type="number" min={1} value={c.maxIterations} disabled={c.untilComplete} onChange={(e) => set("maxIterations", num(e.target.value, 1))} />
              </div>
              <div className="field">
                <span className="field-label">Stop after story</span>
                <Select
                  value={c.stopAfterStory ?? ""}
                  options={[
                    { value: "", label: "Don't stop early" },
                    ...pending.map((s) => ({ value: s.id, label: `${s.id} · ${s.title}` })),
                  ]}
                  onChange={(v) => set("stopAfterStory", v || null)}
                />
              </div>
            </div>
          </section>

          <section className="run-section">
            <h4>Agent</h4>
            <div className="form-grid">
              <div className="field">
                <span className="field-label">Agent</span>
                <Select
                  value={c.agent ?? ""}
                  options={[
                    { value: "", label: "From AI settings" },
                    { value: "claude", label: "Claude Code (live activity)" },
                    { value: "opencode", label: "OpenCode" },
                    { value: "omp", label: "omp" },
                  ]}
                  onChange={(v) => set("agent", v || null)}
                />
              </div>
              <div className="field">
                <span className="field-label">Model</span>
                <input value={c.model ?? ""} placeholder="From AI settings" onChange={(e) => set("model", e.target.value || null)} />
              </div>
            </div>
            <div className="field">
              <span className="field-label">Extra instructions for this run</span>
              <textarea
                rows={2}
                value={c.extraInstructions}
                placeholder="e.g. Use the project's own test command. Don't touch the public API."
                onChange={(e) => set("extraInstructions", e.target.value)}
              />
            </div>
            <button type="button" className="btn-link" onClick={() => setShowPrompt((v) => !v)}>
              {showPrompt ? "Hide" : "Customize"} the iteration prompt {promptCustom ? "· customized for this workspace" : ""}
            </button>
            {showPrompt && (
              <div className="ralph-prompt-edit">
                <textarea className="mono" rows={14} value={promptText} onChange={(e) => setPromptText(e.target.value)} />
                <span className="field-hint">
                  Placeholders: <code>{"{prd_path}"}</code> <code>{"{progress_path}"}</code> <code>{"{branch}"}</code>{" "}
                  <code>{"{repo}"}</code>. Applies to every repo of this workspace.
                </span>
                <div className="form-actions">
                  <button type="button" className="secondary" onClick={() => savePrompt(promptText)}>
                    Save prompt
                  </button>
                  {promptCustom && (
                    <button type="button" className="secondary danger-outline" onClick={() => savePrompt(null)}>
                      Restore default
                    </button>
                  )}
                </div>
              </div>
            )}
          </section>

          <section className="run-section">
            <h4>Safety limits</h4>
            <div className="form-grid form-grid-3">
              <div className="field">
                <span className="field-label">Time budget (hours)</span>
                <input type="number" min={0} step={0.5} value={c.maxHours} onChange={(e) => set("maxHours", num(e.target.value))} />
                <span className="field-hint">0 = no limit</span>
              </div>
              <div className="field">
                <span className="field-label">Stop when stuck after</span>
                <input type="number" min={1} value={c.stallAfter} onChange={(e) => set("stallAfter", num(e.target.value, 1))} />
                <span className="field-hint">iterations without a commit</span>
              </div>
              <div className="field">
                <span className="field-label">Max per iteration (min)</span>
                <input type="number" min={5} value={c.iterationTimeoutMin} onChange={(e) => set("iterationTimeoutMin", num(e.target.value, 5))} />
                <span className="field-hint">kills a hung agent</span>
              </div>
            </div>
            <div className="check-row">
              <label className="check-item">
                <CheckBox label="Wait on usage limits" checked={c.waitOnLimit} onChange={(v) => set("waitOnLimit", v)} />
                On usage limits, wait
                <input
                  className="inline-num"
                  type="number"
                  min={1}
                  value={c.limitWaitMin}
                  disabled={!c.waitOnLimit}
                  onClick={(e) => e.preventDefault()}
                  onChange={(e) => set("limitWaitMin", num(e.target.value, 1))}
                />
                min and retry
              </label>
              <label className="check-item">
                <CheckBox label="Push after each iteration" checked={c.push} onChange={(v) => set("push", v)} />
                Push after each iteration with commits
              </label>
            </div>
          </section>
        </div>
        <div className="modal-footer">
          <button className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button onClick={start}>▶ Start</button>
        </div>
      </div>
    </div>
  );
}
