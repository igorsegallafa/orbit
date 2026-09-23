import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config } from "../types/config";
import { ConfirmDialog } from "./ConfirmDialog";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

export function GroupsSection({ config, onChange, onError }: Props) {
  const [newGroup, setNewGroup] = useState("");
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const [editingGroup, setEditingGroup] = useState<string | null>(null);
  const [draftMembers, setDraftMembers] = useState<string[]>([]);

  const createGroup = async () => {
    const name = newGroup.trim();
    if (!name) return;
    try {
      const cfg = await invoke<Config>("create_group", { name });
      onChange(cfg);
      setNewGroup("");
    } catch (e) {
      onError(String(e));
    }
  };

  const deleteGroup = async (name: string) => {
    try {
      const cfg = await invoke<Config>("delete_group", { name });
      onChange(cfg);
      setConfirmRemove(null);
    } catch (e) {
      onError(String(e));
    }
  };

  const openEdit = (group: string) => {
    setEditingGroup(group);
    setDraftMembers(config.groups[group] ?? []);
  };

  const toggleDraftMember = (service: string) => {
    setDraftMembers((m) =>
      m.includes(service) ? m.filter((x) => x !== service) : [...m, service]
    );
  };

  const saveMembers = async () => {
    if (!editingGroup) return;
    try {
      const cfg = await invoke<Config>("set_group_members", {
        name: editingGroup,
        members: draftMembers,
      });
      onChange(cfg);
      setEditingGroup(null);
    } catch (e) {
      onError(String(e));
    }
  };

  const groups = Object.keys(config.groups).sort((a, b) => a.localeCompare(b));

  return (
    <div className="section">
      <div className="section-header">
        <h3>Groups</h3>
      </div>

      <form
        className="card form-row"
        onSubmit={(e) => {
          e.preventDefault();
          createGroup();
        }}
      >
        <input value={newGroup} onChange={(e) => setNewGroup(e.target.value)} placeholder="New group name" />
        <button type="submit">Create group</button>
      </form>

      {groups.length === 0 ? (
        <div className="empty-state small">
          <p>
            {config.services.length === 0
              ? "Add repositories first, then group them to reuse selections when creating workspaces."
              : "No groups yet. Create one above to bundle repositories you often change together."}
          </p>
        </div>
      ) : (
        <div className="group-list">
          {groups.map((group) => {
            const members = config.groups[group] ?? [];
            return (
              <div className="group-row card" key={group}>
                <div className="group-info">
                  <div className="group-name">{group}</div>
                  <div className="group-members">
                    {members.length === 0 ? (
                      <span className="tag tag-muted">empty</span>
                    ) : (
                      members.map((m) => (
                        <span key={m} className="tag">
                          {m}
                        </span>
                      ))
                    )}
                  </div>
                </div>
                <div className="row-actions">
                  <button className="link" onClick={() => openEdit(group)}>
                    Edit
                  </button>
                  <button className="link danger" onClick={() => setConfirmRemove(group)}>
                    Remove
                  </button>
                </div>
              </div>
            );
          })}
        </div>
      )}

      {confirmRemove && (
        <ConfirmDialog
          title="Remove group"
          message={`Remove group '${confirmRemove}'?`}
          confirmLabel="Remove"
          danger
          onConfirm={() => deleteGroup(confirmRemove)}
          onClose={() => setConfirmRemove(null)}
        />
      )}

      {editingGroup && (
        <div className="modal-overlay" onMouseDown={() => setEditingGroup(null)}>
          <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
            <div className="modal-header">
              <h3>Edit group “{editingGroup}”</h3>
            </div>
            <div className="modal-search">
              <input
                autoFocus
                placeholder={`${draftMembers.length} of ${config.services.length} repositories selected`}
                disabled
              />
            </div>
            <div className="modal-list">
              {config.services.map((s) => {
                const checked = draftMembers.includes(s.name);
                return (
                  <label key={s.name} className={`modal-row modal-row-check ${checked ? "modal-row-selected" : ""}`}>
                    <input
                      type="checkbox"
                      checked={checked}
                      onChange={() => toggleDraftMember(s.name)}
                    />
                    <span className="modal-row-name">{s.name}</span>
                  </label>
                );
              })}
              {config.services.length === 0 && (
                <p className="empty">No repositories to add. Configure them in the Repositories tab first.</p>
              )}
            </div>
            <div className="modal-footer">
              <button type="button" className="secondary" onClick={() => setEditingGroup(null)}>
                Cancel
              </button>
              <button type="button" onClick={saveMembers}>
                Save
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}