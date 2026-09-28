interface Props {
  checked: boolean;
  /** Some but not all of what it stands for is checked (drawn as a dash). */
  mixed?: boolean;
  disabled?: boolean;
  /** Invoked on click/toggle. */
  onChange: (checked: boolean) => void;
  /** Accessible label for the toggle. */
  label: string;
}

/** Orbit checkbox: a styled toggle button, not a bare HTML input. */
export function CheckBox({ checked, mixed, disabled, onChange, label }: Props) {
  return (
    <button
      type="button"
      className={`orbit-check ${checked || mixed ? "orbit-check-on" : ""}`}
      disabled={disabled}
      aria-pressed={mixed ? "mixed" : checked}
      aria-label={label}
      onClick={() => onChange(!checked)}
    >
      <span className="orbit-check-box">
        {mixed && !checked && (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none">
            <path d="M2 5H8" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
          </svg>
        )}
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
