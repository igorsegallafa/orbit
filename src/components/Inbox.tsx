import { useEffect, useRef, useState } from "react";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { BellIcon } from "./Icons";
import { timeAgo } from "../types/review";

export type InboxKind = "done" | "waiting" | "ralph-done" | "ralph-stopped";

export interface InboxItem {
  id: string;
  /** Tab to focus when opened (terminal session or Ralph view). */
  tabId: string;
  kind: InboxKind;
  title: string;
  detail: string;
  at: number;
  read: boolean;
}

/** Native OS notification; silently skipped when not permitted. */
export async function notifyOs(title: string, body: string) {
  try {
    let ok = await isPermissionGranted();
    if (!ok) ok = (await requestPermission()) === "granted";
    if (ok) sendNotification({ title, body });
  } catch {
    // notifications unavailable: the inbox still has it
  }
}

/** Titlebar bell: unread count and the recent agent events. */
export function InboxButton({
  items,
  onOpen,
  onMarkAllRead,
  onClear,
}: {
  items: InboxItem[];
  onOpen: (item: InboxItem) => void;
  onMarkAllRead: () => void;
  onClear: () => void;
}) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const unread = items.filter((i) => !i.read).length;

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => ref.current && !ref.current.contains(e.target as Node) && setOpen(false);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  return (
    <div className="inbox" ref={ref}>
      <button className={`titlebar-btn inbox-btn ${open ? "on" : ""}`} aria-label="Notifications" onClick={() => setOpen((v) => !v)}>
        <BellIcon size={14} />
        {unread > 0 && <span className="inbox-badge">{unread > 9 ? "9+" : unread}</span>}
      </button>
      {open && (
        <div className="inbox-pop">
          <div className="inbox-head">
            <strong>Notifications</strong>
            {items.length > 0 && (
              <span className="inbox-head-actions">
                {unread > 0 && (
                  <button className="inbox-link" onClick={onMarkAllRead}>
                    Mark all read
                  </button>
                )}
                <button className="inbox-link" onClick={onClear}>
                  Clear
                </button>
              </span>
            )}
          </div>
          {items.length === 0 ? (
            <div className="inbox-empty">You'll hear here when an agent finishes or needs you.</div>
          ) : (
            <div className="inbox-list">
              {items.map((it) => (
                <button
                  key={it.id}
                  className={`inbox-item ${it.read ? "" : "unread"}`}
                  onClick={() => {
                    onOpen(it);
                    setOpen(false);
                  }}
                >
                  <span className={`inbox-dot inbox-dot-${it.kind}`} />
                  <span className="inbox-main">
                    <span className="inbox-title">{it.title}</span>
                    <span className="inbox-detail">
                      {it.detail} · {timeAgo(new Date(it.at).toISOString())}
                    </span>
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
