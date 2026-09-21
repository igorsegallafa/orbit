import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

/** Windows/Linux have no native frame (decorations: false in
 *  tauri.windows.conf.json), so the titlebar draws its own controls. */
export const isMac = navigator.userAgent.includes("Mac");

export function WindowControls() {
  const win = getCurrentWindow();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    win.isMaximized().then(setMaximized);
    const un = win.onResized(() => win.isMaximized().then(setMaximized));
    return () => {
      un.then((f) => f());
    };
  }, []);

  return (
    <div className="window-controls">
      <button className="window-btn" aria-label="Minimize" onClick={() => win.minimize()}>
        <svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 5h10" stroke="currentColor" /></svg>
      </button>
      <button
        className="window-btn"
        aria-label={maximized ? "Restore" : "Maximize"}
        onClick={() => win.toggleMaximize()}
      >
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor">
            <rect x="0.5" y="2.5" width="7" height="7" />
            <path d="M2.5 2.5v-2h7v7h-2" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor">
            <rect x="0.5" y="0.5" width="9" height="9" />
          </svg>
        )}
      </button>
      <button className="window-btn window-btn-close" aria-label="Close" onClick={() => win.close()}>
        <svg width="10" height="10" viewBox="0 0 10 10" stroke="currentColor"><path d="M0 0l10 10M10 0L0 10" /></svg>
      </button>
    </div>
  );
}
