import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, Workspace } from "../types/config";
import { WorkspaceCreateModal } from "../components/WorkspaceCreateModal";
import { ContextMenu, MenuItem, useContextMenu } from "../components/ContextMenu";

interface Props {
  config: Config;
  workspaces: Workspace[];
  onOpen: (ws: Workspace) => void;
  onChanged: () => void;
  onError: (msg: string) => void;
}

export function DashboardPage({ config, workspaces, onOpen, onChanged, onError }: Props) {
  const [showCreate, setShowCreate] = useState(false);
  const { menu, setMenu, openFromEvent } = useContextMenu<Workspace>();

  const repoCount = config.services.length;
  const groupCount = Object.keys(config.groups).length;
  const freshSetup = repoCount === 0 && workspaces.length === 0;

  const menuItems = (ws: Workspace): MenuItem[] => [
    {
      label: "Open workspace",
      onSelect: () => onOpen(ws),
    },
    {
      label: "Open in editor",
      onSelect: () =>
        invoke("open_workspace_in_editor", { name: ws.name }).catch((e) => onError(String(e))),
    },
    {
      label: "Open folder in Finder",
      onSelect: () =>
        invoke("reveal_workspace_folder", { name: ws.name }).catch((e) => onError(String(e))),
    },
  ];

  return (
    <div className="page">
      <div className="page-header">
        <h2>Workspaces</h2>
        <button disabled={repoCount === 0} onClick={() => setShowCreate(true)}>
          + New workspace
        </button>
      </div>

      {freshSetup ? (
        <div className="empty-state">
          <div className="empty-state-icon">🛰️</div>
          <h3>Welcome to Orbit</h3>
          <p>
            A workspace is a set of git worktrees — one per repo — for working on a feature
            across multiple repositories in parallel. Start by adding your repositories in
            Settings.
          </p>
        </div>
      ) : workspaces.length === 0 ? (
        <div className="empty-state">
          <div className="empty-state-icon">🛰️</div>
          <h3>No workspaces yet</h3>
          <p>
            Create one to start working on a feature: Orbit will set up a git worktree per
            selected repository, all on the same branch.
          </p>
          <div className="empty-state-actions">
            <button onClick={() => setShowCreate(true)}>+ New workspace</button>
          </div>
        </div>
      ) : (
        <div className="workspace-grid">
          {workspaces.map((ws) => (
            <button
              key={ws.name}
              className="workspace-card"
              onClick={() => onOpen(ws)}
              onContextMenu={(e) => {
                e.preventDefault();
                setMenu({ x: e.clientX, y: e.clientY, payload: ws });
              }}
              onMouseDown={(e) => openFromEvent(e, ws)}
              onPointerDown={(e) => openFromEvent(e, ws)}
            >
              <div className="workspace-card-head">
                <span className="workspace-card-icon">🛰️</span>
                <span className="workspace-card-name">{ws.name}</span>
              </div>
              <div className="workspace-card-meta">
                <span className="tag">{ws.branch}</span>
                <span className="tag tag-muted">{ws.repos.length} repos</span>
              </div>
            </button>
          ))}
        </div>
      )}

      <div className="stats-row">
        <div className="stat-card">
          <span className="stat-value">{workspaces.length}</span>
          <span className="stat-label">workspaces</span>
        </div>
        <div className="stat-card">
          <span className="stat-value">{repoCount}</span>
          <span className="stat-label">repositories</span>
        </div>
        <div className="stat-card">
          <span className="stat-value">{groupCount}</span>
          <span className="stat-label">groups</span>
        </div>
      </div>

      {menu && (
        <ContextMenu
          x={menu.x}
          y={menu.y}
          items={menuItems(menu.payload)}
          onClose={() => setMenu(null)}
        />
      )}

      {showCreate && (
        <WorkspaceCreateModal
          config={config}
          onCreated={onChanged}
          onClose={() => setShowCreate(false)}
          onError={onError}
        />
      )}
    </div>
  );
}