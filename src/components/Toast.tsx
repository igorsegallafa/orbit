import { useEffect, useState } from "react";
import { CheckIcon, XIcon } from "./Icons";

export type ToastKind = "success" | "error" | "info" | "loading";

export interface ToastAction {
  label: string;
  onClick: () => void;
}

export interface ToastOptions {
  description?: string;
  action?: ToastAction;
  /** ms before auto-dismiss; 0 keeps it until closed. */
  duration?: number;
}

interface ToastItem extends ToastOptions {
  id: number;
  kind: ToastKind;
  title: string;
}

const DEFAULT_DURATION: Record<ToastKind, number> = {
  success: 4000,
  info: 5000,
  error: 8000,
  loading: 0,
};
const MAX_VISIBLE = 4;

type Listener = (items: ToastItem[]) => void;

/** Global toast queue: anything can call toast.success(...) etc.; the
 *  single <ToastHost /> at the app root renders the stack. */
class ToastRegistry {
  private items: ToastItem[] = [];
  private listener: Listener | null = null;
  private timers = new Map<number, number>();
  private hovered = false;
  private nextId = 1;

  subscribe(l: Listener) {
    this.listener = l;
    l(this.items);
    return () => {
      if (this.listener === l) this.listener = null;
    };
  }

  private emit() {
    this.listener?.([...this.items]);
  }

  private schedule(item: ToastItem) {
    window.clearTimeout(this.timers.get(item.id));
    const ms = item.duration ?? DEFAULT_DURATION[item.kind];
    if (!ms) return;
    const t = window.setTimeout(() => {
      // Keep toasts up while the pointer is over the stack.
      if (this.hovered) this.schedule(item);
      else this.dismiss(item.id);
    }, ms);
    this.timers.set(item.id, t);
  }

  private push(kind: ToastKind, title: string, opts: ToastOptions = {}) {
    const item: ToastItem = { id: this.nextId++, kind, title, ...opts };
    this.items = [...this.items, item].slice(-MAX_VISIBLE);
    this.schedule(item);
    this.emit();
    return item.id;
  }

  success = (title: string, opts?: ToastOptions) => this.push("success", title, opts);
  error = (title: string, opts?: ToastOptions) => this.push("error", title, opts);
  info = (title: string, opts?: ToastOptions) => this.push("info", title, opts);
  loading = (title: string, opts?: ToastOptions) => this.push("loading", title, opts);

  /** Turns an existing toast (usually a loading one) into another state. */
  update(id: number, kind: ToastKind, title: string, opts: ToastOptions = {}) {
    const found = this.items.find((t) => t.id === id);
    if (!found) return this.push(kind, title, opts);
    const item: ToastItem = { id, kind, title, ...opts };
    this.items = this.items.map((t) => (t.id === id ? item : t));
    this.schedule(item);
    this.emit();
    return id;
  }

  dismiss(id: number) {
    window.clearTimeout(this.timers.get(id));
    this.timers.delete(id);
    this.items = this.items.filter((t) => t.id !== id);
    this.emit();
  }

  setHovered(h: boolean) {
    this.hovered = h;
  }
}

export const toast = new ToastRegistry();

export function ToastHost() {
  const [items, setItems] = useState<ToastItem[]>([]);
  useEffect(() => toast.subscribe(setItems), []);

  if (items.length === 0) return null;
  return (
    <div
      className="toast-stack"
      role="region"
      aria-label="Notifications"
      onMouseEnter={() => toast.setHovered(true)}
      onMouseLeave={() => toast.setHovered(false)}
    >
      {items.map((t) => (
        <div key={t.id} className={`toast toast-${t.kind}`} role={t.kind === "error" ? "alert" : "status"}>
          <span className="toast-icon">
            {t.kind === "success" ? (
              <CheckIcon size={12} />
            ) : t.kind === "error" ? (
              <XIcon size={12} />
            ) : t.kind === "loading" ? (
              <span className="spinner" />
            ) : (
              <span className="toast-info-dot" />
            )}
          </span>
          <div className="toast-content">
            <div className="toast-title">{t.title}</div>
            {t.description && <div className="toast-desc">{t.description}</div>}
          </div>
          {t.action && (
            <button
              className="toast-action"
              onClick={() => {
                t.action!.onClick();
                toast.dismiss(t.id);
              }}
            >
              {t.action.label}
            </button>
          )}
          {t.kind !== "loading" && (
            <button className="toast-close" aria-label="Dismiss" onClick={() => toast.dismiss(t.id)}>
              ×
            </button>
          )}
        </div>
      ))}
    </div>
  );
}
