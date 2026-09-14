import { useCallback, useEffect, useRef, useState } from "react";

export interface MenuItem {
  label: string;
  danger?: boolean;
  onSelect: () => void;
}

interface Props {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}

interface MenuState<T> {
  x: number;
  y: number;
  payload: T;
}

/**
 * Right-click support across WKWebView versions: depending on the macOS/WebKit
 * build, a right-click may surface as `pointerdown`, `mousedown` (button 2)
 * and/or `contextmenu` — so trigger from all of them; duplicate opens with the
 * same coords are harmless (same state).
 */
export function useContextMenu<T>() {
  const [menu, setMenu] = useState<MenuState<T> | null>(null);

  const openFromEvent = useCallback(
    (
      e: { button: number; clientX: number; clientY: number; preventDefault: () => void },
      payload: T
    ) => {
      if (e.button !== 2) return;
      e.preventDefault();
      setMenu({ x: e.clientX, y: e.clientY, payload });
    },
    []
  );

  return { menu, setMenu, openFromEvent };
}

export function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [onClose]);

  // Keep the menu inside the window
  const style: React.CSSProperties = { left: x, top: y };
  const MENU_W = 200;
  const MENU_H = items.length * 34 + 8;
  if (x + MENU_W > window.innerWidth) style.left = x - MENU_W;
  if (y + MENU_H > window.innerHeight) style.top = y - MENU_H;

  return (
    <div className="context-menu" ref={ref} style={style}>
      {items.map((item) => (
        <button
          key={item.label}
          className={`context-menu-item ${item.danger ? "danger" : ""}`}
          onClick={() => {
            item.onSelect();
            onClose();
          }}
        >
          {item.label}
        </button>
      ))}
    </div>
  );
}