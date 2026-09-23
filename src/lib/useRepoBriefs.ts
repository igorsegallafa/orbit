import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { RepoBrief } from "../types/config";

/** Status refresh while Orbit is open (local git only, cheap). */
const STATUS_INTERVAL_MS = 60 * 1000;
/** Background `git fetch` of every clone, so "behind" counts stay true. */
const FETCH_INTERVAL_MS = 10 * 60 * 1000;

/** Sidebar status of every configured repo's clone (branch, changes,
 *  ahead/behind), kept fresh: on focus, every minute, and after a quiet
 *  background fetch every 10 minutes. */
export function useRepoBriefs(repoCount: number): { briefs: Record<string, RepoBrief>; refresh: () => void } {
  const [briefs, setBriefs] = useState<Record<string, RepoBrief>>({});
  const fetching = useRef(false);

  const refresh = useCallback(() => {
    invoke<RepoBrief[]>("repo_briefs")
      .then((list) => setBriefs(Object.fromEntries(list.map((b) => [b.name, b]))))
      .catch(() => null);
  }, []);

  const fetchAll = useCallback(async () => {
    if (fetching.current) return;
    fetching.current = true;
    try {
      const list = await invoke<RepoBrief[]>("repo_briefs");
      // One at a time: a burst of parallel fetches competes with the user's own git work.
      for (const b of list.filter((b) => b.cloned)) {
        await invoke("refresh_repo", { name: b.name }).catch(() => null);
      }
    } finally {
      fetching.current = false;
      refresh();
    }
  }, [refresh]);

  useEffect(() => {
    refresh();
    window.addEventListener("focus", refresh);
    const status = window.setInterval(refresh, STATUS_INTERVAL_MS);
    const fetch = window.setInterval(() => void fetchAll(), FETCH_INTERVAL_MS);
    return () => {
      window.removeEventListener("focus", refresh);
      window.clearInterval(status);
      window.clearInterval(fetch);
    };
    // Repos added or removed in Settings: reload with the new list.
  }, [refresh, fetchAll, repoCount]);

  return { briefs, refresh };
}
