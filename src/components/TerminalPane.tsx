import { useCallback, useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AgentStatus, AgentStatusTracker, HookState } from "../lib/agentStatus";
import { StatusIndicator } from "./StatusIndicator";
import { currentTheme, onThemeChange, terminalTheme } from "../lib/theme";

export interface TerminalTab {
  id: string; // stable session id — display name may change, this must not
  workspace: string;
  label: string; // what was launched: "claude" | "opencode" | "shell"
  sessionName: string; // display name (renamable)
  cmd: string | null; // null = interactive shell
  args?: string[]; // extra argv (claude initial prompt)
  /** Run in this repo's folder instead of the workspace root (e.g. cmake). */
  repo?: string;
  /**
   * Text typed into the PTY right after launch (opencode TUI takes no
   * initial prompt as argv, so we inject it as keystrokes).
   */
  initialInput?: string;
  /** Claude conversation id (`--session-id`), so a restart can `--resume` it. */
  agentSessionId?: string;
  /** Reopened from the saved session after an app restart. */
  restored?: boolean;
}

/** Launch argv for a new claude session: a fixed id makes it resumable. */
export function claudeSessionArgs(extra: string[] = []): { args: string[]; agentSessionId: string } {
  const agentSessionId = crypto.randomUUID();
  return { args: ["--session-id", agentSessionId, ...extra], agentSessionId };
}

/** Something the user may want to hear about even when not looking. */
export type AgentSignal = { kind: "done" } | { kind: "waiting"; message?: string };

interface Props {
  tab: TerminalTab;
  onError: (msg: string) => void;
  onStatusChange?: (status: AgentStatus) => void;
  onSignal?: (signal: AgentSignal) => void;
  /** Marks this pane as the drop target for external (window-level) drags. */
  isDropTarget?: boolean;
}

/** Live PTYs by session id — used to route external file drops. */
const livePtys = new Map<string, { ptyId: number | null; inject: (text: string) => void }>();

/** Called by the App when files are dropped on the window (native event):
 *  injects the paths into the given session's prompt. */
export function dropFilesIntoTerminal(sessionId: string, paths: string[]) {
  livePtys.get(sessionId)?.inject(paths.join(" "));
}

const WAITING_TYPES = new Set(["permission_prompt", "elicitation_dialog", "elicitation_url_dialog", "agent_needs_input"]);
/** Agents without hooks: a stretch of activity this long ending in silence counts as "done". */
const FALLBACK_DONE_MS = 10_000;

export function TerminalPane({ tab, onError, onStatusChange, onSignal }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const ptyIdRef = useRef<number | null>(null);
  const trackerRef = useRef(new AgentStatusTracker());
  const onStatusRef = useRef(onStatusChange);
  onStatusRef.current = onStatusChange;
  const onSignalRef = useRef(onSignal);
  onSignalRef.current = onSignal;
  const [status, setStatus] = useState<AgentStatus>("idle");

  // Types the text into the running agent's prompt (PTY keystrokes).
  const injectText = useCallback((text: string) => {
    if (ptyIdRef.current !== null) {
      invoke("pty_write", { id: ptyIdRef.current, data: text }).catch(() => null);
    }
  }, []);

  // Register in the live-PTY registry (for external window-level drops).
  useEffect(() => {
    livePtys.set(tab.id, { ptyId: null, inject: injectText });
    return () => {
      livePtys.delete(tab.id);
    };
  }, [tab.id, injectText]);
  // Keep the registry entry's ptyId fresh.
  useEffect(() => {
    const entry = livePtys.get(tab.id);
    if (entry) entry.ptyId = ptyIdRef.current;
  });

  // Internal drag from the file tree: the tree's manual drag hit-tests
  // panels and dispatches "orbit-drop-file" with a workspace-relative path.
  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const onDropFile = (e: Event) => {
      const detail = (e as CustomEvent<{ path: string }>).detail;
      if (detail?.path) {
        injectText(detail.path);
        termRef.current?.focus();
      }
    };
    host.addEventListener("orbit-drop-file", onDropFile);
    return () => host.removeEventListener("orbit-drop-file", onDropFile);
  }, [injectText]);

  useEffect(() => {
    if (!hostRef.current) return;
    const term = new Terminal({
      fontSize: 12.5,
      fontFamily: 'ui-monospace, "SF Mono", Menlo, monospace',
      cursorBlink: true,
      theme: terminalTheme(currentTheme()),
    });
    const offTheme = onThemeChange((t) => {
      term.options.theme = terminalTheme(t);
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    termRef.current = term;
    fit.fit();
    requestAnimationFrame(() => fit.fit());
    term.focus();

    let delivered = false;
    // Unmounted before the spawn resolved (StrictMode, fast tab close): that
    // PTY has no pane and must not keep running or writing its scrollback.
    let disposed = false;
    // After a restart: claude picks its conversation back up (and redraws it);
    // other programs start fresh under the output they printed before.
    const launch = async (): Promise<{ args?: string[]; initialInput?: string }> => {
      if (!tab.restored) return { args: tab.args, initialInput: tab.initialInput };
      if (tab.agentSessionId) {
        const exists = await invoke<boolean>("claude_session_exists", { workspace: tab.workspace, sessionId: tab.agentSessionId }).catch(() => false);
        return { args: exists ? ["--resume", tab.agentSessionId] : ["--session-id", tab.agentSessionId] };
      }
      const saved = await invoke<number[]>("pty_scrollback", { key: tab.id }).catch(() => []);
      if (saved.length) {
        // Screen clears (ConPTY starts every session with one) would wipe the
        // replay: turn each into a push of the viewport into scrollback, and
        // push once more so the new session's clear keeps the last screen too.
        const push = "\r\n".repeat(term.rows);
        const text = new TextDecoder().decode(new Uint8Array(saved)).split("\x1b[2J").join(push);
        term.write(`${text}\r\n\x1b[90m── restored after restart ──\x1b[0m${push}`);
      }
      return {};
    };
    launch()
      .then(({ args, initialInput }) =>
        disposed
          ? null
          : invoke<number>("pty_spawn", { workspace: tab.workspace, repo: tab.repo ?? null, cmd: tab.cmd, args, cols: term.cols, rows: term.rows, key: tab.id }).then((id) => ({ id, initialInput })),
      )
      .then((spawned) => {
        if (!spawned) return;
        const { id, initialInput } = spawned;
        if (disposed) {
          invoke("pty_kill", { id }).catch(() => null);
          return;
        }
        ptyIdRef.current = id;
        // Inject an initial prompt as keystrokes (opencode TUI has no
        // argv prompt). First attempt after the TUI boots; a second one
        // only fires if no output followed the first (TUI wasn't ready).
        if (initialInput) {
          const text = initialInput;
          const send = () => {
            if (ptyIdRef.current === null || delivered) return;
            invoke("pty_write", { id: ptyIdRef.current, data: text }).catch(() => null);
            window.setTimeout(() => {
              if (ptyIdRef.current !== null && !delivered) {
                invoke("pty_write", { id: ptyIdRef.current, data: "\r" }).catch(() => null);
                delivered = true;
              }
            }, 300);
          };
          window.setTimeout(send, 2500);
          window.setTimeout(send, 5000);
        }
      })
      .catch((e) => onError(String(e)));

    term.onData((data) => {
      if (ptyIdRef.current !== null) {
        invoke("pty_write", { id: ptyIdRef.current, data }).catch(() => null);
      }
    });

    let busySince = 0;
    const publish = () => {
      const tracker = trackerRef.current;
      const s = tracker.status();
      setStatus(s);
      onStatusRef.current?.(s);
      // Heuristic "done" for agents that don't report their own lifecycle.
      if (tab.cmd && !tracker.hasHooks()) {
        if (s !== "idle" && s !== "exited") busySince ||= Date.now();
        else if (busySince) {
          if (s === "idle" && Date.now() - busySince >= FALLBACK_DONE_MS) onSignalRef.current?.({ kind: "done" });
          busySince = 0;
        }
      }
    };

    const offHook = listen<{ key: string; event: string; notificationType?: string; message?: string }>("agent-event", (e) => {
      const ev = e.payload;
      if (ev.key !== tab.id) return;
      let state: HookState | null = null;
      if (ev.event === "UserPromptSubmit" || ev.event === "PostToolUse") state = "working";
      else if (ev.event === "Stop" || ev.event === "StopFailure") state = "done";
      else if (ev.event === "Notification" && WAITING_TYPES.has(ev.notificationType ?? "")) state = "waiting";
      if (!state) return;
      trackerRef.current.setHook(state);
      publish();
      if (state === "done") onSignalRef.current?.({ kind: "done" });
      if (state === "waiting") onSignalRef.current?.({ kind: "waiting", message: ev.message });
    });

    const offOutput = listen<{ id: number; data: number[] }>("pty-output", (event) => {
      if (event.payload.id !== ptyIdRef.current) return;
      const bytes = new Uint8Array(event.payload.data);
      term.write(bytes);
      trackerRef.current.feed(new TextDecoder().decode(bytes));
      publish();
    });
    const offExit = listen<number>("pty-exit", (event) => {
      if (event.payload === ptyIdRef.current) {
        trackerRef.current.markExited();
        publish();
        term.write("\r\n\x1b[90m[process exited]\x1b[0m\r\n");
      }
    });

    // Re-evaluate "idle" when output stops arriving
    const statusPoll = window.setInterval(publish, 1500);

    const onResize = () => {
      fit.fit();
      if (ptyIdRef.current !== null) {
        invoke("pty_resize", {
          id: ptyIdRef.current,
          cols: term.cols,
          rows: term.rows,
        }).catch(() => null);
      }
    };
    const resizeObserver = new ResizeObserver(onResize);
    resizeObserver.observe(hostRef.current);

    return () => {
      disposed = true;
      if (ptyIdRef.current !== null) {
        invoke("pty_kill", { id: ptyIdRef.current }).catch(() => null);
      }
      offOutput.then((f) => f());
      offExit.then((f) => f());
      offHook.then((f) => f());
      window.clearInterval(statusPoll);
      resizeObserver.disconnect();
      offTheme();
      term.dispose();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="terminal-pane">
      <div className="terminal-statusbar">
        <StatusIndicator status={status} withLabel />
        <span className="terminal-status-cmd">{tab.label}</span>
      </div>
      <div className="terminal-host" ref={hostRef} />
    </div>
  );
}