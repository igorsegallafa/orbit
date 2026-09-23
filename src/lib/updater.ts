import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { toast } from "../components/Toast";

/** Looks for a newer release on GitHub and offers it as a toast. Silent on
 *  failure (offline, dev build, no release yet): updating is never urgent. */
export async function checkForUpdates() {
  if (import.meta.env.DEV) return;
  let update;
  try {
    update = await check();
  } catch {
    return;
  }
  if (!update) return;

  toast.info(`Orbit ${update.version} is available`, {
    description: "Restart to install the update.",
    duration: 0,
    action: {
      label: "Update",
      onClick: async () => {
        const id = toast.loading(`Downloading Orbit ${update.version}…`);
        try {
          await update.downloadAndInstall();
          await relaunch();
        } catch (e) {
          toast.update(id, "error", "Update failed", { description: String(e) });
        }
      },
    },
  });
}
