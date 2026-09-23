import { open as openDialog } from "@tauri-apps/plugin-dialog";

interface Props {
  value: string;
  onChange: (path: string) => void;
  placeholder?: string;
  disabled?: boolean;
  /** Maps the picked folder to the stored value (e.g. parent + repo name). */
  fromPicked?: (folder: string) => string;
}

/** Text field for a folder path with a native "Browse…" picker. */
export function PathInput({ value, onChange, placeholder, disabled, fromPicked }: Props) {
  const browse = async () => {
    const picked = await openDialog({ directory: true, defaultPath: value || undefined });
    if (typeof picked === "string") onChange(fromPicked ? fromPicked(picked) : picked);
  };

  return (
    <div className="path-input">
      <input className="mono" value={value} placeholder={placeholder} disabled={disabled} onChange={(e) => onChange(e.target.value)} />
      <button type="button" className="secondary" disabled={disabled} onClick={browse}>
        Browse…
      </button>
    </div>
  );
}
