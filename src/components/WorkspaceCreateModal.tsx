import { useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, Workspace } from "../types/config";

interface Props {
  config: Config;
  onCreated: () => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

function slugify(s: string): string {
  return s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function WorkspaceCreateModal({ config, onCreated, onClose, onError }: Props) {
  const [name, setName] = useState("");
  const [branch, setBranch] = useState("");
  const [base, setBase] = useState("main");
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [creating, setCreating] = useState(false);

  const effectiveBranch = useMemo(() => {
    if (branch.trim()) return branch.trim();
    const slug = slugify(name);
    return slug ? `feat/${slug}` : "";
  }, [branch, name]);

  const toggleRepo = (repo: string) => {
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(repo)) next.delete(repo);
      else next.add(repo);
      return next;
    });
  };

  const toggleGroup = (group: string) => {
    const members = config.groups[group] ?? [];
    const allIn = members.every((m) => selected.has(m));
    setSelected((s) => {
      const next = new Set(s);
      for (const m of members) {
        if (allIn) next.delete(m);
        else next.add(m);
      }
      return next;
    });
  };

  const create = async () => {
    if (!name.trim() || selected.size === 0 || !effectiveBranch) {
      onError("Name and at least one repository are required.");
      return;
    }
    setCreating(true);
    try {
      await invoke<Workspace>("create_workspace", {
        name: name.trim(),
        branch: effectiveBranch,
        base: base.trim() || "main",
        repos: Array.from(selected),
      });
      onCreated();
      onClose();
    } catch (e) {
      onError(String(e));
    } finally {
      setCreating(false);
    }
  };

  const hasGroups = Object.keys(config.groups).length > 0;

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal modal-wizard" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>New workspace</h3>
        </div>

        <div className="wizard-body">
          <div className="wizard-field">
            <label>Workspace name</label>
            <input
              autoFocus
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="checkout-improvements"
            />
          </div>

          <div className="wizard-row">
            <div className="wizard-field">
              <label>Branch</label>
              <input
                value={branch}
                onChange={(e) => setBranch(e.target.value)}
                placeholder={effectiveBranch || "feat/my-workspace"}
              />
            </div>
            <div className="wizard-field">
              <label>Base branch</label>
              <input value={base} onChange={(e) => setBase(e.target.value)} placeholder="main" />
            </div>
          </div>

          <div className="wizard-field">
            <label>
              Repositories
              <span className="wizard-count">{selected.size} selected</span>
            </label>
            {hasGroups && (
              <div className="wizard-chips">
                {Object.keys(config.groups).map((g) => {
                  const members = config.groups[g] ?? [];
                  const allIn = members.length > 0 && members.every((m) => selected.has(m));
                  return (
                    <button
                      type="button"
                      key={g}
                      className={`chip ${allIn ? "chip-active" : ""}`}
                      onClick={() => toggleGroup(g)}
                    >
                      {g}
                    </button>
                  );
                })}
              </div>
            )}
            <div className="wizard-picker">
              {config.services.map((s) => {
                const checked = selected.has(s.name);
                return (
                  <label key={s.name} className={`modal-row modal-row-check ${checked ? "modal-row-selected" : ""}`}>
                    <input type="checkbox" checked={checked} onChange={() => toggleRepo(s.name)} />
                    <span className="modal-row-name">{s.name}</span>
                  </label>
                );
              })}
              {config.services.length === 0 && (
                <p className="empty">No repositories configured. Add some in Settings first.</p>
              )}
            </div>
          </div>
        </div>

        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            disabled={creating || !name.trim() || selected.size === 0}
            onClick={create}
          >
            {creating ? "Creating…" : "Create workspace"}
          </button>
        </div>
      </div>
    </div>
  );
}