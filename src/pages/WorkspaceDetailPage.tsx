import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RepoStatus, Workspace } from "../types/config";
import { ConfirmDialog } from "../components/ConfirmDialog";
import { SkeletonTable } from "../components/Skeleton";
import { PlanModal } from "../components/PlanModal";
import { GrillModal } from "../components/GrillModal";
import { PlanProgress } from "../components/PlanProgress";

interface Props {
  workspace: Workspace;
  onOpenEditor: (repo: string) => void;
  onOpenPlan: () => void;
  onRemoved: () => void;
  onError: (msg: string) => void;
}

export function WorkspaceDetailPage({ workspace, onOpenEditor, onOpenPlan, onRemoved, onError }: Props) {
  const [statuses, setStatuses] = useState<RepoStatus[] | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<null | "normal" | "force">(null);
  const [planOpen, setPlanOpen] = useState(false);
  const [grillOpen, setGrillOpen] = useState(false);
  const [planExists, setPlanExists] = useState<boolean | null>(null);

  // Does a PLAN.md already exist for this workspace?
  useEffect(() => {
    let cancelled = false;
    invoke<boolean>("workspace_plan_exists", { name: workspace.name })
      .then((v) => !cancelled && setPlanExists(v))
      .catch(() => !cancelled && setPlanExists(false));
    return () => {
      cancelled = true;
    };
  }, [workspace.name]);

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
    <div className="page ws-detail">
      <header className="detail-header">
        <div className="detail-title-row">
          <h2>{workspace.name}</h2>
          <div className="detail-actions">
            {workspace.card && (
              <>
                <button
                  className="secondary"
                  title={planExists ? "Open the generated PLAN.md" : "Generate a PLAN.md from the linked card"}
                  onClick={() => (planExists ? onOpenPlan() : setPlanOpen(true))}
                >
                  {planExists ? "Open plan" : "✳ Plan"}
                </button>
                {planExists && (
                  <button
                    className="secondary"
                    title="Discard the current PLAN.md and generate a new one"
                    onClick={() => setPlanOpen(true)}
                  >
                    ↻ Replan
                  </button>
                )}
              </>
            )}
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
          {workspace.card && (
            <a
              className="tag tag-info plan-card-tag"
              href={workspace.card.url}
              title={workspace.card.title}
              onClick={(e) => {
                e.preventDefault();
                import("@tauri-apps/plugin-opener")
                  .then(({ openUrl }) => openUrl(workspace.card!.url))
                  .catch(() => null);
              }}
            >
              {workspace.card.id}
            </a>
          )}
        </div>
      </header>

      <PlanProgress
        workspace={workspace.name}
        refreshKey={statuses === null ? 0 : 1}
        onOpenPlan={onOpenPlan}
        onError={onError}
      />

      <div className="table-wrap">
        <table className="list">
          <thead>
            <tr>
              <th style={{ width: "28%" }}>Repository</th>
              <th style={{ width: "24%" }}>Branch</th>
              <th style={{ width: "14%" }}>Changes</th>
              <th style={{ width: "10%" }}>Ahead</th>
              <th style={{ width: "10%" }}>Behind</th>
            </tr>
          </thead>
          <tbody>
            {(statuses ?? []).map((st) => (
              <tr
                key={st.repo}
                className="ws-repo-row"
                onClick={() => onOpenEditor(st.repo)}
                title="Click to browse files"
              >
                <td className="cell-name">{st.repo}</td>
                <td>
                  <span className="mono">{st.branch ?? "—"}</span>
                </td>
                <td>
                  {st.dirty ? <span className="tag tag-warn">dirty</span> : <span className="tag tag-ok">clean</span>}
                </td>
                <td>{st.ahead > 0 ? <span className="tag tag-info">↑{st.ahead}</span> : <span className="tag tag-muted">—</span>}</td>
                <td>{st.behind > 0 ? <span className="tag tag-warn">↓{st.behind}</span> : <span className="tag tag-muted">—</span>}</td>
              </tr>
            ))}
            {statuses === null && (
              <tr>
                <td colSpan={5} className="table-skeleton-cell">
                  <SkeletonTable rows={Math.max(2, workspace.repos.length)} cols={5} />
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

      {grillOpen && workspace.card && (
        <GrillModal
          workspace={workspace}
          card={workspace.card}
          onPlanReady={() => setPlanExists(true)}
          onClose={() => setGrillOpen(false)}
          onError={onError}
        />
      )}

      {planOpen && workspace.card && (
        <PlanModal
          workspace={workspace}
          card={workspace.card}
          onOpenPlan={() => {
            setPlanOpen(false);
            onOpenPlan();
          }}
          onStartInterview={() => setGrillOpen(true)}
          onClose={() => {
            setPlanOpen(false);
            setPlanExists(true);
          }}
          onError={onError}
        />
      )}
    </div>
  );
}