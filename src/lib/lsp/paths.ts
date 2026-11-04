// Path mapping between a repo folder as Orbit opens it and the folder a
// language server works in. Repos shared into a workspace are symlinks, and
// servers answer with resolved paths: without this, "go to definition" lands
// on files Orbit doesn't recognize as the repo's.

/* eslint-disable @typescript-eslint/no-explicit-any */

/** Comparable form of a path: forward slashes, no trailing slash, and
 *  lower case on Windows (case-insensitive file system). */
export function comparablePath(p: string, windows: boolean): string {
  const s = p.replace(/\\/g, "/").replace(/\/+$/, "");
  return windows ? s.toLowerCase() : s;
}

/** File system path of a `file:` URI ("file:///c%3A/x" → "c:/x"). */
export function uriPath(uri: string): string | null {
  if (!uri.startsWith("file://")) return null;
  let p = decodeURIComponent(uri.slice("file://".length));
  if (/^\/[A-Za-z]:/.test(p)) p = p.slice(1);
  return p;
}

/** Rewrites `path` from under `from` to under `to`, or null when it's not
 *  inside `from`. Separators in the result follow `to`. */
export function rebase(path: string, from: string, to: string, windows: boolean): string | null {
  const p = comparablePath(path, windows);
  const f = comparablePath(from, windows);
  if (p !== f && !p.startsWith(f + "/")) return null;
  const rest = path.replace(/\\/g, "/").slice(from.replace(/\\/g, "/").replace(/\/+$/, "").length);
  const base = to.replace(/[\\/]+$/, "");
  return base + (to.includes("\\") ? rest.replace(/\//g, "\\") : rest);
}

/** Maps every `file:` URI inside a JSON-RPC payload from one root to the
 *  other. Returns the payload itself when nothing needs mapping. */
export function mapUris(value: any, from: string, to: string, windows: boolean, toUri: (path: string) => string): any {
  if (typeof value === "string") {
    const path = uriPath(value);
    if (path === null) return value;
    const moved = rebase(path, from, to, windows);
    return moved === null ? value : toUri(moved);
  }
  if (Array.isArray(value)) {
    let out: any[] | null = null;
    for (let i = 0; i < value.length; i++) {
      const v = value[i];
      if (typeof v === "number") continue;
      const m = mapUris(v, from, to, windows, toUri);
      if (m !== v) (out ??= value.slice())[i] = m;
    }
    return out ?? value;
  }
  if (value && typeof value === "object") {
    let out: Record<string, any> | null = null;
    for (const k of Object.keys(value)) {
      const v = value[k];
      const m = mapUris(v, from, to, windows, toUri);
      if (m !== v) (out ??= { ...value })[k] = m;
    }
    return out ?? value;
  }
  return value;
}
