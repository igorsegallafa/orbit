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
    const first = agent === "opencode" ? "aihub/aihub/best" : "claude-sonnet-5";
    const next = { agent, model: first };
    setAi(next);
    invoke("set_ai_settings", { ai: next }).catch((e) => onError(String(e)));
  };

  const changeModel = (model: string) => {
    if (!ai) return;
    const next = { ...ai, model };
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
    <div className="section ai-section">
      <div className="card">
        <h3>AI agent</h3>
        <p className="integration-hint">
          Which CLI agent Orbit uses for AI features like Plan generation.
        </p>
        <div className="wizard-source-pills">
          {AGENTS.map((a) => (
            <button
              key={a.id}
              className={`wizard-source-pill ${ai.agent === a.id ? "wizard-source-active" : ""}`}
              onClick={() => changeAgent(a.id)}
            >
              {a.label}
            </button>
          ))}
        </div>
      </div>

      <div className="card">
        <h3>Model</h3>
        <p className="integration-hint">
          {ai.agent === "opencode"
            ? "Provider/model as listed by `opencode models`."
            : "Model alias passed to `claude -p --model`."}
        </p>
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

      <div className="card">
        <h3>Test</h3>
        <p className="integration-hint">
          Runs the agent with a tiny prompt to confirm it is installed and the
          model responds.
        </p>
        <div className="form-actions">
          <button onClick={test} disabled={testing}>
            {testing ? "Testing…" : "Test agent"}
          </button>
          {testResult === "ok" && <span className="tag tag-ok">working</span>}
          {testResult && testResult !== "ok" && (
            <span className="tag tag-warn" title={testResult}>
              failed — see tooltip
            </span>
          )}
        </div>
      </div>
    </div>
  );
}