// Folder scopes: sessions opened in any folder, outside workspaces.
// Addressed like a workspace name, "dir:<path>" ("dir:~" is home), so tabs,
// recent sessions and the backend (pty_spawn) handle them like the rest.

export const HOME_SCOPE = "dir:~";

export const isFolderScope = (scope: string) => scope.startsWith("dir:");

export const folderScope = (path: string) => `dir:${path}`;

export const folderPath = (scope: string) => scope.slice(4);

/** Short name for a folder scope: "Home", or the folder's own name. */
export function folderLabel(scope: string): string {
  const p = folderPath(scope).replace(/[\\/]+$/, "");
  if (p === "~") return "Home";
  return p.split(/[\\/]/).pop() || p;
}

/** Human label for any scope: workspace name, repo, or folder. */
export function scopeLabel(scope: string): string {
  if (isFolderScope(scope)) return folderLabel(scope);
  if (scope.startsWith("@")) return scope.slice(1);
  return scope;
}

const RECENT_FOLDERS_KEY = "orbit.recent-folders";
const RECENT_FOLDERS_MAX = 6;

export function recentFolders(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(RECENT_FOLDERS_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((s) => typeof s === "string" && isFolderScope(s)) : [];
  } catch {
    return [];
  }
}

export function rememberFolder(scope: string) {
  if (!isFolderScope(scope) || scope === HOME_SCOPE) return;
  const next = [scope, ...recentFolders().filter((s) => s !== scope)].slice(0, RECENT_FOLDERS_MAX);
  try {
    localStorage.setItem(RECENT_FOLDERS_KEY, JSON.stringify(next));
  } catch {
    // Not remembered; the folder still opens.
  }
}
