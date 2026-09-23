import { useEffect, useState } from "react";
import { toast } from "./Toast";
import { invoke } from "@tauri-apps/api/core";
import { Config } from "../types/config";
import { PathInput } from "./PathInput";

export interface Folders {
  reposDir: string;
  workspacesDir: string;
  defaultReposDir: string;
  defaultWorkspacesDir: string;
}

interface Props {
  config: Config;
  onChange: (cfg: Config) => void;
  onError: (msg: string) => void;
}

/** Where Orbit clones repos and creates workspaces. */
export function FoldersCard({ config, onChange, onError }: Props) {
  const [defaults, setDefaults] = useState<Folders | null>(null);
  const [repos, setRepos] = useState(config.reposDir ?? "");
  const [workspaces, setWorkspaces] = useState(config.workspacesDir ?? "");

  useEffect(() => {
    invoke<Folders>("get_folders").then(setDefaults).catch((e) => onError(String(e)));
  }, []);

  const dirty = repos !== (config.reposDir ?? "") || workspaces !== (config.workspacesDir ?? "");

  const save = async () => {
    try {
      onChange(await invoke<Config>("set_folders", { reposDir: repos || null, workspacesDir: workspaces || null }));
      toast.success("Folders saved");
    } catch (e) {
      onError(String(e));
    }
  };

  return (
    <div className="card folders-card">
      <div className="form-grid">
        <div className="field">
          <span className="field-label">Clones folder</span>
          <PathInput value={repos} onChange={setRepos} placeholder={defaults?.defaultReposDir} />
          <span className="field-hint">New repositories are cloned here unless you pick a folder for them.</span>
        </div>
        <div className="field">
          <span className="field-label">Workspaces folder</span>
          <PathInput value={workspaces} onChange={setWorkspaces} placeholder={defaults?.defaultWorkspacesDir} />
          <span className="field-hint">Each workspace gets a folder here with one worktree per repository.</span>
        </div>
      </div>
      {dirty && (
        <div className="folders-save">
          <span className="field-hint">Existing clones and workspaces are not moved; Orbit starts using the new folders.</span>
          <button type="button" className="secondary" onClick={() => { setRepos(config.reposDir ?? ""); setWorkspaces(config.workspacesDir ?? ""); }}>
            Cancel
          </button>
          <button type="button" onClick={save}>
            Save folders
          </button>
        </div>
      )}
    </div>
  );
}
