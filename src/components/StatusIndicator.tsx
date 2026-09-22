import { AgentStatus } from "../lib/agentStatus";

interface Props {
  status: AgentStatus;
  withLabel?: boolean;
}

/**
 * Animated status indicator (Orca-style): a small rotating arc while the
 * agent is working, a pulsing dot when merely active, static shapes for
 * idle/exited. No emoji — pure CSS so it stays crisp at any size.
 */
export function StatusIndicator({ status, withLabel }: Props) {
  const cls =
    status === "thinking" || status === "running" || status === "editing"
      ? "status-work"
      : status === "busy"
        ? "status-busy"
        : status === "waiting"
          ? "status-waiting"
        : status === "exited"
          ? "status-exited"
          : "status-idle";

  return (
    <span className={`status-wrap ${cls}`}>
      <span className="status-ind" />
      {withLabel && <span className="status-label">{status}</span>}
    </span>
  );
}