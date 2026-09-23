import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GithubRepo } from "../types/config";
import { CheckBox } from "./CheckBox";
import { PathInput } from "./PathInput";
import { Folders } from "./FoldersCard";
import { clonePathFor, defaultClonePath } from "./RepoFormModal";

interface Props {
  knownNames: Set<string>;
  folders: Folders | null;
  /** `path` null = the clones folder. */
  onAdd: (repo: GithubRepo, path: string | null, cloneNow: boolean) => Promise<void>;
  onClose: () => void;
  onError: (msg: string) => void;
}

function ownerOf(repo: GithubRepo): string {
  return repo.nameWithOwner.split("/")[0];
}

export function GithubRepoModal({ knownNames, folders, onAdd, onClose, onError }: Props) {
  const [repos, setRepos] = useState<GithubRepo[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [path, setPath] = useState("");
  const [cloneNow, setCloneNow] = useState(true);

  const load = async (forceRefresh: boolean) => {
    try {
      const result = await invoke<GithubRepo[]>("github_list_repos", { forceRefresh });
      setRepos(result);
    } catch (e) {
      onError(String(e));
    } finally {
      setLoading(false);
      setRefreshing(false);
    }
  };

  useEffect(() => {
    load(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const refresh = () => {
    setRefreshing(true);
    load(true);
  };

  const filtered = useMemo(() => {
    if (!repos) return [];
    const q = query.trim().toLowerCase();
    if (!q) return repos;
    return repos.filter((r) => r.nameWithOwner.toLowerCase().includes(q));
  }, [repos, query]);

  const grouped = useMemo(() => {
    const groups = new Map<string, GithubRepo[]>();
    for (const r of filtered) {
      const owner = ownerOf(r);
      if (!groups.has(owner)) groups.set(owner, []);
      groups.get(owner)!.push(r);
    }
    return Array.from(groups.entries());
  }, [filtered]);

  const selectedRepo = repos?.find((r) => r.nameWithOwner === selected) ?? null;

  const confirmAdd = async () => {
    if (!selectedRepo) return;
    setAdding(true);
    try {
      await onAdd(selectedRepo, path.trim() || null, cloneNow);
      onClose();
    } catch (e) {
      onError(String(e));
    } finally {
      setAdding(false);
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>Add a repository from GitHub</h3>
          <button
            type="button"
            className="icon-button"
            title="Refresh"
            disabled={loading || refreshing}
            onClick={refresh}
          >
            {refreshing ? "…" : "⟳"}
          </button>
        </div>

        <div className="modal-search">
          <input
            autoFocus
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter repositories…"
          />
        </div>

        <div className="modal-list">
          {loading ? (
            <div className="modal-loading">
              <span className="spinner" />
              Fetching your repositories and organizations…
            </div>
          ) : filtered.length === 0 ? (
            <p className="empty">No repositories found.</p>
          ) : (
            grouped.map(([owner, ownerRepos]) => (
              <div key={owner} className="modal-group">
                <div className="modal-group-label">{owner}</div>
                {ownerRepos.map((r) => {
                  const already = knownNames.has(r.name);
                  const isSelected = selected === r.nameWithOwner;
                  return (
                    <button
                      key={r.nameWithOwner}
                      type="button"
                      className={`modal-row ${isSelected ? "modal-row-selected" : ""} ${already ? "modal-row-disabled" : ""}`}
                      disabled={already}
                      onClick={() => {
                        setSelected(r.nameWithOwner);
                        setPath("");
                      }}
                    >
                      <span className="modal-row-name">{r.name}</span>
                      <span className="modal-row-meta">
                        {already ? "already added" : r.isPrivate ? "private" : "public"}
                      </span>
                    </button>
                  );
                })}
              </div>
            ))
          )}
        </div>

        {selectedRepo && (
          <div className="gh-dest">
            <div className="field">
              <span className="field-label">Local path for {selectedRepo.name}</span>
              <PathInput
                value={path}
                onChange={setPath}
                placeholder={defaultClonePath(folders, selectedRepo.name)}
                fromPicked={(folder) => clonePathFor(folder, selectedRepo.name)}
              />
              <span className="field-hint">Empty uses the clones folder. Pick an existing checkout to use it as is.</span>
            </div>
          </div>
        )}

        <div className="modal-footer">
          <label className="check-item repo-clone-now">
            <CheckBox label="Clone now" checked={cloneNow} onChange={setCloneNow} />
            Clone now
          </label>
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button type="button" disabled={!selectedRepo || adding} onClick={confirmAdd}>
            {adding ? "Adding…" : cloneNow ? "Add and clone" : "Add repository"}
          </button>
        </div>
      </div>
    </div>
  );
}
