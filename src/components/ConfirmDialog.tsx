import { useEffect } from "react";

interface Props {
  title: string;
  message: string;
  confirmLabel?: string;
  danger?: boolean;
  /** The confirmed action is running: lock the dialog so it can't fire twice. */
  busy?: boolean;
  busyLabel?: string;
  onConfirm: () => void;
  onClose: () => void;
}

export function ConfirmDialog({ title, message, confirmLabel = "Confirm", danger, busy, busyLabel, onConfirm, onClose }: Props) {
  useEffect(() => {
    if (busy) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
      if (e.key === "Enter") onConfirm();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [busy, onClose, onConfirm]);

  return (
    <div className="modal-overlay" onMouseDown={busy ? undefined : onClose}>
      <div className="modal modal-sm" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-body">
          <h3>{title}</h3>
          <p>{message}</p>
        </div>
        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose} disabled={busy}>
            Cancel
          </button>
          <button type="button" className={danger ? "danger" : ""} onClick={onConfirm} disabled={busy} autoFocus>
            {busy ? (
              <>
                <span className="spinner" /> {busyLabel ?? `${confirmLabel}…`}
              </>
            ) : (
              confirmLabel
            )}
          </button>
        </div>
      </div>
    </div>
  );
}
