import { useEffect, useState } from "react";

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
    const el = (source as { currentTarget?: EventTarget | null } | undefined)
      ?.currentTarget as HTMLElement | null | undefined;
    if (el && typeof el.getBoundingClientRect === "function") {
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

  useEffect(() => tooltip.subscribe(setState), []);

  if (!state) return null;

  // Clamp horizontally so the tooltip never leaves the window
  // (centered: assume up to 260px wide → 130 half).
  const half = state.centered ? 130 : 8;
  const x = Math.max(half + 4, Math.min(state.x, window.innerWidth - half - 4));

  const transform = state.centered
    ? state.flip
      ? "translate(-50%, -100%)"
      : "translateX(-50%)"
    : state.flip
      ? "translateY(-100%)"
      : undefined;

  return (
    <div className="orbit-tooltip" style={{ left: x, top: state.y, transform }}>
      {state.text}
    </div>
  );
}