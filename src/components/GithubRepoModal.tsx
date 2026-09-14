import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { GithubRepo } from "../types/config";

interface Props {
  knownNames: Set<string>;
  onAdd: (repo: GithubRepo) => Promise<void>;
  onClose: () => void;
  onError: (msg: string) => void;
}

function ownerOf(repo: GithubRepo): string {
  return repo.nameWithOwner.split("/")[0];
}

export function GithubRepoModal({ knownNames, onAdd, onClose, onError }: Props) {
  const [repos, setRepos] = useState<GithubRepo[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

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
      await onAdd(selectedRepo);
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
                      onClick={() => setSelected(r.nameWithOwner)}
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

        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button type="button" disabled={!selectedRepo || adding} onClick={confirmAdd}>
            {adding ? "Adding…" : "Add repository"}
          </button>
        </div>
      </div>
    </div>
  );
}
