import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Config, RepoStatus, Workspace } from "../types/config";
import { WorkspaceCreateModal } from "../components/WorkspaceCreateModal";
import { ContextMenu, MenuItem, useContextMenu } from "../components/ContextMenu";
import { CodeViewIcon, FolderIcon, PlusIcon, SatelliteIcon, SparkIcon } from "../components/Icons";
import { tooltip } from "../components/Tooltip";
import { useRalphRunning } from "../lib/useRalphRunning";

interface Props {
  config: Config;
  workspaces: Workspace[];
  onOpen: (ws: Workspace) => void;
  onOpenRalph: (ws: Workspace) => void;
  onGoToSettings: () => void;
  onChanged: () => void;
  onError: (msg: string) => void;
}

export function DashboardPage({ config, workspaces, onOpen, onOpenRalph, onGoToSettings, onChanged, onError }: Props) {
  const [showCreate, setShowCreate] = useState(false);
  const [statuses, setStatuses] = useState<Record<string, RepoStatus[] | "error">>({});
  const { menu, setMenu, openFromEvent } = useContextMenu<Workspace>();
  const ralphRunning = useRalphRunning();
  const repoCount = config.services.length;

  useEffect(() => {
    let cancelled = false;
    for (const ws of workspaces) {
      invoke<RepoStatus[]>("workspace_status", { name: ws.name })
        .then((st) => !cancelled && setStatuses((p) => ({ ...p, [ws.name]: st })))
        .catch(() => !cancelled && setStatuses((p) => ({ ...p, [ws.name]: "error" })));
    }
    return () => {
      cancelled = true;
    };
  }, [workspaces]);

  const openInEditor = (ws: Workspace) =>
    invoke("open_workspace_in_editor", { name: ws.name }).catch((e) => onError(String(e)));
  const revealFolder = (ws: Workspace) =>
    invoke("reveal_workspace_folder", { name: ws.name }).catch((e) => onError(String(e)));

  const menuItems = (ws: Workspace): MenuItem[] => [
    { label: "Open workspace", onSelect: () => onOpen(ws) },
    { label: "Open Ralph", onSelect: () => onOpenRalph(ws) },
    { label: "Open in editor", onSelect: () => openInEditor(ws) },
    { label: "Open folder", onSelect: () => revealFolder(ws) },
  ];

  const hint = (text: string) => ({
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  });

  return (
    <div className="page dash">
      <div className="page-header">
        <div>
          <h2>Workspaces</h2>
          <span className="dash-sub">
            {workspaces.length} workspace{workspaces.length === 1 ? "" : "s"} · {repoCount} repositor{repoCount === 1 ? "y" : "ies"}
          </span>
        </div>
        {repoCount > 0 && (
          <button onClick={() => setShowCreate(true)}>
            <PlusIcon size={14} /> New workspace
          </button>
        )}
      </div>

      {repoCount === 0 ? (
        <div className="dash-onboard">
          <span className="dash-onboard-icon">
            <SatelliteIcon size={26} />
          </span>
          <h3>Welcome to Orbit</h3>
          <p>Work on one feature across several repositories at once: every workspace is a set of git worktrees on the same branch.</p>
          <ol className="rv-steps">
            <li>
              <strong>Add your repositories</strong>
              <span>From GitHub or any git URL. Orbit clones each one once.</span>
            </li>
            <li>
              <strong>Create a workspace</strong>
              <span>Pick the repos and a branch, or start from a Linear or Shortcut card.</span>
            </li>
            <li>
              <strong>Ship it</strong>
              <span>Commit, push and open PRs across repos together, or let Ralph implement a PRD.</span>
            </li>
          </ol>
          <button onClick={onGoToSettings}>Add repositories</button>
        </div>
      ) : workspaces.length === 0 ? (
        <div className="dash-onboard">
          <span className="dash-onboard-icon">
            <SatelliteIcon size={26} />
          </span>
          <h3>No workspaces yet</h3>
          <p>Create one to start a feature: Orbit sets up a worktree per repository, all on the same branch.</p>
          <button onClick={() => setShowCreate(true)}>
            <PlusIcon size={14} /> New workspace
          </button>
        </div>
      ) : (
        <div className="dash-grid">
          {workspaces.map((ws) => {
            const st = statuses[ws.name];
            const rows = Array.isArray(st) ? st : null;
            const dirty = rows?.filter((r) => r.dirty).length ?? 0;
            const ahead = rows?.reduce((a, r) => a + r.ahead, 0) ?? 0;
            const behind = rows?.reduce((a, r) => a + r.behind, 0) ?? 0;
            const running = ralphRunning.has(ws.name);
            return (
              <div
                key={ws.name}
                className={`dash-card ${running ? "running" : ""}`}
                role="button"
                tabIndex={0}
                onClick={() => onOpen(ws)}
                onKeyDown={(e) => e.key === "Enter" && onOpen(ws)}
                onContextMenu={(e) => {
                  e.preventDefault();
                  setMenu({ x: e.clientX, y: e.clientY, payload: ws });
                }}
                onMouseDown={(e) => openFromEvent(e, ws)}
              >
                <div className="dash-card-head">
                  <span className="dash-card-icon">
                    <SatelliteIcon size={15} />
                  </span>
                  <div className="dash-card-title">
                    <span className="dash-card-name">{ws.name}</span>
                    <span className="dash-card-branch">{ws.branch}</span>
                  </div>
                  {running && (
                    <button
                      className="dash-ralph"
                      onClick={(e) => {
                        e.stopPropagation();
                        onOpenRalph(ws);
                      }}
                      {...hint("Ralph is running · open")}
                    >
                      <span className="rv-dot" /> Ralph
                    </button>
                  )}
                </div>

                <div className="dash-repos">
                  {ws.repos.map((repo) => {
                    const r = rows?.find((x) => x.repo === repo);
                    const cls = !rows ? "" : r?.dirty ? "dirty" : r && r.ahead > 0 ? "ahead" : "clean";
                    return (
                      <span key={repo} className={`dash-repo ${cls}`}>
                        <span className="dash-repo-dot" />
                        {repo}
                        {r && r.ahead > 0 && <span className="dash-repo-n">↑{r.ahead}</span>}
                      </span>
                    );
                  })}
                </div>

                <div className="dash-card-foot">
                  <span className="dash-summary">
                    {st === undefined ? (
                      <span className="dash-muted">Checking status…</span>
                    ) : st === "error" ? (
                      <span className="dash-muted">Status unavailable</span>
                    ) : dirty === 0 && ahead === 0 && behind === 0 ? (
                      <span className="dash-clean">Up to date</span>
                    ) : (
                      <>
                        {dirty > 0 && <span className="dash-warn">{dirty} with changes</span>}
                        {ahead > 0 && <span>{ahead} to push</span>}
                        {behind > 0 && <span className="dash-muted">{behind} behind</span>}
                      </>
                    )}
                  </span>
                  {ws.card && (
                    <button
                      className="dash-card-link"
                      onClick={(e) => {
                        e.stopPropagation();
                        openUrl(ws.card!.url).catch(() => null);
                      }}
                      {...hint(ws.card.title)}
                    >
                      {ws.card.id}
                    </button>
                  )}
                  <span className="dash-actions" onClick={(e) => e.stopPropagation()}>
                    <button className="icon-button" aria-label="Ralph" {...hint("Ralph")} onClick={() => onOpenRalph(ws)}>
                      <SparkIcon size={13} />
                    </button>
                    <button className="icon-button" aria-label="Open in editor" {...hint("Open in editor")} onClick={() => openInEditor(ws)}>
                      <CodeViewIcon size={13} />
                    </button>
                    <button className="icon-button" aria-label="Open folder" {...hint("Open folder")} onClick={() => revealFolder(ws)}>
                      <FolderIcon size={13} />
                    </button>
                  </span>
                </div>
              </div>
            );
          })}
          <button className="dash-new" onClick={() => setShowCreate(true)}>
            <PlusIcon size={16} />
            New workspace
          </button>
        </div>
      )}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menuItems(menu.payload)} onClose={() => setMenu(null)} />}

      {showCreate && (
        <WorkspaceCreateModal config={config} onCreated={onChanged} onClose={() => setShowCreate(false)} onError={onError} />
      )}
    </div>
  );
}
