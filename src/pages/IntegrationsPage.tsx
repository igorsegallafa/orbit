import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Skeleton } from "../components/Skeleton";
import { ShortcutLogo, LinearLogo, FigmaLogo } from "../components/BrandIcons";

interface Status {
  kind: string;
  connected: boolean;
  account: string | null;
}

interface IntegrationDef {
  kind: "shortcut" | "linear" | "figma";
  name: string;
  description: string;
  logo: React.ReactNode;
  tokenUrl: string;
  tokenHint: string;
}

const INTEGRATIONS: IntegrationDef[] = [
  {
    kind: "shortcut",
    name: "Shortcut",
    description: "Track feature cards, stories and workflows.",
    logo: <ShortcutLogo />,
    tokenUrl: "https://app.shortcut.com/settings/account/api-tokens",
    tokenHint: "Shortcut → Settings → API Tokens",
  },
  {
    kind: "linear",
    name: "Linear",
    description: "Track issues, cycles and project status.",
    logo: <LinearLogo />,
    tokenUrl: "https://linear.app/settings/api",
    tokenHint: "Linear → Settings → API",
  },
  {
    kind: "figma",
    name: "Figma",
    description: "Preview designs attached to feature cards.",
    logo: <FigmaLogo />,
    tokenUrl: "https://www.figma.com/settings",
    tokenHint: "Figma → Settings → Personal access tokens",
  },
];

function IntegrationRow({ def, onError }: { def: IntegrationDef; onError: (m: string) => void }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [token, setToken] = useState("");
  const [testing, setTesting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showToken, setShowToken] = useState(false);

  const loadStatus = useCallback(async () => {
    try {
      setStatus(await invoke<Status>("integration_status", { kind: def.kind }));
    } catch (e) {
      onError(String(e));
    }
  }, [def.kind, onError]);

  useEffect(() => {
    loadStatus();
  }, [loadStatus]);

  const connect = async () => {
    if (!token.trim()) return;
    setTesting(true);
    setError(null);
    try {
      const account = await invoke<string>("integration_connect", {
        kind: def.kind,
        token: token.trim(),
      });
      setStatus({ kind: def.kind, connected: true, account });
      setToken("");
    } catch (e) {
      setError(String(e));
    } finally {
      setTesting(false);
    }
  };

  const disconnect = async () => {
    try {
      await invoke("integration_disconnect", { kind: def.kind });
      setStatus({ kind: def.kind, connected: false, account: null });
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div className="integration-row card">
      <div className="integration-row-head">
        <span className="integration-logo">{def.logo}</span>
        <span className="integration-row-info">
          <span className="integration-name">{def.name}</span>
          <span className="integration-desc">{def.description}</span>
        </span>
        {status === null ? (
          <Skeleton w={90} h={18} rounded={999} />
        ) : status.connected ? (
          <span className="integration-badge integration-badge-on">
            <span className="integration-badge-dot" /> Connected
          </span>
        ) : (
          <span className="integration-badge">
            <span className="integration-badge-dot" /> Not connected
          </span>
        )}
      </div>

      {status?.connected ? (
        <div className="integration-row-actions">
          <span className="integration-account">
            {status.account ? `Signed in as ${status.account}` : "Token saved locally"}
          </span>
          <span className="integration-row-actions-buttons">
            <button className="secondary danger-outline" onClick={disconnect}>
              Disconnect
            </button>
          </span>
        </div>
      ) : (
        status !== null && (
          <>
            <p className="integration-hint">
              Create an API token at{" "}
              <button className="link" onClick={() => openUrl(def.tokenUrl).catch(() => null)}>
                {def.tokenHint}
              </button>{" "}
              and paste it below.
            </p>
            <div className="integration-connect-row">
              <input
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                type={showToken ? "text" : "password"}
                value={token}
                placeholder={`${def.name} API token`}
                onChange={(e) => setToken(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && connect()}
              />
              <button className="secondary" onClick={() => setShowToken((s) => !s)}>
                {showToken ? "Hide" : "Show"}
              </button>
              <button onClick={connect} disabled={testing || !token.trim()}>
                {testing ? "Testing…" : "Connect"}
              </button>
            </div>
            {error && <p className="integration-error">{error}</p>}
          </>
        )
      )}
    </div>
  );
}

export function IntegrationsPage({ onError }: { onError: (m: string) => void }) {
  return (
    <div className="page">
      <div className="page-header">
        <h2>Integrations</h2>
      </div>
      <p className="integrations-sub">
        Connect your tools so Orbit can pull card and design context. Tokens are
        stored locally and never leave your machine.
      </p>
      {INTEGRATIONS.map((def) => (
        <IntegrationRow key={def.kind} def={def} onError={onError} />
      ))}
    </div>
  );
}