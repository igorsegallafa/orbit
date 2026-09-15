import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, GithubRepo, Service } from "../types/config";
import { GithubRepoModal } from "./GithubRepoModal";
import { ConfirmDialog } from "./ConfirmDialog";
import { Skeleton } from "./Skeleton";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

const emptyForm = { name: "", repo: "" };

export function ReposSection({ config, onChange, onError }: Props) {
  const [form, setForm] = useState(emptyForm);
  const [editing, setEditing] = useState<string | null>(null);
  const [cloneStatus, setCloneStatus] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [showGithubModal, setShowGithubModal] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);

  // Check clone status for all repos when the list changes
  useEffect(() => {
    let cancelled = false;
    (async () => {
      for (const s of config.services) {
        if (cloneStatus[s.name] !== undefined) continue;
        try {
          const cloned = await invoke<boolean>("service_clone_status", { name: s.name });
          if (!cancelled) setCloneStatus((prev) => ({ ...prev, [s.name]: cloned }));
        } catch {
          // ignore individual check failures
        }
      }
    })();
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [config.services]);

  const resetForm = () => {
    setForm(emptyForm);
    setEditing(null);
  };

  const startEdit = (s: Service) => {
    setEditing(s.name);
    setForm({ name: s.name, repo: s.repo });
  };

  const submit = async () => {
    if (!form.name.trim() || !form.repo.trim()) {
      onError("Name and repo URL are required.");
      return;
    }
    const args = { name: form.name.trim(), repo: form.repo.trim() };
    try {
      const cmd = editing ? "update_service" : "add_service";
      const cfg = await invoke<Config>(cmd, args);
      onChange(cfg);
      resetForm();
    } catch (e) {
      onError(String(e));
    }
  };

  const remove = async (name: string) => {
    try {
      const cfg = await invoke<Config>("remove_service", { name });
      onChange(cfg);
      setConfirmRemove(null);
    } catch (e) {
      onError(String(e));
    }
  };

  const cloneRepo = async (name: string) => {
    setBusy(name);
    try {
      await invoke("clone_service", { name });
      setCloneStatus((s) => ({ ...s, [name]: true }));
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(null);
    }
  };

  const addFromGithub = async (repo: GithubRepo) => {
    const cfg = await invoke<Config>("add_service", { name: repo.name, repo: repo.sshUrl });
    onChange(cfg);
  };

  return (
    <div className="section">
      <div className="section-header">
        <h3>Repositories</h3>
        <button className="secondary" onClick={() => setShowGithubModal(true)}>
          Add from GitHub…
        </button>
      </div>

      <form
        className="card form"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <div className="form-grid">
          <label>
            Name
            <input
              value={form.name}
              disabled={!!editing}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="my-service"
            />
          </label>
          <label>
            Repo URL
            <input
              value={form.repo}
              onChange={(e) => setForm({ ...form, repo: e.target.value })}
              placeholder="git@github.com:org/repo.git"
            />
          </label>
        </div>
        <div className="form-actions">
          <button type="submit">{editing ? "Save changes" : "Add repository"}</button>
          {editing && (
            <button type="button" className="secondary" onClick={resetForm}>
              Cancel
            </button>
          )}
        </div>
      </form>

      {config.services.length === 0 ? (
        <div className="empty-state small">
          <p>No repositories configured yet. Add one manually above or import from GitHub.</p>
        </div>
      ) : (
        <div className="table-wrap">
          <table className="list">
            <thead>
              <tr>
                <th style={{ width: "30%" }}>Name</th>
                <th style={{ width: "42%" }}>Repo</th>
                <th style={{ width: "14%" }}>Clone</th>
                <th style={{ width: "14%" }} />
              </tr>
            </thead>
            <tbody>
              {config.services.map((s) => {
                const cloned = cloneStatus[s.name];
                return (
                  <tr key={s.name}>
                    <td className="cell-name">{s.name}</td>
                    <td>
                      <span className="mono truncate" title={s.repo}>
                        {s.repo}
                      </span>
                    </td>
                    <td>
                      {cloned === undefined ? (
                        <Skeleton w={54} h={12} rounded={999} />
                      ) : cloned ? (
                        <span className="tag tag-ok">cloned</span>
                      ) : (
                        <button
                          className="btn-mini"
                          disabled={busy === s.name}
                          onClick={() => cloneRepo(s.name)}
                        >
                          {busy === s.name ? "cloning…" : "clone"}
                        </button>
                      )}
                    </td>
                    <td className="row-actions">
                      <button className="link" onClick={() => startEdit(s)}>
                        Edit
                      </button>
                      <button className="link danger" onClick={() => setConfirmRemove(s.name)}>
                        Remove
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      {showGithubModal && (
        <GithubRepoModal
          knownNames={new Set(config.services.map((s) => s.name))}
          onAdd={addFromGithub}
          onClose={() => setShowGithubModal(false)}
          onError={onError}
        />
      )}

      {confirmRemove && (
        <ConfirmDialog
          title="Remove repository"
          message={`Remove '${confirmRemove}'? This also deletes the local clone.`}
          confirmLabel="Remove"
          danger
          onConfirm={() => remove(confirmRemove)}
          onClose={() => setConfirmRemove(null)}
        />
      )}
    </div>
  );
}