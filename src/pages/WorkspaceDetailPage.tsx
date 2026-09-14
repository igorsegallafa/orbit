import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RepoStatus, Workspace } from "../types/config";
import { ConfirmDialog } from "../components/ConfirmDialog";

interface Props {
  workspace: Workspace;
  onRemoved: () => void;
  onBack: () => void;
  onError: (msg: string) => void;
}

export function WorkspaceDetailPage({ workspace, onRemoved, onBack, onError }: Props) {
  const [statuses, setStatuses] = useState<RepoStatus[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<null | "normal" | "force">(null);

  const load = useCallback(
    async (fetch: boolean) => {
      setRefreshing(true);
      try {
        if (fetch) {
          await Promise.all(
            workspace.repos.map((repo) => invoke("refresh_repo", { name: repo }).catch(() => null))
          );
        }
        const st = await invoke<RepoStatus[]>("workspace_status", { name: workspace.name });
        setStatuses(st);
      } catch (e) {
        onError(String(e));
      } finally {
        setRefreshing(false);
      }
    },
    [workspace.name, workspace.repos, onError]
  );

  useEffect(() => {
    setStatuses(null);
    load(false);
  }, [load]);

  const remove = async (force: boolean) => {
    try {
      await invoke("remove_workspace", { name: workspace.name, force });
      setConfirmRemove(null);
      onRemoved();
    } catch (e) {
      const msg = String(e);
      if (!force && msg.includes("uncommitted changes")) {
        setConfirmRemove("force");
      } else {
        onError(msg);
        setConfirmRemove(null);
      }
    }
  };

  return (
    <div className="page">
      <header className="detail-header">
        <button className="back-link" onClick={onBack}>
          ← Workspaces
        </button>
        <div className="detail-title-row">
          <h2>{workspace.name}</h2>
          <div className="detail-actions">
            <button className="secondary" disabled={refreshing} onClick={() => load(true)}>
              {refreshing ? "Refreshing…" : "↻ Refresh"}
            </button>
            <button className="secondary danger-outline" onClick={() => setConfirmRemove("normal")}>
              Remove
            </button>
          </div>
        </div>
        <div className="workspace-meta">
          <span className="tag">{workspace.branch}</span>
          <span className="tag tag-muted">base: {workspace.base}</span>
          <span className="tag tag-muted">{workspace.repos.length} repos</span>
        </div>
      </header>

      <div className="table-wrap">
        <table className="list">
          <thead>
            <tr>
              <th style={{ width: "28%" }}>Repository</th>
              <th style={{ width: "22%" }}>Branch</th>
              <th style={{ width: "12%" }}>Changes</th>
              <th style={{ width: "10%" }}>Ahead</th>
              <th style={{ width: "10%" }}>Behind</th>
              <th style={{ width: "18%" }} />
            </tr>
          </thead>
          <tbody>
            {(statuses ?? []).map((st) => (
              <tr key={st.repo}>
                <td className="cell-name">{st.repo}</td>
                <td>
                  <span className="mono">{st.branch ?? "—"}</span>
                </td>
                <td>
                  {st.dirty ? <span className="tag tag-warn">dirty</span> : <span className="tag tag-ok">clean</span>}
                </td>
                <td>{st.ahead > 0 ? <span className="tag tag-info">↑{st.ahead}</span> : <span className="tag tag-muted">—</span>}</td>
                <td>{st.behind > 0 ? <span className="tag tag-warn">↓{st.behind}</span> : <span className="tag tag-muted">—</span>}</td>
                <td className="row-actions">
                  <button
                    className="link"
                    onClick={() =>
                      invoke("open_in_editor", { workspace: workspace.name, repo: st.repo }).catch((e) =>
                        onError(String(e))
                      )
                    }
                  >
                    Open in editor
                  </button>
                </td>
              </tr>
            ))}
            {statuses === null && (
              <tr>
                <td colSpan={6} className="table-loading">
                  <span className="spinner" /> Loading status…
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {confirmRemove && (
        <ConfirmDialog
          title={confirmRemove === "force" ? "Force remove workspace" : "Remove workspace"}
          message={
            confirmRemove === "force"
              ? "Some worktrees have uncommitted changes. Force remove will discard them permanently. Continue?"
              : `Remove workspace '${workspace.name}'? This deletes its worktrees and the branch '${workspace.branch}' in each repository.`
          }
          confirmLabel={confirmRemove === "force" ? "Force remove" : "Remove"}
          danger
          onConfirm={() => remove(confirmRemove === "force")}
          onClose={() => setConfirmRemove(null)}
        />
      )}
    </div>
  );
}