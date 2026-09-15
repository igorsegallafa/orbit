import { useEffect, useRef, useState } from "react";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { AgentStatus, AgentStatusTracker } from "../lib/agentStatus";
import { StatusIndicator } from "./StatusIndicator";

export interface TerminalTab {
  id: string; // stable session id — display name may change, this must not
  workspace: string;
  label: string; // what was launched: "claude" | "opencode" | "shell"
  sessionName: string; // display name (renamable)
  cmd: string | null; // null = interactive shell
  args?: string[]; // extra argv (claude initial prompt)
  /**
   * Text typed into the PTY right after launch (opencode TUI takes no
   * initial prompt as argv, so we inject it as keystrokes).
   */
  initialInput?: string;
}

interface Props {
  tab: TerminalTab;
  onError: (msg: string) => void;
  onStatusChange?: (status: AgentStatus) => void;
}

export function TerminalPane({ tab, onError, onStatusChange }: Props) {
  const hostRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const ptyIdRef = useRef<number | null>(null);
  const trackerRef = useRef(new AgentStatusTracker());
  const onStatusRef = useRef(onStatusChange);
  onStatusRef.current = onStatusChange;
  const [status, setStatus] = useState<AgentStatus>("idle");

  useEffect(() => {
    if (!hostRef.current) return;
    const term = new Terminal({
      fontSize: 12.5,
      fontFamily: 'ui-monospace, "SF Mono", Menlo, monospace',
      cursorBlink: true,
      theme: {
        background: "#0d0f13",
        foreground: "#e6e8ec",
      },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    termRef.current = term;
    fit.fit();
    requestAnimationFrame(() => fit.fit());
    term.focus();

    const cwd = `${"$HOME"}/Documents/orbit-workspace/workspaces/${tab.workspace}`;
    let delivered = false;
    invoke<number>("pty_spawn", { cwd, cmd: tab.cmd, args: tab.args, cols: term.cols, rows: term.rows })
      .then((id) => {
        ptyIdRef.current = id;
        // Inject an initial prompt as keystrokes (opencode TUI has no
        // argv prompt). First attempt after the TUI boots; a second one
        // only fires if no output followed the first (TUI wasn't ready).
        if (tab.initialInput) {
          const text = tab.initialInput;
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

    const publish = () => {
      const s = trackerRef.current.status();
      setStatus(s);
      onStatusRef.current?.(s);
    };

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
      if (ptyIdRef.current !== null) {
        invoke("pty_kill", { id: ptyIdRef.current }).catch(() => null);
      }
      offOutput.then((f) => f());
      offExit.then((f) => f());
      window.clearInterval(statusPoll);
      resizeObserver.disconnect();
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