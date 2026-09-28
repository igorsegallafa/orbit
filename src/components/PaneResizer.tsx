import { useEffect, useRef, useState } from "react";

/**
 * Width of a side panel the user can drag, remembered under `key`.
 * Returns the width and the handle to render on the panel's edge.
 */
export function usePaneWidth(key: string, initial: number, min: number, max: number) {
  const [width, setWidth] = useState(() => {
    try {
      const n = Number(localStorage.getItem(key));
      return Number.isFinite(n) && n >= min && n <= max ? n : initial;
    } catch {
      return initial;
    }
  });
  const drag = useRef<{ x: number; w: number } | null>(null);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!drag.current) return;
      setWidth(Math.min(max, Math.max(min, drag.current.w + e.clientX - drag.current.x)));
    };
    const onUp = () => {
      if (!drag.current) return;
      drag.current = null;
      document.body.style.cursor = "";
      setWidth((w) => {
        try {
          localStorage.setItem(key, String(w));
        } catch {
          // Not remembered; the drag still applies.
        }
        return w;
      });
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [key, min, max]);

  const handle = (
    <div
      className="pane-resizer"
      role="separator"
      aria-orientation="vertical"
      onMouseDown={(e) => {
        e.preventDefault();
        drag.current = { x: e.clientX, w: width };
        document.body.style.cursor = "col-resize";
      }}
      onDoubleClick={() => setWidth(initial)}
    />
  );
  return { width, handle };
}
