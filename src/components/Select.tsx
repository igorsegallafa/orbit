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
}

/**
 * Custom select: button + floating listbox (no native <select> — its chrome
 * renders inconsistently in the dark WKWebView). The list is positioned
 * FIXED from the trigger's viewport rect so it overlays scrollable
 * containers instead of being clipped by their overflow.
 */
export function Select({
  value,
  options,
  onChange,
  placeholder,
  className,
  disabled,
}: Props) {
  const [open, setOpen] = useState(false);
  const [activeIdx, setActiveIdx] = useState(0);
  const [listPos, setListPos] = useState<{ x: number; y: number; w: number } | null>(null);
  const rootRef = useRef<HTMLDivElement>(null);

  const openList = () => {
    const rect = rootRef.current?.getBoundingClientRect();
    if (rect) {
      setListPos({ x: rect.left, y: rect.bottom + 4, w: rect.width });
    }
    setActiveIdx(Math.max(0, options.findIndex((o) => o.value === value)));
    setOpen(true);
  };

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node;
      // clicks on the trigger toggle; clicks on the list pick; others close
      if (!rootRef.current?.contains(target) && !document.getElementById("orbit-select-list")?.contains(target)) {
        setOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    // Any ancestor scrolling detaches a fixed list — close instead.
    const onScroll = () => setOpen(false);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [open]);

  const current = options.find((o) => o.value === value);

  const onListKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIdx((i) => Math.min(i + 1, options.length - 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIdx((i) => Math.max(i - 1, 0));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const opt = options[activeIdx];
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
          id="orbit-select-list"
          className="select-list"
          style={{ left: listPos.x, top: listPos.y, width: listPos.w }}
          onKeyDown={onListKeyDown}
        >
          {options.map((o, i) => (
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
          {options.length === 0 && <div className="select-empty">No options</div>}
        </div>
      )}
    </div>
  );
}