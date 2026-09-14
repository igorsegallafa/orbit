import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config } from "../types/config";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

export function GroupsPage({ config, onChange, onError }: Props) {
  const [newGroup, setNewGroup] = useState("");

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
    if (!confirm(`Remove group '${name}'?`)) return;
    try {
      const cfg = await invoke<Config>("delete_group", { name });
      onChange(cfg);
    } catch (e) {
      onError(String(e));
    }
  };

  const toggleMember = async (group: string, service: string, checked: boolean) => {
    const current = config.groups[group] ?? [];
    const members = checked ? [...current, service] : current.filter((m) => m !== service);
    try {
      const cfg = await invoke<Config>("set_group_members", { name: group, members });
      onChange(cfg);
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div className="page">
      <h2>Groups</h2>

      <form
        className="card form-row"
        onSubmit={(e) => {
          e.preventDefault();
          createGroup();
        }}
      >
        <input value={newGroup} onChange={(e) => setNewGroup(e.target.value)} placeholder="Group name" />
        <button type="submit">Create group</button>
      </form>

      {config.services.length === 0 && <p className="empty">Add repositories before creating groups.</p>}

      {Object.keys(config.groups).map((group) => (
        <div className="card" key={group}>
          <div className="card-header">
            <h3>{group}</h3>
            <button className="link danger" onClick={() => deleteGroup(group)}>
              remove group
            </button>
          </div>
          <div className="chip-list">
            {config.services.map((s) => {
              const checked = (config.groups[group] ?? []).includes(s.name);
              return (
                <label key={s.name} className={`chip chip-toggle ${checked ? "chip-active" : ""}`}>
                  <input
                    type="checkbox"
                    checked={checked}
                    onChange={(e) => toggleMember(group, s.name, e.target.checked)}
                  />
                  {s.name}
                </label>
              );
            })}
          </div>
        </div>
      ))}

      {Object.keys(config.groups).length === 0 && config.services.length > 0 && (
        <p className="empty">No groups created yet.</p>
      )}
    </div>
  );
}
