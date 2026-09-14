import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, GithubRepo, Service } from "../types/config";

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

const emptyForm = { name: "", repo: "" };

export function ReposPage({ config, onChange, onError }: Props) {
  const [form, setForm] = useState(emptyForm);
  const [editing, setEditing] = useState<string | null>(null);
  const [cloneStatus, setCloneStatus] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [ghRepos, setGhRepos] = useState<GithubRepo[] | null>(null);
  const [ghLoading, setGhLoading] = useState(false);

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
    if (!confirm(`Remove repo '${name}'? This also deletes the local clone.`)) return;
    try {
      const cfg = await invoke<Config>("remove_service", { name });
      onChange(cfg);
    } catch (e) {
      onError(String(e));
    }
  };

  const checkClone = async (name: string) => {
    try {
      const cloned = await invoke<boolean>("service_clone_status", { name });
      setCloneStatus((s) => ({ ...s, [name]: cloned }));
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

  const loadGithubRepos = async () => {
    setGhLoading(true);
    try {
      const repos = await invoke<GithubRepo[]>("github_list_repos");
      setGhRepos(repos);
    } catch (e) {
      onError(String(e));
    } finally {
      setGhLoading(false);
    }
  };

  const addFromGithub = async (repo: GithubRepo) => {
    try {
      const cfg = await invoke<Config>("add_service", { name: repo.name, repo: repo.sshUrl });
      onChange(cfg);
    } catch (e) {
      onError(String(e));
    }
  };

  const knownNames = new Set(config.services.map((s) => s.name));

  return (
    <div className="page">
      <h2>Repositories</h2>

      <form
        className="card form"
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <h3>{editing ? `Edit '${editing}'` : "Add repository"}</h3>
        <div className="form-grid">
          <label>
            Name
            <input
              value={form.name}
              disabled={!!editing}
              onChange={(e) => setForm({ ...form, name: e.target.value })}
              placeholder="audience-svc"
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
          <button type="submit">{editing ? "Save" : "Add"}</button>
          {editing && (
            <button type="button" className="secondary" onClick={resetForm}>
              Cancel
            </button>
          )}
        </div>
      </form>

      <div className="card">
        <div className="card-header">
          <h3>Import from GitHub</h3>
          <button disabled={ghLoading} onClick={loadGithubRepos}>
            {ghLoading ? "Loading…" : "List my repos"}
          </button>
        </div>
        {ghRepos === null ? (
          <p className="empty">Uses your existing `gh` CLI login. Click to fetch your repos.</p>
        ) : ghRepos.length === 0 ? (
          <p className="empty">No repos found.</p>
        ) : (
          <div className="chip-list">
            {ghRepos.map((r) => (
              <button
                key={r.nameWithOwner}
                className={`chip ${knownNames.has(r.name) ? "chip-disabled" : ""}`}
                disabled={knownNames.has(r.name)}
                onClick={() => addFromGithub(r)}
              >
                {r.nameWithOwner}
                {r.isPrivate ? " 🔒" : ""}
              </button>
            ))}
          </div>
        )}
      </div>

      <table className="list">
        <thead>
          <tr>
            <th>Name</th>
            <th>Repo</th>
            <th>Clone</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {config.services.map((s) => (
            <tr key={s.name}>
              <td>{s.name}</td>
              <td className="mono">{s.repo}</td>
              <td>
                {cloneStatus[s.name] === undefined ? (
                  <button className="link" onClick={() => checkClone(s.name)}>
                    check
                  </button>
                ) : cloneStatus[s.name] ? (
                  "✓ cloned"
                ) : (
                  <button disabled={busy === s.name} onClick={() => cloneRepo(s.name)}>
                    {busy === s.name ? "cloning…" : "clone"}
                  </button>
                )}
              </td>
              <td className="row-actions">
                <button className="link" onClick={() => startEdit(s)}>
                  edit
                </button>
                <button className="link danger" onClick={() => remove(s.name)}>
                  remove
                </button>
              </td>
            </tr>
          ))}
          {config.services.length === 0 && (
            <tr>
              <td colSpan={4} className="empty">
                No repositories configured yet.
              </td>
            </tr>
          )}
        </tbody>
      </table>
    </div>
  );
}
