import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Workspace names with a Ralph run in progress, kept live from ralph-event. */
export function useRalphRunning(): Set<string> {
  const [keys, setKeys] = useState<Set<string>>(new Set());

  useEffect(() => {
    invoke<string[]>("ralph_running")
      .then((k) => setKeys(new Set(k)))
      .catch(() => null);
    const off = listen<{ key: string; event: { kind: string } }>("ralph-event", (e) => {
      const { key, event } = e.payload;
      if (event.kind !== "run_start" && event.kind !== "stopped") return;
      setKeys((prev) => {
        const next = new Set(prev);
        if (event.kind === "run_start") next.add(key);
        else next.delete(key);
        return next;
      });
    });
    return () => {
      off.then((f) => f());
    };
  }, []);

  return new Set([...keys].map((k) => k.split("/")[0]));
}
