import { useEffect, useRef } from "react";

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