import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** One batch of changes the backend watcher saw in a repo folder. */
export interface FsChange {
  workspace: string;
  repo: string;
  /** Changed paths relative to the repo root ("/" separated). */
  paths: string[];
  /** Git state moved (commit, checkout, stage, reset...). */
  git: boolean;
}

/**
 * Calls `onChange` whenever files of `workspace` change on disk (agents,
 * other editors, git in a terminal). The backend watches each workspace
 * while at least one subscriber holds it. `repo` narrows it to one repo.
 * Also fires a synthetic full change when the window regains focus, as a
 * safety net for anything a watcher can miss (network drives, limits).
 */
export function useFsChanges(
  workspace: string | null | undefined,
  repo: string | null | undefined,
  onChange: (c: FsChange) => void
) {
  const cb = useRef(onChange);
  cb.current = onChange;

  useEffect(() => {
    if (!workspace) return;
    let alive = true;
    invoke("fs_watch", { workspace }).catch(() => null);
    const off = listen<FsChange>("fs-changed", (e) => {
      if (!alive || e.payload.workspace !== workspace) return;
      if (repo && e.payload.repo !== repo) return;
      cb.current(e.payload);
    });
    const onFocus = () => cb.current({ workspace, repo: repo ?? "", paths: [], git: true });
    window.addEventListener("focus", onFocus);
    return () => {
      alive = false;
      off.then((f) => f());
      window.removeEventListener("focus", onFocus);
      invoke("fs_unwatch", { workspace }).catch(() => null);
    };
  }, [workspace, repo]);
}

/** True when `c` touches `path` (or is a whole-repo/git change). */
export function touches(c: FsChange, path: string): boolean {
  return c.git || c.paths.length === 0 || c.paths.includes(path);
}
