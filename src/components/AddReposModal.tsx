import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, Workspace } from "../types/config";
import { CheckBox } from "./CheckBox";
import { toast } from "./Toast";

interface Props {
  workspace: Workspace;
  onAdded: () => void;
  onClose: () => void;
}

export function AddReposModal({ workspace, onAdded, onClose }: Props) {
  const [available, setAvailable] = useState<string[] | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());

  useEffect(() => {
    invoke<Config>("get_config")
      .then((cfg) => setAvailable(cfg.services.map((s) => s.name).filter((n) => !workspace.repos.includes(n))))
      .catch(() => setAvailable([]));
  }, [workspace.repos]);

  const toggle = (repo: string) =>
    setSelected((s) => {
      const next = new Set(s);
      if (!next.delete(repo)) next.add(repo);
      return next;
    });

  const add = async () => {
    const repos = Array.from(selected);
    const t = toast.loading(`Adding ${repos.length} repo(s) to ${workspace.name}…`, { description: `On ${workspace.branch}` });
    onClose();
    try {
      await invoke<Workspace>("add_workspace_repos", { name: workspace.name, repos });
      toast.update(t, "success", `Added to ${workspace.name}`, { description: repos.join(", ") });
      onAdded();
    } catch (e) {
      toast.update(t, "error", `Couldn't add repos to ${workspace.name}`, { description: String(e) });
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal modal-wizard" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Add repositories</h3>
        </div>
        <div className="wizard-body">
          <div className="wizard-field">
            <label>
              Repositories
              <span className="wizard-count">{selected.size} selected</span>
            </label>
            <div className="wizard-picker">
              {available?.map((name) => {
                const checked = selected.has(name);
                return (
                  <label key={name} className={`modal-row modal-row-check ${checked ? "modal-row-selected" : ""}`}>
                    <CheckBox label={name} checked={checked} onChange={() => toggle(name)} />
                    <span className="modal-row-name">{name}</span>
                  </label>
                );
              })}
              {available?.length === 0 && <p className="empty">Every configured repository is already in this workspace.</p>}
            </div>
          </div>
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button type="button" disabled={selected.size === 0} onClick={add}>
            Add to workspace
          </button>
        </div>
      </div>
    </div>
  );
}
