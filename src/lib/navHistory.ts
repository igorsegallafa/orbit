// Back/forward through where you've been (mouse side buttons, Alt+←/→, the
// titlebar arrows), like VS Code: tab and page switches, plus jumps inside a
// file (go to definition, a click far away) as positions of that file.

export interface NavEntry<T> {
  /** Tab id or page key: two entries with the same key and no position are one place. */
  key: string;
  target: T;
  line?: number;
  column?: number;
}

const LIMIT = 60;

export class NavHistory<T> {
  private back: NavEntry<T>[] = [];
  private forward: NavEntry<T>[] = [];
  current: NavEntry<T> | null = null;
  private listeners = new Set<() => void>();
  private quietUntil = 0;

  get canGoBack() {
    return this.back.length > 0;
  }

  get canGoForward() {
    return this.forward.length > 0;
  }

  subscribe(l: () => void): () => void {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  }

  private changed() {
    this.listeners.forEach((l) => l());
  }

  private push(entry: NavEntry<T>) {
    if (this.current) this.back.push(this.current);
    if (this.back.length > LIMIT) this.back.shift();
    this.current = entry;
    this.forward = [];
    this.changed();
  }

  /** Something else is shown now (a tab or page switch). */
  visit(entry: NavEntry<T>) {
    if (this.current?.key === entry.key) return;
    this.push(entry);
  }

  /** The cursor of the shown file moved: remembered for when you come back. */
  setPosition(key: string, line: number, column: number) {
    if (this.current?.key !== key) return;
    this.current.line = line;
    this.current.column = column;
  }

  /** A jump inside the shown file: where it left becomes a place to go back to. */
  jump(key: string, target: T, from: { line: number; column: number }, to: { line: number; column: number }) {
    if (Date.now() < this.quietUntil || this.current?.key !== key) return;
    this.current.line = from.line;
    this.current.column = from.column;
    this.push({ key, target, line: to.line, column: to.column });
  }

  /** Moves done by going back/forward themselves aren't new history. */
  quiet(ms = 800) {
    this.quietUntil = Date.now() + ms;
  }

  /** Steps back (or forward), skipping places `usable` rejects. */
  step(dir: "back" | "forward", usable: (e: NavEntry<T>) => boolean): NavEntry<T> | null {
    const from = dir === "back" ? this.back : this.forward;
    const to = dir === "back" ? this.forward : this.back;
    while (from.length > 0) {
      const e = from.pop()!;
      if (!usable(e)) continue;
      if (this.current) to.push(this.current);
      this.current = e;
      this.changed();
      return e;
    }
    this.changed();
    return null;
  }
}
