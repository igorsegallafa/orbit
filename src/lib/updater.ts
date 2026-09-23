import { useEffect, useState } from "react";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { toast } from "../components/Toast";

/** Background checks while the app is open (plus one at startup). */
const CHECK_INTERVAL_MS = 60 * 60 * 1000;

export type UpdaterState =
  | { status: "idle" }
  | { status: "checking" }
  | { status: "up-to-date"; checkedAt: number }
  | { status: "available"; version: string; notes?: string }
  | { status: "downloading"; version: string; percent: number | null }
  | { status: "error"; message: string };

let state: UpdaterState = { status: "idle" };
let pending: Update | null = null;
let announced: string | null = null;
const listeners = new Set<(s: UpdaterState) => void>();

function setState(next: UpdaterState) {
  state = next;
  listeners.forEach((l) => l(state));
}

/** Live updater state for the Settings → Updates tab. */
export function useUpdater(): UpdaterState {
  const [s, setS] = useState(state);
  useEffect(() => {
    listeners.add(setS);
    setS(state);
    return () => void listeners.delete(setS);
  }, []);
  return s;
}

/** Asks GitHub for a newer release. Background checks stay silent on
 *  failure (offline, no release yet) and toast a given version only once;
 *  a manual check reports whatever it finds. */
export async function checkForUpdates({ manual = false } = {}) {
  if (state.status === "checking" || state.status === "downloading") return;
  setState({ status: "checking" });
  try {
    pending = await check();
  } catch (e) {
    setState(manual ? { status: "error", message: String(e) } : { status: "idle" });
    return;
  }
  if (!pending) {
    setState({ status: "up-to-date", checkedAt: Date.now() });
    return;
  }
  const version = pending.version;
  setState({ status: "available", version, notes: pending.body ?? undefined });
  if (!manual && announced !== version) {
    announced = version;
    toast.info(`Orbit ${version} is available`, {
      description: "Restart to install the update.",
      duration: 0,
      action: { label: "Update", onClick: () => void installUpdate() },
    });
  }
}

/** Downloads and installs the pending update, then restarts Orbit. */
export async function installUpdate() {
  if (!pending) return;
  const version = pending.version;
  let total = 0;
  let done = 0;
  setState({ status: "downloading", version, percent: null });
  try {
    await pending.downloadAndInstall((e) => {
      if (e.event === "Started") total = e.data.contentLength ?? 0;
      if (e.event === "Progress") {
        done += e.data.chunkLength;
        setState({ status: "downloading", version, percent: total ? Math.round((done / total) * 100) : null });
      }
    });
    await relaunch();
  } catch (e) {
    setState({ status: "error", message: String(e) });
    toast.error("Update failed", { description: String(e) });
  }
}

/** Startup check plus hourly ones while Orbit stays open. Dev builds skip. */
export function startUpdateChecks(): () => void {
  if (import.meta.env.DEV) return () => {};
  void checkForUpdates();
  const t = window.setInterval(() => void checkForUpdates(), CHECK_INTERVAL_MS);
  return () => window.clearInterval(t);
}
