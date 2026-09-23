import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AiSettings } from "../types/config";
import { Select } from "./Select";
import { Skeleton } from "./Skeleton";

interface Props {
  onError: (msg: string) => void;
}

const AGENTS = [
  { id: "claude", label: "Claude Code" },
  { id: "opencode", label: "OpenCode" },
  { id: "omp", label: "OMP" },
];

/**
 * Settings → AI: which CLI agent and model Orbit uses for AI features
 * (Plan generation). Includes a live connectivity test.
 */
export function AiSection({ onError }: Props) {
  const [ai, setAi] = useState<AiSettings | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const [loadingModels, setLoadingModels] = useState(false);
  const [testing, setTesting] = useState(false);
  const [testResult, setTestResult] = useState<string | null>(null);

  useEffect(() => {
    invoke<AiSettings>("get_ai_settings")
      .then(setAi)
      .catch((e) => onError(String(e)));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const loadModels = (agent: string) => {
    setLoadingModels(true);
    invoke<string[]>("list_models", { agentName: agent })
      .then(setModels)
      .catch(() => setModels([]))
      .finally(() => setLoadingModels(false));
  };

  useEffect(() => {
    if (ai) loadModels(ai.agent);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ai?.agent]);

  const changeAgent = (agent: string) => {
    if (!ai || agent === ai.agent) return;
    // model format follows the agent — snap to that agent's default
    const first =
      agent === "opencode"
        ? "aihub/aihub/best"
        : agent === "omp"
          ? "aihub/glm-5.3"
          : "claude-sonnet-5";
    const next = { agent, model: first, fast_model: null };
    setAi(next);
    invoke("set_ai_settings", { ai: next }).catch((e) => onError(String(e)));
  };

  const changeModel = (model: string) => {
    if (!ai) return;
    const next = { ...ai, model };
    setAi(next);
    invoke("set_ai_settings", { ai: next }).catch((e) => onError(String(e)));
  };

  const changeFastModel = (model: string) => {
    if (!ai) return;
    const next = { ...ai, fast_model: model || null };
    setAi(next);
    invoke("set_ai_settings", { ai: next }).catch((e) => onError(String(e)));
  };

  const test = async () => {
    setTesting(true);
    setTestResult(null);
    try {
      await invoke("test_agent");
      setTestResult("ok");
    } catch (e) {
      setTestResult(String(e));
    } finally {
      setTesting(false);
    }
  };

  if (!ai) {
    return (
      <div className="section">
        <Skeleton w="40%" h={20} />
        <Skeleton w="100%" h={34} />
      </div>
    );
  }

  return (
    <div className="section">
      <div className="settings-card">
        <div className="settings-row">
          <div className="settings-row-text">
            <strong>Agent</strong>
            <span>The CLI Orbit runs for plans, commit messages, PR drafts, CI investigation and Ralph.</span>
          </div>
          <span className="mode-toggle seg">
            {AGENTS.map((a) => (
              <button key={a.id} className={`mode-btn ${ai.agent === a.id ? "mode-active" : ""}`} onClick={() => changeAgent(a.id)}>
                {a.label}
              </button>
            ))}
          </span>
        </div>

        <div className="settings-row">
          <div className="settings-row-text">
            <strong>Model</strong>
            <span>
              {ai.agent === "opencode"
                ? "Provider/model as listed by `opencode models`."
                : ai.agent === "omp"
                  ? "As listed by `omp models` (fuzzy match supported)."
                  : "Passed to `claude --model`. Each AI feature uses it unless a run overrides it."}
            </span>
          </div>
          <div className="settings-row-control">
            {loadingModels ? (
              <Skeleton w="100%" h={34} />
            ) : (
              <Select
                value={models.includes(ai.model) ? ai.model : models[0] ?? ai.model}
                options={models.map((m) => ({ value: m, label: m }))}
                onChange={changeModel}
                searchable
              />
            )}
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-text">
            <strong>Fast model</strong>
            <span>Used for commit messages and PR drafts, where speed matters more than depth.</span>
          </div>
          <div className="settings-row-control">
            {loadingModels ? (
              <Skeleton w="100%" h={34} />
            ) : (
              <Select
                value={ai.fast_model ?? ""}
                options={[
                  { value: "", label: ai.agent === "claude" ? "Default (claude-haiku-4-5)" : "Same as Model" },
                  ...models.map((m) => ({ value: m, label: m })),
                ]}
                onChange={changeFastModel}
                searchable
              />
            )}
          </div>
        </div>

        <div className="settings-row">
          <div className="settings-row-text">
            <strong>Connection</strong>
            <span>Sends a tiny prompt to check the agent is installed and the model answers.</span>
            {testResult && testResult !== "ok" && <code className="settings-error">{testResult}</code>}
          </div>
          <div className="settings-row-control settings-row-inline">
            {testResult === "ok" && (
              <span className="repo-state">
                <span className="repo-dot" /> Working
              </span>
            )}
            {testResult && testResult !== "ok" && <span className="settings-fail">Failed</span>}
            <button className="secondary" onClick={test} disabled={testing}>
              {testing ? (
                <>
                  <span className="spinner" /> Testing…
                </>
              ) : (
                "Test agent"
              )}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
