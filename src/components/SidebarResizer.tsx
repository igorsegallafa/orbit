import { useCallback, useEffect, useRef } from "react";

interface Props {
  width: number;
  onResize: (width: number) => void;
}

const MIN = 64;
const MAX = 400;
const COLLAPSE_THRESHOLD = 110;

export function SidebarResizer({ width, onResize }: Props) {
  const dragging = useRef(false);

  const onMouseDown = useCallback(() => {
    dragging.current = true;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";
  }, []);

  useEffect(() => {
    const onMove = (e: MouseEvent) => {
      if (!dragging.current) return;
      // Snap to icon-only width when dragged below the collapse threshold
      const raw = e.clientX;
      const w = raw < COLLAPSE_THRESHOLD ? MIN : Math.min(MAX, Math.max(MIN, raw));
      onResize(w);
    };
    const onUp = () => {
      dragging.current = false;
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
  }, [onResize]);

  return <div className={`sidebar-resizer ${width <= MIN + 1 ? "at-min" : ""}`} onMouseDown={onMouseDown} />;
}