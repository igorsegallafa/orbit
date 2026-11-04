import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CheckIcon, FolderIcon, HomeIcon, RepoIcon, SatelliteIcon } from "./Icons";
import { HOME_SCOPE, folderLabel, folderPath, folderScope, isFolderScope, recentFolders, rememberFolder } from "../lib/folderScope";

/** What the session runs: null cmd = the user's shell. */
const AGENTS: { label: string; tabLabel: string; cmd: string | null }[] = [
  { label: "Terminal", tabLabel: "shell", cmd: null },
  { label: "Claude Code", tabLabel: "claude", cmd: "claude" },
  { label: "Open Code", tabLabel: "opencode", cmd: "opencode" },
  { label: "OMP", tabLabel: "omp", cmd: "omp" },
];

const LAST_KEY = "orbit.new-session.last";

interface Props {
  x: number;
  y: number;
  /** Scope in focus (workspace or "@repo"), offered first and preselected. */
  focus: { scope: string; label: string; kind: "workspace" | "repo" | "folder" } | null;
  /** Cloned repos, offered as their base clone ("@repo"). */
  repos: string[];
  onOpen: (scope: string, tabLabel: string, cmd: string | null) => void;
  onClose: () => void;
}

/**
 * "New session" menu: pick where (the focused workspace or repo, home, a
 * recent folder, a repo clone, any folder) and what (shell or an agent).
 * Works from anywhere, not only inside a workspace.
 */
export function NewSessionMenu({ x, y, focus, repos, onOpen, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [recent] = useState(recentFolders);
  const [picked, setPicked] = useState<string | null>(null);
  const [extra, setExtra] = useState<string | null>(null); // folder chosen in the dialog
  const [pos, setPos] = useState({ left: x, top: y });
  const choosing = useRef(false);

  const last = (() => {
    try {
      return localStorage.getItem(LAST_KEY);
    } catch {
      return null;
    }
  })();
  const fallback = last && (last === HOME_SCOPE || recent.includes(last) || repos.includes(last.slice(1))) ? last : HOME_SCOPE;
  const target = picked ?? focus?.scope ?? fallback;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      left: Math.max(8, Math.min(x, window.innerWidth - r.width - 8)),
      top: Math.max(8, Math.min(y, window.innerHeight - r.height - 8)),
    });
  }, [x, y]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!choosing.current && !ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  const chooseFolder = async () => {
    choosing.current = true;
    try {
      const dir = await openDialog({ directory: true, multiple: false, title: "Open a session in…" });
      if (typeof dir === "string") {
        const scope = folderScope(dir);
        setExtra(scope);
        setPicked(scope);
      }
    } finally {
      choosing.current = false;
    }
  };

  const open = (agent: (typeof AGENTS)[number]) => {
    rememberFolder(target);
    if (target !== focus?.scope) {
      try {
        localStorage.setItem(LAST_KEY, target);
      } catch {
        // Only the default choice is lost.
      }
    }
    onOpen(target, agent.tabLabel, agent.cmd);
    onClose();
  };

  const row = (scope: string, icon: React.ReactNode, label: string, hint?: string) => (
    <button
      key={scope}
      className={`context-menu-item nsm-loc ${target === scope ? "nsm-loc-on" : ""}`}
      onClick={() => setPicked(scope)}
      title={isFolderScope(scope) ? folderPath(scope) : undefined}
      aria-pressed={target === scope}
    >
      <span className="nsm-icon">{icon}</span>
      <span className="nsm-name">{label}</span>
      {hint && <span className="nsm-hint">{hint}</span>}
      <span className="nsm-check">{target === scope && <CheckIcon />}</span>
    </button>
  );

  const folders = [...(extra && !recent.includes(extra) ? [extra] : []), ...recent];

  return (
    <div className="context-menu new-session-menu" ref={ref} style={pos}>
      <div className="nsm-label">Open in</div>
      <div className="nsm-locations">
        {focus &&
          row(
            focus.scope,
            focus.kind === "workspace" ? <SatelliteIcon size={13} /> : focus.kind === "repo" ? <RepoIcon size={13} /> : <FolderIcon size={13} />,
            focus.label,
            focus.kind,
          )}
        {focus?.scope !== HOME_SCOPE && row(HOME_SCOPE, <HomeIcon size={13} />, "Home")}
        {folders.filter((f) => f !== focus?.scope).map((f) => row(f, <FolderIcon size={13} />, folderLabel(f), folderPath(f).replace(/[\\/][^\\/]*[\\/]?$/, "")))}
        {repos
          .filter((r) => `@${r}` !== focus?.scope)
          .map((r) => row(`@${r}`, <RepoIcon size={13} />, r, "repo"))}
      </div>
      <button className="context-menu-item nsm-choose" onClick={chooseFolder}>
        <span className="nsm-icon">
          <FolderIcon size={13} />
        </span>
        Choose folder…
      </button>
      <div className="nsm-sep" />
      {AGENTS.map((a) => (
        <button key={a.label} className="context-menu-item" onClick={() => open(a)}>
          {a.label}
        </button>
      ))}
    </div>
  );
}
