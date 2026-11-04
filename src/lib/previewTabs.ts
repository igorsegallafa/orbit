// Where a file tab goes when it's opened (VS Code's preview tabs): a preview
// open reuses the current preview tab's slot instead of adding a tab; a
// regular open of a previewed file keeps it.

export interface TabOps<T> {
  id: (t: T) => string;
  isPreview: (t: T) => boolean;
  /** The tab, kept open (no longer a preview). */
  pin: (t: T) => T;
}

export function placeFileTab<T>(tabs: T[], tab: T, preview: boolean, ops: TabOps<T>): T[] {
  const id = ops.id(tab);
  const existing = tabs.find((t) => ops.id(t) === id);
  if (existing) {
    return !preview && ops.isPreview(existing) ? tabs.map((t) => (t === existing ? ops.pin(existing) : t)) : tabs;
  }
  const old = preview ? tabs.findIndex(ops.isPreview) : -1;
  if (old === -1) return [...tabs, tab];
  const next = tabs.slice();
  next[old] = tab;
  return next;
}
