import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, GithubRepo, Service } from "../types/config";
import { GithubRepoModal } from "./GithubRepoModal";
import { ConfirmDialog } from "./ConfirmDialog";
import { Skeleton } from "./Skeleton";
import { FoldersCard, Folders } from "./FoldersCard";
import { RepoFormModal } from "./RepoFormModal";
import { toast } from "./Toast";
import { tooltip } from "./Tooltip";
import { DownloadIcon, FolderIcon, GitIcon, PencilIcon, PlusIcon, TrashIcon } from "./Icons";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

type CloneState = "unknown" | "cloned" | "missing" | "cloning";

export function ReposSection({ config, onChange, onError }: Props) {
  const [clone, setClone] = useState<Record<string, CloneState>>({});
  const [folders, setFolders] = useState<Folders | null>(null);
  const [modal, setModal] = useState<null | { mode: "add" } | { mode: "edit"; service: Service }>(null);
  const [showGithub, setShowGithub] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<Service | null>(null);

  useEffect(() => {
    invoke<Folders>("get_folders").then(setFolders).catch(() => null);
  }, [config.reposDir]);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      for (const s of config.services) {
        if (clone[s.name] && clone[s.name] !== "unknown") continue;
        try {
          const cloned = await invoke<boolean>("service_clone_status", { name: s.name });
          if (!cancelled) setClone((p) => ({ ...p, [s.name]: cloned ? "cloned" : "missing" }));
        } catch {
          // status stays unknown
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config.services]);

  const cloneRepo = async (name: string) => {
    setClone((p) => ({ ...p, [name]: "cloning" }));
    const id = toast.loading(`Cloning ${name}…`);
    try {
      await invoke("clone_service", { name });
      setClone((p) => ({ ...p, [name]: "cloned" }));
      toast.update(id, "success", `${name} cloned`, {
        action: { label: "Open folder", onClick: () => invoke("reveal_service", { name }).catch((e) => onError(String(e))) },
      });
    } catch (e) {
      setClone((p) => ({ ...p, [name]: "missing" }));
      toast.update(id, "error", `Couldn't clone ${name}`, { description: String(e) });
    }
  };

  const afterAdd = (cfg: Config, name: string, cloneNow: boolean) => {
    onChange(cfg);
    setClone((p) => ({ ...p, [name]: "unknown" }));
    if (cloneNow) {
      cloneRepo(name);
    } else {
      toast.success(`${name} added`, { action: { label: "Clone now", onClick: () => cloneRepo(name) } });
    }
  };

  const addFromGithub = async (repo: GithubRepo, path: string | null, cloneNow: boolean) => {
    // HTTPS: the gh login authenticates it; SSH would need a key set up.
    const cfg = await invoke<Config>("add_service", { name: repo.name, repo: `https://github.com/${repo.nameWithOwner}.git`, path });
    afterAdd(cfg, repo.name, cloneNow);
  };

  const remove = async (svc: Service) => {
    try {
      onChange(await invoke<Config>("remove_service", { name: svc.name }));
      setConfirmRemove(null);
      toast.success(`${svc.name} removed`, { description: svc.path ? "Its folder was kept on disk." : undefined });
    } catch (e) {
      onError(String(e));
    }
  };

  const hint = (text: string) => ({
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  });

  return (
    <div className="section">
      <div className="section-header">
        <h3>Folders</h3>
      </div>
      <FoldersCard config={config} onChange={onChange} onError={onError} />

      <div className="section-header">
        <h3>
          Repositories <span className="section-count">{config.services.length}</span>
        </h3>
        <div className="section-actions">
          <button className="secondary" onClick={() => setModal({ mode: "add" })}>
            <PlusIcon size={14} /> Add manually
          </button>
          <button onClick={() => setShowGithub(true)}>
            <GitIcon size={14} /> Add from GitHub
          </button>
        </div>
      </div>

      {config.services.length === 0 ? (
        <div className="repo-empty">
          <span className="repo-empty-icon">
            <GitIcon size={20} />
          </span>
          <strong>No repositories yet</strong>
          <span>Add the repositories your workspaces will span. Orbit clones them once and creates worktrees per workspace.</span>
          <button onClick={() => setShowGithub(true)}>Add from GitHub</button>
        </div>
      ) : (
        <div className="repo-list">
          {config.services.map((s) => {
            const st = clone[s.name] ?? "unknown";
            return (
              <div key={s.name} className="repo-row" onDoubleClick={() => setModal({ mode: "edit", service: s })}>
                <span className="repo-icon">
                  <GitIcon size={15} />
                </span>
                <div className="repo-main">
                  <div className="repo-title">
                    <span className="repo-name">{s.name}</span>
                    {s.build && <span className="repo-tag">build</span>}
                    {s.worktree === false && <span className="repo-tag">branch in base clone</span>}
                  </div>
                  <div className="repo-sub" title={s.repo}>
                    {s.repo}
                  </div>
                  {s.path && (
                    <div className="repo-sub repo-sub-path" title={s.path}>
                      <FolderIcon size={11} /> {s.path}
                    </div>
                  )}
                </div>

                <div className="repo-status">
                  {st === "unknown" ? (
                    <Skeleton w={70} h={12} rounded={999} />
                  ) : st === "cloned" ? (
                    <span className="repo-state repo-state-ok">
                      <span className="repo-dot" /> Cloned
                    </span>
                  ) : st === "cloning" ? (
                    <span className="repo-state">
                      <span className="spinner" /> Cloning…
                    </span>
                  ) : (
                    <button className="secondary repo-clone-btn" onClick={() => cloneRepo(s.name)}>
                      <DownloadIcon size={13} /> Clone
                    </button>
                  )}
                </div>

                <div className="repo-actions">
                  <button
                    className="icon-button"
                    disabled={st !== "cloned"}
                    aria-label="Open folder"
                    {...hint("Open folder")}
                    onClick={() => invoke("reveal_service", { name: s.name }).catch((e) => onError(String(e)))}
                  >
                    <FolderIcon size={14} />
                  </button>
                  <button className="icon-button" aria-label="Edit" {...hint("Edit")} onClick={() => setModal({ mode: "edit", service: s })}>
                    <PencilIcon size={14} />
                  </button>
                  <button
                    className="icon-button icon-button-danger"
                    aria-label="Remove"
                    {...hint("Remove")}
                    onClick={() => setConfirmRemove(s)}
                  >
                    <TrashIcon size={14} />
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {modal && (
        <RepoFormModal
          service={modal.mode === "edit" ? modal.service : undefined}
          cloned={modal.mode === "edit" && clone[modal.service.name] === "cloned"}
          folders={folders}
          onSaved={(cfg, info) => {
            if (info.added) afterAdd(cfg, info.name, info.cloneNow);
            else {
              onChange(cfg);
              toast.success(`${info.name} saved`);
            }
          }}
          onClose={() => setModal(null)}
          onError={onError}
        />
      )}

      {showGithub && (
        <GithubRepoModal
          knownNames={new Set(config.services.map((s) => s.name))}
          folders={folders}
          onAdd={addFromGithub}
          onClose={() => setShowGithub(false)}
          onError={onError}
        />
      )}

      {confirmRemove && (
        <ConfirmDialog
          title={`Remove ${confirmRemove.name}?`}
          message={
            confirmRemove.path
              ? "It leaves Orbit's list. The folder at its custom path is kept on disk."
              : "It leaves Orbit's list and its clone in the clones folder is deleted."
          }
          confirmLabel="Remove"
          danger
          onConfirm={() => remove(confirmRemove)}
          onClose={() => setConfirmRemove(null)}
        />
      )}
    </div>
  );
}
