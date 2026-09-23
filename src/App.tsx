import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useConfig } from "./hooks/useConfig";
import { DashboardPage } from "./pages/DashboardPage";
import { SettingsPage } from "./pages/SettingsPage";
import { IntegrationsPage } from "./pages/IntegrationsPage";
import { CodeReviewPage } from "./pages/CodeReviewPage";
import { WorkspaceDetailPage } from "./pages/WorkspaceDetailPage";
import { SidebarResizer } from "./components/SidebarResizer";
import { ContextMenu, MenuItem, useContextMenu } from "./components/ContextMenu";
import { EditorPane, revealInEditor } from "./components/EditorPane";
import { FindPopup, OpenMatch } from "./components/FindInFiles";
import { AgentSignal, TerminalPane, TerminalTab, claudeSessionArgs, dropFilesIntoTerminal } from "./components/TerminalPane";
import { InboxButton, InboxItem, InboxKind, notifyOs } from "./components/Inbox";
import { FileTreePanel } from "./components/FileTreePanel";
import { ReviewPane } from "./components/ReviewPane";
import { CommitReviewPane } from "./components/CommitReviewPane";
import { PrReviewPane } from "./components/PrReviewPane";
import { SearchEverywhereModal } from "./components/SearchEverywhereModal";
import { UsageBar } from "./components/UsageBar";
import { TooltipHost, tooltip } from "./components/Tooltip";
import { WindowControls, isMac } from "./components/WindowControls";
import { RalphView } from "./components/RalphView";
import { ToastHost, toast } from "./components/Toast";
import { useRalphRunning } from "./lib/useRalphRunning";
import { startUpdateChecks } from "./lib/updater";
import { listen } from "@tauri-apps/api/event";
import { reasonLabel, StopReason } from "./types/ralph";
import {
  HomeIcon, SettingsIcon, SatelliteIcon, DocIcon, TerminalIcon, PlusIcon,
  ChevronRightIcon, PlugIcon, DiffIcon, GitIcon, PanelLeftIcon, PanelLeftExpandIcon,
  PanelRightIcon, PanelRightExpandIcon, SparkIcon,
} from "./components/Icons";
import { SkeletonCards, SkeletonTable } from "./components/Skeleton";
import { randomSessionName } from "./lib/names";
import { AgentStatus } from "./lib/agentStatus";
import { StatusIndicator } from "./components/StatusIndicator";
import { GitCommit, PullRequest, Workspace } from "./types/config";
import "./App.css";

type NavPage =
  | { kind: "dashboard" }
  | { kind: "reviews" }
  | { kind: "settings" }
  | { kind: "integrations" };

type Tab =
  | { kind: "workspace"; workspace: Workspace }
  | { kind: "editor"; workspace: string; repo: string; path: string }
  | { kind: "review"; workspace: string; repo: string; path: string }
  | { kind: "commit"; workspace: string; repo: string; commit: GitCommit }
  | { kind: "pr"; prs: PullRequest[] }
  | { kind: "ralph"; workspace: string }
  | { kind: "terminal"; terminal: TerminalTab };

const SIDEBAR_KEY = "orbit.sidebar-width";
const DOCK_KEY = "orbit.dock-width";
const DEFAULT_WIDTH = 220;
const COLLAPSED = 64;

function loadStored(key: string, def: number, min: number, max: number): number {
  const n = Number(localStorage.getItem(key));
  return Number.isFinite(n) && n >= min && n <= max ? n : def;
}

function tabId(tab: Tab): string {
  const t = tab;
  if (t.kind === "workspace") return `ws:${t.workspace.name}`;
  if (t.kind === "editor") return `ed:${t.workspace}/${t.repo}/${t.path}`;
  if (t.kind === "review") return `rv:${t.workspace}/${t.repo}/${t.path}`;
  if (t.kind === "commit") return `cm:${t.workspace}/${t.repo}/${t.commit.sha}`;
  if (t.kind === "pr") return `pr:${t.prs.map((p) => `${p.ownerRepo}/${p.number}`).join("+")}`;
  if (t.kind === "ralph") return `ralph:${t.workspace}`;
  // Stable id: must NOT embed the display name, or renaming re-keys the pane
  // and remounts the terminal (killing the PTY session).
  return t.terminal.id;
}

const SESSION_KEY = "orbit.session";

/** Open tabs from the last run; terminals come back marked as restored. */
function loadSession(): { tabs: Tab[]; active: string | null } {
  try {
    const raw = JSON.parse(localStorage.getItem(SESSION_KEY) ?? "null");
    if (!raw || !Array.isArray(raw.tabs)) return { tabs: [], active: null };
    const tabs = (raw.tabs as Tab[]).map((t) => (t.kind === "terminal" ? { ...t, terminal: { ...t.terminal, restored: true } } : t));
    const active = tabs.some((t) => tabId(t) === raw.active) ? raw.active : (tabs[0] ? tabId(tabs[0]) : null);
    return { tabs, active };
  } catch {
    return { tabs: [], active: null };
  }
}

function App() {
  const { config, setConfig, loading, error, setError } = useConfig();
  const [navPage, setNavPage] = useState<NavPage>({ kind: "dashboard" });
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [initialSession] = useState(loadSession);
  const [tabs, setTabs] = useState<Tab[]>(initialSession.tabs);
  const [activeTab, setActiveTab] = useState<string | null>(initialSession.active);
  const [sessionStatuses, setSessionStatuses] = useState<Record<string, AgentStatus>>({});
  const [inbox, setInbox] = useState<InboxItem[]>([]);
  const activeTabRef = useRef<string | null>(null);
  activeTabRef.current = activeTab;
  const [renamingTab, setRenamingTab] = useState<string | null>(null);
  const [sidebarWidth, setSidebarWidth] = useState(() => loadStored(SIDEBAR_KEY, DEFAULT_WIDTH, 64, 400));
  const [sidebarHidden, setSidebarHidden] = useState(() => localStorage.getItem("orbit.sidebar-hidden") === "1");
  const [dockHidden, setDockHidden] = useState(() => localStorage.getItem("orbit.dock-hidden") === "1");
  const [dockWidth, setDockWidth] = useState(() => loadStored(DOCK_KEY, 240, 180, 460));
  const [searchOpen, setSearchOpen] = useState(false);
  const [findOpen, setFindOpen] = useState(false);
  const [plusMenu, setPlusMenu] = useState<{ x: number; y: number } | null>(null);
  const [wsExpanded, setWsExpanded] = useState<Record<string, boolean>>(() => {
    try {
      return JSON.parse(localStorage.getItem("orbit.ws-expanded") ?? "{}");
    } catch {
      return {};
    }
  });
  const { menu, setMenu, openFromEvent } = useContextMenu<Workspace>();
  const tabStripRef = useRef<HTMLDivElement>(null);
  const tabMenu = useContextMenu<Tab>();

  const loadWorkspaces = useCallback(async () => {
    try {
      const list = await invoke<Workspace[]>("list_workspaces");
      setWorkspaces(list);
      // Tabs of workspaces that no longer exist (removed elsewhere) can't come back.
      const names = new Set(list.map((w) => w.name));
      const owner = (t: Tab) =>
        t.kind === "workspace" ? t.workspace.name : t.kind === "terminal" ? t.terminal.workspace : t.kind === "pr" ? null : t.workspace;
      setTabs((ts) => {
        const kept = ts.filter((t) => {
          const ws = owner(t);
          return ws === null || names.has(ws);
        });
        return kept.length === ts.length ? ts : kept;
      });
    } catch (e) {
      setError(String(e));
    }
  }, [setError]);

  // Persist open tabs so a restart reopens them.
  useEffect(() => {
    try {
      localStorage.setItem(SESSION_KEY, JSON.stringify({ tabs, active: activeTab }));
    } catch {
      // storage full/unavailable: the session just won't be restored
    }
  }, [tabs, activeTab]);

  useEffect(() => {
    loadWorkspaces();
  }, [loadWorkspaces]);

  useEffect(() => startUpdateChecks(), []);

  // External file drops (Finder → window): the Tauri native drag-drop event
  // carries absolute paths; inject them into the active agent terminal.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    import("@tauri-apps/api/webview").then(({ getCurrentWebview }) => {
      const w = getCurrentWebview();
      const off = w.onDragDropEvent((event) => {
        if (event.payload.type !== "drop") return;
        const paths = event.payload.paths ?? [];
        if (paths.length === 0) return;
        // Route to the active terminal session, if any.
        const activeTabObj = tabs.find((t) => tabId(t) === activeTab);
        if (activeTabObj?.kind === "terminal") {
          dropFilesIntoTerminal(activeTabObj.terminal.id, paths);
        }
      });
      off.then((f: () => void) => (unlisten = f));
    });
    return () => unlisten?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTab, tabs]);

  // Keep the active tab visible in the strip: opening/focusing a tab when
  // the strip overflows should scroll it into view automatically.
  useEffect(() => {
    const strip = tabStripRef.current;
    const active = strip?.querySelector(".tab-active");
    active?.scrollIntoView({ block: "nearest", inline: "nearest", behavior: "smooth" });
  }, [activeTab, tabs.length]);

  // JetBrains-style double-shift: two bare Shift presses within 350ms open
  // Search Everywhere. Guarded against Shift+key combos (typing capitals).
  useEffect(() => {
    let lastShift = 0;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Shift" && !e.metaKey && !e.ctrlKey && !e.altKey) {
        const now = Date.now();
        if (now - lastShift < 350) {
          lastShift = 0;
          setSearchOpen((open) => !open);
        } else {
          lastShift = now;
        }
      } else if (e.key !== "Meta" && e.key !== "Control" && e.key !== "Alt") {
        lastShift = 0;
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const onResize = useCallback((w: number) => {
    setSidebarWidth(w);
    localStorage.setItem(SIDEBAR_KEY, String(w));
  }, []);

  // Collapse/expand animation: the width transition lives on a class that
  // exists ONLY for the duration of a toggle (state-driven — React owns the
  // className, so adding it via DOM would be wiped by the re-render), so
  // dragging the resizer (which also sets width) stays instant.
  const [sidebarAnimating, setSidebarAnimating] = useState(false);
  const [dockAnimating, setDockAnimating] = useState(false);

  const toggleSidebar = useCallback(() => {
    setSidebarAnimating(true);
    // Safety net: if no transitionend fires (e.g. width unchanged), clear.
    window.setTimeout(() => setSidebarAnimating(false), 400);
    setSidebarHidden((h) => {
      localStorage.setItem("orbit.sidebar-hidden", h ? "0" : "1");
      return !h;
    });
  }, []);

  const toggleDock = useCallback(() => {
    setDockAnimating(true);
    window.setTimeout(() => setDockAnimating(false), 400);
    setDockHidden((h) => {
      localStorage.setItem("orbit.dock-hidden", h ? "0" : "1");
      return !h;
    });
  }, []);

  const onDockResize = useCallback(
    (w: number) => {
      setDockWidth(w);
      localStorage.setItem(DOCK_KEY, String(w));
    },
    []
  );

  const collapsed = sidebarWidth <= COLLAPSED;
  const repoCount = config.services.length;
  const groupCount = Object.keys(config.groups).length;

  // Every onError in the app surfaces as an error toast.
  useEffect(() => {
    if (!error) return;
    toast.error("Something went wrong", { description: error });
    setError(null);
  }, [error, setError]);

  // A Ralph run finishing matters even when its tab isn't open.
  useEffect(() => {
    const off = listen<{ key: string; event: { kind: string; reason?: StopReason; iterations?: number } }>(
      "ralph-event",
      (e) => {
        const ev = e.payload.event;
        if (ev.kind !== "stopped") return;
        const [ws, repo] = e.payload.key.split("/");
        const ok = ev.reason?.kind === "complete";
        const opts = {
          description: `${ws} · ${repo} · ${ev.iterations ?? 0} iteration(s)`,
          action: { label: "Open", onClick: () => openTab({ kind: "ralph", workspace: ws }) },
        };
        if (ok) toast.success("Ralph finished every story", opts);
        else toast.info(`Ralph stopped: ${reasonLabel(ev.reason ?? null)}`, opts);
        pushInbox(`ralph:${ws}`, ok ? "ralph-done" : "ralph-stopped", ok ? "Ralph finished every story" : `Ralph stopped: ${reasonLabel(ev.reason ?? null)}`, opts.description);
      },
    );
    return () => {
      off.then((f) => f());
    };
  }, []);

  const ralphRunning = useRalphRunning();

  // ---------- notifications ----------
  // Unread unless the user is looking at that tab right now; the OS
  // notification only fires when they aren't.
  const pushInbox = (tabId: string, kind: InboxKind, title: string, detail: string) => {
    const looking = document.hasFocus() && activeTabRef.current === tabId;
    setInbox((all) => [
      { id: `${Date.now()}-${Math.random().toString(36).slice(2, 6)}`, tabId, kind, title, detail, at: Date.now(), read: looking },
      // One live item per tab: older ones for it are settled.
      ...all.map((i) => (i.tabId === tabId ? { ...i, read: true } : i)),
    ].slice(0, 50));
    if (!looking) notifyOs(title, detail);
  };

  const onAgentSignal = (t: TerminalTab, sig: AgentSignal) => {
    const name = t.sessionName;
    if (sig.kind === "done") pushInbox(t.id, "done", `${name} finished`, t.workspace);
    else pushInbox(t.id, "waiting", `${name} needs your attention`, sig.message ? `${t.workspace} · ${sig.message}` : t.workspace);
  };

  // Seeing the tab settles its notifications.
  useEffect(() => {
    const settle = () => {
      const id = activeTabRef.current;
      if (!id || !document.hasFocus()) return;
      setInbox((all) => (all.some((i) => i.tabId === id && !i.read) ? all.map((i) => (i.tabId === id ? { ...i, read: true } : i)) : all));
    };
    settle();
    window.addEventListener("focus", settle);
    return () => window.removeEventListener("focus", settle);
  }, [activeTab]);

  const unreadByTab = new Map<string, InboxKind>();
  for (const i of inbox) if (!i.read && !unreadByTab.has(i.tabId)) unreadByTab.set(i.tabId, i.kind);

  const openTab = (tab: Tab) => {
    const id = tabId(tab);
    setTabs((ts) => (ts.some((t) => tabId(t) === id) ? ts : [...ts, tab]));
    setActiveTab(id);
  };

  // Workspace folders can't be deleted on Windows while a session runs in
  // them: close their tabs (killing the processes) before a removal.
  const closeWorkspaceSessions = (names: string[]) => {
    const doomed = tabs.filter((t) => t.kind !== "pr" && t.kind !== "workspace" && names.includes(t.kind === "terminal" ? t.terminal.workspace : t.workspace));
    if (!doomed.length) return;
    doomed.forEach(forgetTerminal);
    const ids = new Set(doomed.map(tabId));
    setTabs((ts) => ts.filter((t) => !ids.has(tabId(t))));
    if (activeTab && ids.has(activeTab)) setActiveTab(null);
  };

  const forgetTerminal = (t: Tab | undefined) => {
    if (t?.kind === "terminal") invoke("pty_forget", { key: t.terminal.id }).catch(() => null);
  };

  const closeTab = (id: string) => {
    forgetTerminal(tabs.find((t) => tabId(t) === id));
    setTabs((ts) => {
      const idx = ts.findIndex((t) => tabId(t) === id);
      const next = ts.filter((t) => tabId(t) !== id);
      if (activeTab === id) {
        const nextTab = next[Math.min(idx, next.length - 1)];
        // Never leave a null active tab while tabs remain — the tab body
        // would render an empty viewport.
        setActiveTab(nextTab ? tabId(nextTab) : null);
      }
      return next;
    });
    setSessionStatuses((s) => {
      const { [id]: _drop, ...rest } = s;
      return rest;
    });
  };

  // Batch closes for the tab context menu. All remount their panes, so PTY
  // sessions in closed tabs end naturally via cleanup.
  const closeOtherTabs = (keepId: string) => {
    const keep = tabs.find((t) => tabId(t) === keepId);
    if (!keep) return;
    setTabs((ts) => {
      for (const t of ts) {
        if (tabId(t) !== keepId) {
          forgetTerminal(t);
          setSessionStatuses((s) => {
            const { [tabId(t)]: _drop, ...rest } = s;
            return rest;
          });
        }
      }
      return [keep];
    });
    setActiveTab(keepId);
  };

  const closeAllTabs = () => {
    tabs.forEach(forgetTerminal);
    setTabs([]);
    setActiveTab(null);
    setSessionStatuses({});
  };

  const tabMenuItems = (t: Tab): MenuItem[] => [
    {
      label: "Close",
      onSelect: () => closeTab(tabId(t)),
    },
    {
      label: "Close Other Tabs",
      onSelect: () => closeOtherTabs(tabId(t)),
    },
    {
      label: "Close All Tabs",
      danger: true,
      onSelect: () => closeAllTabs(),
    },
  ];

  const openWorkspaceTab = (ws: Workspace) => {
    setNavPage({ kind: "dashboard" });
    openTab({ kind: "workspace", workspace: ws });
  };

  const openFileTab = (wsName: string, repo: string, path: string) => {
    setNavPage({ kind: "dashboard" });
    openTab({ kind: "editor", workspace: wsName, repo, path });
  };

  const newTerminal = (wsName: string, label: string, cmd: string | null) => {
    const taken = tabs
      .filter((t): t is Extract<Tab, { kind: "terminal" }> => t.kind === "terminal")
      .map((t) => t.terminal.sessionName);
    const claude = cmd === "claude" ? claudeSessionArgs() : null;
    openTab({
      kind: "terminal",
      terminal: {
        id: `tm:${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        workspace: wsName,
        label,
        sessionName: randomSessionName(taken),
        cmd,
        args: claude?.args,
        agentSessionId: claude?.agentSessionId,
      },
    });
  };

   const openShellTerminal = (wsName: string) => newTerminal(wsName, "shell", null);

  // Opens an interactive agent session seeded with a prompt (grill-me
  // interviews, plan application). claude takes the prompt as argv; the
  // opencode TUI types it in after boot.
  const openPromptedSession = (wsName: string, agent: string, model: string, prompt: string, opts: { name?: string; autoEdit?: boolean } = {}) => {
    // claude and omp both take the initial prompt as an argv message; the
    // opencode TUI treats the positional as a project dir, so we type it in.
    const takesArgvPrompt = agent === "claude" || agent === "omp";
    const args = takesArgvPrompt
      ? ["--model", model, ...(opts.autoEdit && agent === "claude" ? ["--permission-mode", "acceptEdits"] : []), prompt]
      : ["--model", model];
    const initialInput = takesArgvPrompt ? undefined : prompt;
    const claude = agent === "claude" ? claudeSessionArgs(args) : null;
    openTab({
      kind: "terminal",
      terminal: {
        id: `tm:${Date.now()}-${Math.random().toString(36).slice(2, 7)}`,
        workspace: wsName,
        label: agent,
        sessionName: opts.name ?? `run ${wsName.split("/").pop() ?? ""}`.trim(),
        cmd: agent,
        args: claude?.args ?? args,
        agentSessionId: claude?.agentSessionId,
        initialInput,
      },
    });
  };

  // Collapse/expand of workspace sessions in the sidebar; persisted so the
  // tree reopens as the user left it.
  const toggleWsExpanded = (wsName: string) => {
    setWsExpanded((prev) => {
      const next = { ...prev, [wsName]: !(prev[wsName] ?? true) };
      localStorage.setItem("orbit.ws-expanded", JSON.stringify(next));
      return next;
    });
  };

  // Rename: display-only. The tab id is stable so the pane never remounts.
  const renameTab = (id: string, newName: string) => {
    const name = newName.trim();
    if (!name) return;
    const target = tabs.find((t) => tabId(t) === id);
    if (!target || target.kind !== "terminal") return;
    if (
      tabs.some(
        (t) =>
          t.kind === "terminal" &&
          t.terminal.workspace === target.terminal.workspace &&
          t.terminal.sessionName === name &&
          tabId(t) !== id
      )
    ) {
      setError(`a session named '${name}' already exists in this workspace`);
      return;
    }
    setTabs((ts) =>
      ts.map((t) =>
        tabId(t) === id && t.kind === "terminal"
          ? { ...t, terminal: { ...t.terminal, sessionName: name } }
          : t
      )
    );
  };

  // Most telling status among a workspace's agent sessions (race cards).
  const workspaceAgentStatus = (ws: string): AgentStatus | null => {
    const order: AgentStatus[] = ["waiting", "thinking", "running", "editing", "busy", "idle", "exited"];
    const found = tabs
      .filter((t): t is Extract<Tab, { kind: "terminal" }> => t.kind === "terminal" && t.terminal.workspace === ws && t.terminal.cmd !== null)
      .map((t) => sessionStatuses[tabId(t)] ?? "idle");
    return found.sort((a, b) => order.indexOf(a) - order.indexOf(b))[0] ?? null;
  };

  const setSessionStatus = (id: string, status: AgentStatus) => {
    setSessionStatuses((s) => (s[id] === status ? s : { ...s, [id]: status }));
  };

  const sidebarMenuItems = (ws: Workspace): MenuItem[] => [
    { label: "Open workspace", onSelect: () => openWorkspaceTab(ws) },
    { label: "New terminal", onSelect: () => openShellTerminal(ws.name) },
    {
      label: "Open in editor",
      onSelect: () =>
        invoke("open_workspace_in_editor", { name: ws.name }).catch((e) => setError(String(e))),
    },
    {
      label: "Open folder in Finder",
      onSelect: () =>
        invoke("reveal_workspace_folder", { name: ws.name }).catch((e) => setError(String(e))),
    },
  ];

  const active = tabs.find((t) => tabId(t) === activeTab) ?? null;

  // The right dock shows for whichever workspace is "in focus": the active
  // workspace tab, else a workspace owning the active editor/review/terminal tab.
  // PR tabs are not tied to a workspace; only terminal/editor/review/
  // commit tabs carry one.
  const focusWorkspace: Workspace | null =
    active?.kind === "workspace"
      ? active.workspace
      : active && active.kind !== "pr"
        ? (workspaces.find(
            (w) =>
              w.name ===
              (active.kind === "terminal" ? active.terminal.workspace : active.workspace)
          ) ?? null)
        : null;

  const openMatch =
    (wsName: string): OpenMatch =>
    (m, length) => {
      openFileTab(wsName, m.repo, m.path);
      revealInEditor({ workspace: wsName, repo: m.repo, path: m.path, line: m.line, col: m.col, length });
    };

  // Ctrl/Cmd+Shift+F: Find in Files for the workspace in focus.
  const focusWsRef = useRef<Workspace | null>(null);
  focusWsRef.current = focusWorkspace;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.code === "KeyF" && focusWsRef.current) {
        e.preventDefault();
        setFindOpen(true);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const renderPane = (tab: Tab) => {
    if (tab.kind === "workspace") {
      return (
        <WorkspaceDetailPage
          workspace={tab.workspace}
          onOpenEditor={(repo) => openFileTab(tab.workspace.name, repo, "")}
          onOpenPlan={() => openFileTab(tab.workspace.name, "", "PLAN.md")}
          onRemoved={() => {
            loadWorkspaces();
            closeTab(tabId(tab));
          }}
          onError={setError}
          onOpenPrList={(prs) => openTab({ kind: "pr", prs })}
          onOpenRalph={() => openTab({ kind: "ralph", workspace: tab.workspace.name })}
          onStartSession={(ws, agent, model, prompt, name) => openPromptedSession(ws, agent, model, prompt, { name, autoEdit: true })}
          onOpenWorkspace={(name) => {
            const w = workspaces.find((x) => x.name === name);
            if (w) openWorkspaceTab(w);
            else invoke<Workspace[]>("list_workspaces").then((all) => {
              setWorkspaces(all);
              const found = all.find((x) => x.name === name);
              if (found) openWorkspaceTab(found);
            }).catch(() => null);
          }}
          statusOf={workspaceAgentStatus}
          onWorkspacesChanged={loadWorkspaces}
          onBeforeRemove={closeWorkspaceSessions}
        />
      );
    }
    if (tab.kind === "ralph") {
      const ws = workspaces.find((w) => w.name === tab.workspace);
      return ws ? <RalphView workspace={ws} onError={setError} /> : null;
    }
    if (tab.kind === "review") {
      return (
        <ReviewPane
          workspace={tab.workspace}
          repo={tab.repo}
          path={tab.path}
          onError={setError}
        />
      );
    }
    if (tab.kind === "commit") {
      return (
        <CommitReviewPane
          workspace={tab.workspace}
          repo={tab.repo}
          commit={tab.commit}
          onError={setError}
        />
      );
    }
    if (tab.kind === "pr") {
      return <PrReviewPane prs={tab.prs} onError={setError} />;
    }
    if (tab.kind === "editor") {
      return (
        <EditorPane
          workspace={tab.workspace}
          repo={tab.repo}
          path={tab.path}
          onError={setError}
          onApplyPlan={(agent, model) => {
            const prompt = `Read PLAN.md in this directory and implement it: work through the "- [ ]" tasks in order, marking each done (change to "- [x]") as you finish it. Commit nothing unless asked.`;
            openPromptedSession(tab.workspace, agent, model, prompt);
          }}
        />
      );
    }
    return (
      <TerminalPane
        tab={tab.terminal}
        onError={setError}
        onStatusChange={(s) => setSessionStatus(tabId(tab), s)}
        onSignal={(sig) => onAgentSignal(tab.terminal, sig)}
      />
    );
  };

  const renderMain = () => {
    if (navPage.kind === "reviews") {
      return (
        <CodeReviewPage
          onOpenPr={(prs) => {
            setNavPage({ kind: "dashboard" });
            openTab({ kind: "pr", prs });
          }}
          onError={setError}
        />
      );
    }
    if (navPage.kind === "settings") {
      return <SettingsPage config={config} onChange={setConfig} onError={setError} />;
    }
    if (navPage.kind === "integrations") {
      return <IntegrationsPage onError={setError} />;
    }
    return (
      <DashboardPage
        config={config}
        workspaces={workspaces}
        onOpen={openWorkspaceTab}
        onOpenRalph={(ws) => openTab({ kind: "ralph", workspace: ws.name })}
        onGoToSettings={() => {
          setActiveTab(null);
          setNavPage({ kind: "settings" });
        }}
        onChanged={loadWorkspaces}
        onError={setError}
      />
    );
  };

  return (
    <div className="app">
      {/* Titlebar: on macOS the traffic lights overlay the left end (Overlay
          style); elsewhere the window is frameless and draws its own controls. */}
      <header
        className={`titlebar ${isMac ? "titlebar-mac" : ""}`}
        data-tauri-drag-region
        onMouseDown={(e) => {
          // Let the drag region move the window; ignore clicks on children.
          if (e.target === e.currentTarget) e.preventDefault();
        }}
      >
        <span className="titlebar-name" data-tauri-drag-region>
          Orbit
        </span>
        <button
          className="titlebar-btn"
          onMouseEnter={(e) =>
            tooltip.show(sidebarHidden ? "Show sidebar" : "Hide sidebar", e)
          }
          onMouseLeave={() => tooltip.hide()}
          onClick={toggleSidebar}
        >
          {sidebarHidden ? <PanelLeftExpandIcon /> : <PanelLeftIcon />}
        </button>
        <span className="titlebar-spacer" data-tauri-drag-region />
        <InboxButton
          items={inbox}
          onOpen={(it) => {
            setInbox((all) => all.map((i) => (i.tabId === it.tabId ? { ...i, read: true } : i)));
            if (tabs.some((t) => tabId(t) === it.tabId)) {
              setNavPage({ kind: "dashboard" });
              setActiveTab(it.tabId);
            } else if (it.tabId.startsWith("ralph:")) {
              setNavPage({ kind: "dashboard" });
              openTab({ kind: "ralph", workspace: it.tabId.slice("ralph:".length) });
            }
          }}
          onMarkAllRead={() => setInbox((all) => all.map((i) => ({ ...i, read: true })))}
          onClear={() => setInbox([])}
        />
        <button
          className="titlebar-btn"
          onMouseEnter={(e) =>
            tooltip.show(dockHidden ? "Show panel" : "Hide panel", e)
          }
          onMouseLeave={() => tooltip.hide()}
          onClick={toggleDock}
          disabled={!focusWorkspace}
        >
          {dockHidden ? <PanelRightExpandIcon /> : <PanelRightIcon />}
        </button>
        {!isMac && <WindowControls />}
      </header>
      <div className="app-row">
      <aside
        className={`sidebar ${sidebarHidden ? "sidebar-hidden" : ""} ${sidebarAnimating ? "sidebar-anim" : ""}`}
        data-collapsed={collapsed}
        onTransitionEnd={() => setSidebarAnimating(false)}
        style={{ width: sidebarHidden ? 0 : sidebarWidth }}
      >
          <div className="brand">
            <span className="brand-logo"><SatelliteIcon size={18} /></span> {collapsed ? "" : "Orbit"}
            {!collapsed && <span className="brand-sub">multi-repo workspace</span>}
          </div>

        <nav>
          {!collapsed && <div className="nav-section">Workspaces</div>}
          <button
            className={`nav-item ${navPage.kind === "dashboard" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "dashboard" });
            }}
            title="Dashboard"
          >
            <span className="nav-icon"><HomeIcon size={15} /></span> {!collapsed && "Dashboard"}
          </button>
          <button
            className={`nav-item ${navPage.kind === "reviews" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "reviews" });
            }}
            title="Code Review"
          >
            <span className="nav-icon"><GitIcon size={15} /></span> {!collapsed && "Code Review"}
          </button>
          {!collapsed &&
            [...workspaces]
              .sort((a, b) => (a.variant_of ?? a.name).localeCompare(b.variant_of ?? b.name) || (a.variant_of ? 1 : 0) - (b.variant_of ? 1 : 0) || a.name.localeCompare(b.name))
              .map((ws) => {
              const wsId = `ws:${ws.name}`;
              // Sessions under this workspace: terminals (agents/shells) and editors
              const wsSessions = tabs.filter(
                (t): t is Extract<Tab, { kind: "terminal" } | { kind: "editor" }> =>
                  (t.kind === "terminal" && t.terminal.workspace === ws.name) ||
                  (t.kind === "editor" && t.workspace === ws.name)
              );
              return (
                <div key={ws.name} className={`nav-workspace ${ws.variant_of ? "nav-variant" : ""}`}>
                  <button
                    className={`nav-item nav-ws ${activeTab === wsId ? "active" : ""}`}
                    onClick={() => openWorkspaceTab(ws)}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      setMenu({ x: e.clientX, y: e.clientY, payload: ws });
                    }}
                    onMouseDown={(e) => openFromEvent(e, ws)}
                    onPointerDown={(e) => openFromEvent(e, ws)}
                    title={ws.name}
                  >
                    <span className="nav-icon"><SatelliteIcon size={15} /></span>
                    <span className="nav-item-label">{ws.variant_of ? ws.name.slice(ws.variant_of.length + 2) : ws.name}</span>
                    {ralphRunning.has(ws.name) && <span className="nav-ralph" title="Ralph is running" />}
                    {(() => {
                      const kinds = wsSessions.map((t) => unreadByTab.get(tabId(t)));
                      kinds.push(unreadByTab.get(`ralph:${ws.name}`));
                      const live = kinds.filter(Boolean);
                      if (!live.length) return null;
                      return <span className={`nav-unread ${live.includes("waiting") ? "nav-unread-waiting" : ""}`} />;
                    })()}
                    {wsSessions.length > 0 && (
                      <span
                        role="button"
                        tabIndex={0}
                        className={`nav-chevron ${wsExpanded[ws.name] === false ? "" : "open"}`}
                        title={wsExpanded[ws.name] === false ? "Expand sessions" : "Collapse sessions"}
                        onClick={(e) => {
                          e.stopPropagation();
                          toggleWsExpanded(ws.name);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter" || e.key === " ") {
                            e.stopPropagation();
                            toggleWsExpanded(ws.name);
                          }
                        }}
                      >
                        <ChevronRightIcon size={8} />
                      </span>
                    )}
                  </button>
                  {(wsExpanded[ws.name] ?? true) &&
                    wsSessions.map((t) => {
                      const id = tabId(t);
                      const label =
                        t.kind === "terminal"
                          ? t.terminal.sessionName
                          : t.repo
                            ? `${t.repo}/${t.path.split("/").pop()}`
                            : t.path.split("/").pop()!;
                      return (
                        <button
                          key={id}
                          className={`nav-item nav-sub ${activeTab === id ? "active" : ""}`}
                          onClick={() => setActiveTab(id)}
                          title={label}
                        >
                          {t.kind === "terminal" ? (
                            <StatusIndicator status={sessionStatuses[id] ?? "idle"} />
                          ) : (
                            <span className="nav-icon"><DocIcon size={13} /></span>
                          )}
                          <span className="nav-item-label">{label}</span>
                          {unreadByTab.has(id) && (
                            <span className={`nav-unread ${unreadByTab.get(id) === "waiting" ? "nav-unread-waiting" : ""}`} />
                          )}
                        </button>
                      );
                    })}
                </div>
              );
            })}

          {!collapsed && <div className="nav-section">General</div>}
          <button
            className={`nav-item ${navPage.kind === "settings" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "settings" });
            }}
            title="Settings"
          >
            <span className="nav-icon"><SettingsIcon size={15} /></span> {!collapsed && "Settings"}
          </button>
          <button
            className={`nav-item ${navPage.kind === "integrations" && !active ? "active" : ""}`}
            onClick={() => {
              setActiveTab(null);
              setNavPage({ kind: "integrations" });
            }}
            title="Integrations"
          >
            <span className="nav-icon"><PlugIcon size={15} /></span> {!collapsed && "Integrations"}
          </button>
        </nav>

        {!collapsed && (
          <div className="sidebar-footer">
            <div className="sidebar-stat">
              <span className="sidebar-stat-value">{repoCount}</span> repos
            </div>
            <div className="sidebar-stat">
              <span className="sidebar-stat-value">{groupCount}</span> groups
            </div>
          </div>
        )}
      </aside>

      {!sidebarHidden && <SidebarResizer width={sidebarWidth} onResize={onResize} />}

      <main className="content">
        {loading ? (
          <div className="nav-page">
            <SkeletonCards n={4} />
            <div style={{ marginTop: 20 }} />
            <SkeletonTable rows={4} cols={3} />
          </div>
        ) : tabs.length > 0 ? (
          <>
            <div
              ref={tabStripRef}
              className="tab-strip"
              onWheel={(e) => {
                // WKWebView: vertical wheel/two-finger gestures don't scroll
                // overflow-x containers — translate deltaY into scrollLeft.
                if (e.deltaY !== 0 && e.currentTarget.scrollWidth > e.currentTarget.clientWidth) {
                  e.currentTarget.scrollLeft += e.deltaY;
                  e.preventDefault();
                }
              }}
            >
              {tabs.map((t) => {
                const id = tabId(t);
                const label =
                  t.kind === "workspace"
                    ? t.workspace.name
                    : t.kind === "editor"
                      ? (t.repo
                          ? (t.path ? `${t.repo}/${t.path.split("/").pop()}` : t.repo)
                          : (t.path.split("/").pop() || "Files"))
                      : t.kind === "review"
                        ? `${t.repo}/${t.path.split("/").pop()} (diff)`
                        : t.kind === "commit"
                          ? `${t.commit.message.slice(0, 24)}…`
                          : t.kind === "ralph"
                            ? `Ralph · ${t.workspace}`
                            : t.kind === "pr"
                            ? t.prs.length > 0
                              ? `#${t.prs[0].number}` + (t.prs.length > 1 ? ` (+${t.prs.length - 1})` : "")
                              : "PRs"
                            : t.terminal.sessionName;
                const icon =
                  t.kind === "workspace" ? (
                    <SatelliteIcon size={13} />
                  ) : t.kind === "editor" ? (
                    <DocIcon size={13} />
                  ) : t.kind === "review" || t.kind === "commit" || t.kind === "pr" ? (
                    <DiffIcon size={13} />
                  ) : t.kind === "ralph" ? (
                    <SparkIcon size={13} />
                  ) : (
                    <TerminalIcon size={13} />
                  );
                const status = t.kind === "terminal" ? sessionStatuses[id] : undefined;
                return (
                  <div
                    key={id}
                    className={`tab ${activeTab === id ? "tab-active" : ""}`}
                    onClick={() => setActiveTab(id)}
                    onDoubleClick={() => {
                      if (t.kind === "terminal") setRenamingTab(id);
                    }}
                    onContextMenu={(e) => {
                      e.preventDefault();
                      tabMenu.setMenu({ x: e.clientX, y: e.clientY, payload: t });
                    }}
                    onMouseDown={(e) => {
                      if (e.button === 1) {
                        e.preventDefault();
                        closeTab(id);
                      } else {
                        tabMenu.openFromEvent(e, t);
                      }
                    }}
                    onPointerDown={(e) => tabMenu.openFromEvent(e, t)}
                  >
                    <span className="tab-icon">{icon}</span>
                    {renamingTab === id ? (
                      <input
                        className="tab-rename"
                        autoFocus
                        defaultValue={label}
                        onBlur={(e) => {
                          renameTab(id, e.target.value);
                          setRenamingTab(null);
                        }}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") {
                            renameTab(id, e.currentTarget.value);
                            setRenamingTab(null);
                          } else if (e.key === "Escape") {
                            setRenamingTab(null);
                          }
                          e.stopPropagation();
                        }}
                        onMouseDown={(e) => e.stopPropagation()}
                        onClick={(e) => e.stopPropagation()}
                        onDoubleClick={(e) => e.stopPropagation()}
                      />
                    ) : (
                      <span className="tab-label">{label}</span>
                    )}
                    {status && <StatusIndicator status={status} />}
                    <button
                      className="tab-close"
                      onClick={(e) => {
                        e.stopPropagation();
                        closeTab(id);
                      }}
                    >
                      ×
                    </button>
                  </div>
                );
              })}
              <button
                className="tab-plus"
                title="New…"
                onClick={(e) => {
                  const r = e.currentTarget.getBoundingClientRect();
                  setPlusMenu({ x: r.left, y: r.bottom + 4 });
                }}
              >
                <PlusIcon size={14} />
              </button>
            </div>
            {tabs.map((t) => (
              <div
                key={tabId(t)}
                className={`tab-body ${activeTab === tabId(t) ? "tab-body-active" : ""}`}
              >
                {renderPane(t)}
              </div>
            ))}
            {/* No active tab = user navigated to Dashboard/Settings via the
                sidebar: render the nav page alongside open (hidden) tabs. */}
            {!activeTab && <div className="nav-page">{renderMain()}</div>}
          </>
        ) : (
          <div className="nav-page">{renderMain()}</div>
        )}

        {/* Search Everywhere (double-shift) for the focused workspace */}
      {searchOpen && focusWorkspace && (
        <SearchEverywhereModal
          workspace={focusWorkspace}
          onOpenFile={(repo, path) => {
            setNavPage({ kind: "dashboard" });
            openTab({ kind: "editor", workspace: focusWorkspace.name, repo, path });
          }}
          onClose={() => setSearchOpen(false)}
        />
      )}

      {findOpen && focusWorkspace && (
        <FindPopup workspace={focusWorkspace} onOpen={openMatch(focusWorkspace.name)} onClose={() => setFindOpen(false)} />
      )}

      {menu && (
          <ContextMenu
            x={menu.x}
            y={menu.y}
            items={sidebarMenuItems(menu.payload)}
            onClose={() => setMenu(null)}
          />
        )}
      {tabMenu.menu && (
        <ContextMenu
          x={tabMenu.menu.x}
          y={tabMenu.menu.y}
          items={tabMenuItems(tabMenu.menu.payload)}
          onClose={() => tabMenu.setMenu(null)}
        />
      )}
      {plusMenu && focusWorkspace && (
        <ContextMenu
          x={plusMenu.x}
          y={plusMenu.y}
          items={[
            { label: "Terminal", onSelect: () => openShellTerminal(focusWorkspace.name) },
            { label: "Claude Code", onSelect: () => newTerminal(focusWorkspace.name, "claude", "claude") },
            { label: "Open Code", onSelect: () => newTerminal(focusWorkspace.name, "opencode", "opencode") },
            { label: "OMP", onSelect: () => newTerminal(focusWorkspace.name, "omp", "omp") },
          ]}
          onClose={() => setPlusMenu(null)}
        />
      )}
      </main>

      {/* Fixed right dock, full height, visible while a workspace is in focus.
          Width animates to 0 when hidden; the reveal strip stays clickable. */}
      {focusWorkspace && (
        <>
          {!dockHidden && (
            <div
              className="dock-resizer"
              onMouseDown={() => {
                const onMove = (e: MouseEvent) => {
                  onDockResize(Math.min(460, Math.max(180, window.innerWidth - e.clientX)));
                };
                const onUp = () => {
                  window.removeEventListener("mousemove", onMove);
                  window.removeEventListener("mouseup", onUp);
                  document.body.style.cursor = "";
                };
                document.body.style.cursor = "col-resize";
                window.addEventListener("mousemove", onMove);
                window.addEventListener("mouseup", onUp);
              }}
            />
          )}
          <aside
            className={`dock ${dockHidden ? "dock-hidden" : ""} ${dockAnimating ? "dock-anim" : ""}`}
            onTransitionEnd={() => setDockAnimating(false)}
            style={{ width: dockHidden ? 0 : dockWidth }}
          >
            <FileTreePanel
              workspace={focusWorkspace}
              onOpenFile={(repo, path) => openFileTab(focusWorkspace.name, repo, path)}
              onReviewFile={(repo, path) => {
                setNavPage({ kind: "dashboard" });
                openTab({ kind: "review", workspace: focusWorkspace.name, repo, path });
              }}
              onOpenFind={() => setFindOpen(true)}
              onReviewCommit={(repo, commit) => {
                setNavPage({ kind: "dashboard" });
                openTab({ kind: "commit", workspace: focusWorkspace.name, repo, commit });
              }}
              onError={setError}
            />
          </aside>
        </>
      )}
      </div>

      {/* AI usage bottom bar for the focused workspace */}
      {focusWorkspace && <UsageBar workspace={focusWorkspace.name} />}

      {/* App-owned tooltip (replaces native title hints) */}
      <TooltipHost />
      <ToastHost />
    </div>
  );
}

export default App;