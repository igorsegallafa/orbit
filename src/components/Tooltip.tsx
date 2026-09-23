import { useEffect, useLayoutEffect, useRef, useState } from "react";

interface TooltipState {
  text: string;
  /** Horizontal anchor: element center (centered) or point x + offset. */
  x: number;
  /** Vertical anchor: element bottom + gap, or point y + offset. */
  y: number;
  /** Element-anchored: center horizontally on the anchor. */
  centered: boolean;
  /** Render above the anchor instead of below (no room underneath). */
  flip: boolean;
}

type Listener = (s: TooltipState | null) => void;

/** Global tooltip registry: components call tooltip.show(text, event) on
 *  mouseenter and tooltip.hide() on mouseleave; the single <TooltipHost />
 *  mounted at the app root renders it. Tooltips anchor to the hovered
 *  element (centered below, flipping above when near the bottom edge). */
class TooltipRegistry {
  private listener: Listener | null = null;
  private state: TooltipState | null = null;

  show(text: string, source: unknown) {
    const ev = source as
      | { currentTarget?: EventTarget | null; clientX?: number; clientY?: number }
      | null
      | undefined;
    const el = ev?.currentTarget as HTMLElement | null | undefined;
    // Prefer the mouse point when the event carries it — near where the
    // user is looking. Wide elements (flex rows) would center the
    // tooltip far from the cursor.
    if (ev && typeof ev.clientX === "number" && typeof ev.clientY === "number") {
      const below = ev.clientY + 18;
      const flip = below + 80 > window.innerHeight && ev.clientY > 90;
      this.state = {
        text,
        x: ev.clientX + 12,
        y: flip ? ev.clientY - 6 : below,
        centered: false,
        flip,
      };
    } else if (el && typeof el.getBoundingClientRect === "function") {
      const r = el.getBoundingClientRect();
      const below = r.bottom + 6;
      // Flip above when there's no room below but there is above.
      const flip = below + 80 > window.innerHeight && r.top > 90;
      this.state = {
        text,
        x: r.left + r.width / 2,
        y: flip ? r.top - 6 : below,
        centered: true,
        flip,
      };
    } else if (source) {
      const p = source as { clientX: number; clientY: number };
      this.state = {
        text,
        x: p.clientX + 12,
        y: p.clientY + 18,
        centered: false,
        flip: false,
      };
    } else {
      return;
    }
    this.listener?.(this.state);
  }

  hide() {
    this.state = null;
    this.listener?.(null);
  }

  subscribe(listener: Listener): () => void {
    this.listener = listener;
    return () => {
      this.listener = null;
    };
  }
}

export const tooltip = new TooltipRegistry();

/** Mount once at the app root. */
export function TooltipHost() {
  const [state, setState] = useState<TooltipState | null>(null);

  // Clamp needs the RENDERED size — hooks stay ABOVE the early return:
  // a conditional hook count crashes React's reconciler when the
  // tooltip appears/disappears (bit us: updateWorkInProgressHook).
  const ref = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    if (r.width > 0 && (Math.round(r.width) !== size.w || Math.round(r.height) !== size.h)) {
      setSize({ w: Math.round(r.width), h: Math.round(r.height) });
    } else if (r.width === 0 && (size.w !== 0 || size.h !== 0)) {
      setSize({ w: 0, h: 0 });
    }
  }, [state, size.w, size.h]);

  useEffect(() => tooltip.subscribe(setState), []);

  // Kill tooltips on any click/scroll/keydown: the anchor element can be
  // unmounted by a screen switch (no mouseleave ever fires) and the
  // tooltip would float over the new screen forever.
  useEffect(() => {
    const kill = () => tooltip.hide();
    window.addEventListener("pointerdown", kill, true);
    window.addEventListener("wheel", kill, { capture: true, passive: true });
    window.addEventListener("scroll", kill, { capture: true, passive: true });
    window.addEventListener("keydown", kill, true);
    return () => {
      window.removeEventListener("pointerdown", kill, true);
      window.removeEventListener("wheel", kill, true);
      window.removeEventListener("scroll", kill, true);
      window.removeEventListener("keydown", kill, true);
    };
  }, []);

  if (!state) return null;

  // Clamp the box's final edges against its REAL rendered size (measured
  // above): clamping the anchor and then translating by the box size would
  // push flipped tooltips far from the cursor near the bottom edge.
  const margin = 6;
  const w = size.w || 260;
  const h = size.h || 26;
  const left = Math.max(margin, Math.min(state.centered ? state.x - w / 2 : state.x, window.innerWidth - w - margin));
  const top = Math.max(margin, Math.min(state.flip ? state.y - h : state.y, window.innerHeight - h - margin));

  return (
    <div
      ref={ref}
      className="orbit-tooltip"
      style={{ left, top, visibility: size.w ? undefined : "hidden" }}
    >
      {state.text}
    </div>
  );
}