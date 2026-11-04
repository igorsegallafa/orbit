// Actions anyone can run by name: Search Everywhere's Actions tab and the
// Ctrl+Shift+A palette list them. Components register what they can do
// while mounted (useCommands); the list is whatever is registered now.
import { useEffect, useRef } from "react";

export interface Command {
  id: string;
  title: string;
  /** Shortcut shown next to the title, e.g. "Ctrl+F9". */
  keys?: string;
  /** Hidden when it can't run right now. */
  enabled?: () => boolean;
  run: () => void;
}

const registry = new Map<string, Command>();

export function listCommands(): Command[] {
  return [...registry.values()].filter((c) => c.enabled?.() ?? true).sort((a, b) => a.title.localeCompare(b.title));
}

/** Registers commands while the component is mounted; their handlers are
 *  always the latest render's. */
export function useCommands(commands: Command[]) {
  const latest = useRef(commands);
  latest.current = commands;
  const ids = commands.map((c) => c.id).join("|");
  useEffect(() => {
    const mine = latest.current.map((c) => c.id);
    for (const id of mine) {
      registry.set(id, {
        id,
        get title() {
          return latest.current.find((c) => c.id === id)?.title ?? id;
        },
        get keys() {
          return latest.current.find((c) => c.id === id)?.keys;
        },
        enabled: () => latest.current.find((c) => c.id === id)?.enabled?.() ?? true,
        run: () => latest.current.find((c) => c.id === id)?.run(),
      });
    }
    return () => mine.forEach((id) => registry.delete(id));
  }, [ids]);
}
