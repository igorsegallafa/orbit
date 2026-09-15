import { useEffect, useRef, useState } from "react";

export interface SelectOption {
  value: string;
  label: string;
}

interface Props {
  value: string;
  options: SelectOption[];
  onChange: (value: string) => void;
  placeholder?: string;
  className?: string;
  disabled?: boolean;
  /** Renders a filter input at the top of the list (combobox behavior). */
  searchable?: boolean;
  /** Dropdown width in px; defaults to the trigger width. */
  listWidth?: number;
}

/**
 * Custom select: button + floating listbox (no native <select> — its chrome
 * renders inconsistently in the dark WKWebView). The list is positioned
 * FIXED from the trigger's viewport rect so it overlays scrollable
 * containers instead of being clipped by their overflow. With `searchable`,
 * a filter input narrows long option lists (e.g. opencode models).
 */
export function Select({
  value,
  options,
  onChange,
  placeholder,
  className,
  disabled,
  searchable,
  listWidth,
}: Props) {
  const [open, setOpen] = useState(false);
  const [activeIdx, setActiveIdx] = useState(0);
  const [listPos, setListPos] = useState<{ x: number; y: number; w: number } | null>(null);
  const [filter, setFilter] = useState("");
  const rootRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const filterRef = useRef<HTMLInputElement>(null);

  const filtered = searchable && filter
    ? options.filter((o) =>
        (o.label + o.value).toLowerCase().includes(filter.toLowerCase())
      )
    : options;

  const openList = () => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (rect) {
      // Fit the list inside the window: shrink the preferred width when
      // there isn't room to the right, and clamp the left edge.
      const vw = window.innerWidth;
      const preferred = listWidth ?? rect.width;
      const roomRight = vw - rect.left - 8;
      const w = Math.min(preferred, Math.max(rect.width, roomRight));
      const x = Math.max(8, Math.min(rect.left, vw - w - 8));
      setListPos({ x, y: rect.bottom + 4, w });
    }
    setFilter("");
    setActiveIdx(Math.max(0, filtered.findIndex((o) => o.value === value)));
    setOpen(true);
    if (searchable) {
      // focus the filter input once the list mounts
      setTimeout(() => filterRef.current?.focus(), 0);
    }
  };

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      if (
        !rootRef.current?.contains(target) &&
        !listRef.current?.contains(target)
      ) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    // Any ancestor scrolling detaches a fixed list — close, but keep open
    // while scrolling INSIDE the list itself.
    const onScroll = (e: Event) => {
      const target = e.target as Node;
      if (listRef.current?.contains(target)) return;
      setOpen(false);
    };
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll, true);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  const current = options.find((o) => o.value === value);

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(i + 1, filtered.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const opt = filtered[activeIdx];
      if (opt) {
        onChange(opt.value);
        setOpen(false);
      }
    }
  };

  return (
    <div className={`select-root ${className ?? ""} ${disabled ? "select-disabled" : ""}`} ref={rootRef}>
      <button
        type="button"
        className="select-trigger"
        disabled={disabled}
        onClick={() => {
          if (open) setOpen(false);
          else openList();
        }}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
            e.preventDefault();
            openList();
          }
        }}
      >
        <span className={`select-value ${current ? "" : "select-placeholder"}`}>
          {current ? current.label : (placeholder ?? "Select…")}
        </span>
        <span className={`select-caret ${open ? "select-caret-open" : ""}`}>▾</span>
      </button>
      {open && listPos && (
        <div
          className="select-list"
          style={{ left: listPos.x, top: listPos.y, width: listPos.w }}
          onKeyDown={onListKeyDown}
          ref={listRef}
        >
          {searchable && (
            <input
              className="select-filter"
              ref={filterRef}
              value={filter}
              placeholder="Filter…"
              onChange={(e) => {
                setFilter(e.target.value);
                setActiveIdx(0);
              }}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
            />
          )}
          {filtered.map((o, i) => (
            <button
              type="button"
              key={o.value}
              className={`select-option ${o.value === value ? "select-option-selected" : ""} ${i === activeIdx ? "select-option-active" : ""}`}
              onMouseEnter={() => setActiveIdx(i)}
              onClick={() => {
                onChange(o.value);
                setOpen(false);
              }}
            >
              {o.label}
            </button>
          ))}
          {filtered.length === 0 && <div className="select-empty">No matches</div>}
        </div>
      )}
    </div>
  );
}