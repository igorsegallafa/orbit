import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckBox } from "./CheckBox";
import { Skeleton } from "./Skeleton";
import * as lsp from "../lib/lsp/manager";

interface LanguageStatus {
  id: string;
  name: string;
  servers: { bin: string; install: string }[];
  command: string | null;
  customCommand: string | null;
  disabled: boolean;
  hiddenDiagnostics: string[];
  errorsOnly: boolean;
}

interface RunningServer {
  id: number;
  language: string;
  root: string;
  command: string;
  started: number;
}

interface Props {
  onError: (msg: string) => void;
}

/**
 * Settings → Languages: the language server behind each language (found on
 * PATH, or a custom command), and the servers running right now.
 */
export function LanguagesSection({ onError }: Props) {
  const [status, setStatus] = useState<{ languages: LanguageStatus[]; running: RunningServer[] } | null>(null);
  const [drafts, setDrafts] = useState<Record<string, string>>({});
  const [logs, setLogs] = useState<Record<number, string[]>>({});

  // `resetDrafts` only on first load and after saving: the periodic refresh
  // (running servers) must not wipe a command being typed.
  const load = useCallback(
    (resetDrafts: boolean) => {
      invoke<{ languages: LanguageStatus[]; running: RunningServer[] }>("lsp_status")
        .then((s) => {
          setStatus(s);
          if (resetDrafts) setDrafts(Object.fromEntries(s.languages.map((l) => [l.id, l.customCommand ?? ""])));
        })
        .catch((e) => onError(String(e)));
    },
    [onError],
  );

  useEffect(() => {
    load(true);
    const t = window.setInterval(() => load(false), 5000);
    return () => window.clearInterval(t);
  }, [load]);

  const configure = (l: LanguageStatus, command: string | null, disabled: boolean) =>
    invoke("lsp_configure", { language: l.id, command, disabled })
      .then(() => load(true))
      .catch((e) => onError(String(e)));

  const setFilter = (l: LanguageStatus, errorsOnly: boolean, hidden: string[]) =>
    invoke("lsp_diagnostics_filter", { language: l.id, errorsOnly, hidden })
      .then(() => {
        load(false);
        return lsp.loadDiagnosticFilters();
      })
      .catch((e) => onError(String(e)));

  if (!status) {
    return (
      <div className="section">
        <Skeleton w="100%" h={200} />
      </div>
    );
  }

  const names = Object.fromEntries(status.languages.map((l) => [l.id, l.name]));

  return (
    <div className="section">
      <p className="muted">
        Orbit's editor talks to the same language servers VS Code uses: go to definition (F12, Ctrl+click), references
        (Shift+F12), symbols (Ctrl+Shift+O, Ctrl+T), completion, errors and semantic colors. A server starts when you open a
        file of its language, one per repository folder, and stops a few minutes after its last file closes.
      </p>

      <div className="settings-card">
        {status.languages.map((l) => {
          const draft = drafts[l.id] ?? "";
          const commit = () => {
            if ((draft.trim() || null) !== (l.customCommand ?? null)) configure(l, draft.trim() || null, l.disabled);
          };
          return (
            <div key={l.id} className="settings-row lang-row">
              <div className="settings-row-text">
                <strong>
                  {l.name}
                  {l.disabled ? (
                    <span className="lang-state lang-state-off">disabled</span>
                  ) : l.command ? (
                    <span className="lang-state lang-state-ok">ready</span>
                  ) : (
                    <span className="lang-state lang-state-missing">not installed</span>
                  )}
                </strong>
                {l.command && !l.disabled ? (
                  <code className="lang-command">{l.command}</code>
                ) : (
                  !l.disabled && (
                    <span>
                      Install {l.servers.map((s) => s.bin).join(" or ")}: <code>{l.servers[0].install}</code>
                    </span>
                  )
                )}
                {l.id === "cpp" && (
                  <span className="lang-note">
                    clangd reads compile_commands.json for include paths and flags; the editor offers to generate it with CMake
                    when a repo has none.
                  </span>
                )}
                {!l.disabled && (
                  <div className="lang-diagnostics">
                    <label className="lang-enabled">
                      <CheckBox
                        checked={l.errorsOnly}
                        label={`Show only errors for ${l.name}`}
                        onChange={(on) => setFilter(l, on, l.hiddenDiagnostics)}
                      />
                      Errors only
                    </label>
                    {l.hiddenDiagnostics.map((code) => (
                      <span key={code} className="lang-hidden-code">
                        {code}
                        <button
                          aria-label={`Show '${code}' diagnostics again`}
                          title="Show these diagnostics again"
                          onClick={() => setFilter(l, l.errorsOnly, l.hiddenDiagnostics.filter((c) => c !== code))}
                        >
                          ×
                        </button>
                      </span>
                    ))}
                    {l.hiddenDiagnostics.length === 0 && !l.errorsOnly && (
                      <span className="lang-note">Hide a kind of diagnostic from its quick fix menu in the editor (Ctrl+.).</span>
                    )}
                  </div>
                )}
              </div>
              <div className="settings-row-control lang-controls">
                <input
                  placeholder={l.servers.map((s) => s.bin).join(" / ")}
                  value={draft}
                  onChange={(e) => setDrafts((d) => ({ ...d, [l.id]: e.target.value }))}
                  onBlur={commit}
                  onKeyDown={(e) => e.key === "Enter" && commit()}
                  title="Custom command line (leave empty to use the one found on PATH)"
                />
                <label className="lang-enabled">
                  <CheckBox checked={!l.disabled} label={`Enable ${l.name}`} onChange={(on) => configure(l, l.customCommand, !on)} />
                  Enabled
                </label>
              </div>
            </div>
          );
        })}
      </div>

      <h3 className="settings-subtitle">Running servers</h3>
      <div className="settings-card">
        {status.running.length === 0 ? (
          <div className="settings-row">
            <div className="settings-row-text">
              <span>None right now: open a source file to start its language server.</span>
            </div>
          </div>
        ) : (
          status.running.map((r) => (
            <div key={r.id} className="settings-row lang-running">
              <div className="settings-row-text">
                <strong>
                  {names[r.language] ?? r.language} <span className="lang-root">{r.root}</span>
                </strong>
                <code className="lang-command">{r.command}</code>
                {logs[r.id] && <pre className="lang-log">{logs[r.id].join("\n") || "(no output)"}</pre>}
              </div>
              <div className="settings-row-control settings-row-inline">
                <button
                  className="secondary btn-mini"
                  onClick={() =>
                    logs[r.id]
                      ? setLogs(({ [r.id]: _, ...rest }) => rest)
                      : invoke<string[]>("lsp_log", { id: r.id }).then((lines) => setLogs((l) => ({ ...l, [r.id]: lines.slice(-80) })))
                  }
                >
                  {logs[r.id] ? "Hide log" : "Log"}
                </button>
                <button
                  className="secondary btn-mini"
                  onClick={() => {
                    lsp.stopServer(r.id);
                    invoke("lsp_stop", { id: r.id }).finally(() => window.setTimeout(() => load(false), 500));
                  }}
                >
                  Stop
                </button>
              </div>
            </div>
          ))
        )}
      </div>
    </div>
  );
}
