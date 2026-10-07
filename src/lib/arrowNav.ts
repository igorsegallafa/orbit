import type { KeyboardEvent } from "react";

/** ↑/↓ on a list container: focuses the neighbouring `rows` element.
 *  `open` also clicks it, for lists that preview the row in place. */
export function arrowNav(rows: string, open = false) {
  return (e: KeyboardEvent<HTMLElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if (e.altKey || e.ctrlKey || e.metaKey || e.shiftKey) return;
    const target = e.target as HTMLElement;
    if (target.closest("input, textarea, select")) return;
    const all = [...e.currentTarget.querySelectorAll<HTMLElement>(rows)];
    const focused = all.findIndex((r) => r.contains(target));
    // Nothing focused (WebKit doesn't focus a clicked button): go from the open row.
    const at = focused >= 0 ? focused : all.findIndex((r) => r.closest(".tree-active, .on"));
    const next = all[at + (e.key === "ArrowDown" ? 1 : -1)];
    if (!next) return;
    e.preventDefault();
    next.focus();
    if (open) next.click();
  };
}
