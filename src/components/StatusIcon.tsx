import { CheckIcon, XIcon } from "./Icons";

export type StatusKind = "pending" | "working" | "ok" | "error" | "warn";

/** One consistent state marker for per-repo progress rows. */
export function StatusIcon({ kind }: { kind: StatusKind }) {
  return (
    <span className={`status-icon status-${kind}`}>
      {kind === "working" ? (
        <span className="spinner" />
      ) : kind === "ok" ? (
        <CheckIcon size={11} />
      ) : kind === "error" ? (
        <XIcon size={10} />
      ) : kind === "warn" ? (
        "!"
      ) : null}
    </span>
  );
}
