interface Props {
  checked: boolean;
  disabled?: boolean;
  /** Invoked on click/toggle. */
  onChange: (checked: boolean) => void;
  /** Accessible label for the toggle. */
  label: string;
}

/** Orbit checkbox: a styled toggle button, not a bare HTML input. */
export function CheckBox({ checked, disabled, onChange, label }: Props) {
  return (
    <button
      type="button"
      className={`orbit-check ${checked ? "orbit-check-on" : ""}`}
      disabled={disabled}
      aria-pressed={checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
    >
      <span className="orbit-check-box">
        {checked && (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
            <path
              d="M1.5 5.5L4 8L8.5 2"
              stroke="currentColor"
              strokeWidth="2"
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        )}
      </span>
    </button>
  );
}
