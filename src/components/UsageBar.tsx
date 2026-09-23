import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { tooltip } from "./Tooltip";

interface RateWindow {
  usedPercentage: number;
  resetsAt: number;
}

interface RateLimits {
  fiveHour: RateWindow | null;
  sevenDay: RateWindow | null;
  updatedAt: number | null;
}

function fmtIn(unix: number): string {
  const m = Math.max(0, Math.round((unix - Date.now() / 1000) / 60));
  if (m < 60) return `${m}m`;
  if (m < 60 * 24) return `${Math.floor(m / 60)}h ${m % 60}m`;
  return `${Math.floor(m / 1440)}d ${Math.floor((m % 1440) / 60)}h`;
}

/** Claude subscription window: label, fill bar and percentage; reset time on hover. */
function RateChip({ label, name, w }: { label: string; name: string; w: RateWindow }) {
  const pct = Math.min(100, Math.max(0, w.usedPercentage));
  const level = pct >= 90 ? "crit" : pct >= 80 ? "warn" : "";
  const resets = new Date(w.resetsAt * 1000).toLocaleString([], { weekday: "short", hour: "2-digit", minute: "2-digit" });
  return (
    <span
      className={`rate-chip ${level}`}
      onMouseEnter={(e) => tooltip.show(`Claude ${name} limit: ${pct.toFixed(0)}% used · resets in ${fmtIn(w.resetsAt)} (${resets})`, e)}
      onMouseLeave={() => tooltip.hide()}
    >
      {label}
      <span className="rate-bar">
        <span style={{ width: `${pct}%` }} />
      </span>
      {pct.toFixed(0)}%
    </span>
  );
}

interface SessionDetail {
  title: string;
  model: string;
  input_tokens: number;
  output_tokens: number;
  cache_tokens: number;
  cost_usd: number;
  last_active: number;
}

interface ModelUsage {
  model: string;
  input_tokens: number;
  output_tokens: number;
  cache_tokens: number;
  cost_usd: number;
  sessions: SessionDetail[];
}

interface AiUsage {
  by_model: ModelUsage[];
  sessions: number;
}

interface Props {
  workspace: string;
}

function fmtTokens(n: number): string {
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function fmtAgo(unix: number): string {
  if (!unix) return "";
  const s = Math.max(0, Date.now() / 1000 - unix);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86_400)}d ago`;
}

/**
 * Bottom bar with AI usage for the focused workspace. Hovering a model chip
 * opens a popup with that model's sessions (title, tokens, cost, activity).
 * The popup is rendered fixed-position OUTSIDE the stats row because that row
 * has overflow:hidden (chips must not stretch the bar) — an absolutely
 * positioned child would be clipped.
 */
export function UsageBar({ workspace }: Props) {
  const [usage, setUsage] = useState<AiUsage | null>(null);
  const [popup, setPopup] = useState<{ model: ModelUsage; x: number } | null>(null);
  const [limits, setLimits] = useState<RateLimits | null>(null);

  const load = useCallback(async () => {
    try {
      setUsage(await invoke<AiUsage>("workspace_ai_usage", { name: workspace }));
    } catch {
      setUsage(null);
    }
    invoke<RateLimits>("claude_rate_limits").then(setLimits).catch(() => null);
  }, [workspace]);

  useEffect(() => {
    setUsage(null);
    load();
    const t = globalThis.setInterval(load, 30_000);
    return () => globalThis.clearInterval(t);
  }, [load]);

  const openPopup = (m: ModelUsage, el: HTMLElement) => {
    const rect = el.getBoundingClientRect();
    const x = Math.min(rect.left, window.innerWidth - 410);
    setPopup({ model: m, x: Math.max(8, x) });
  };

  return (
    <footer className="usage-bar">
      {usage && usage.by_model.length > 0 ? (
        <span className="usage-stats">
          {usage.by_model.map((m) => (
            <span
              key={m.model}
              className="usage-model"
              onMouseEnter={(e) => openPopup(m, e.currentTarget)}
              onMouseLeave={() => setPopup(null)}
            >
              {m.model}
              <span className="usage-nums">
                {" "}
                {fmtTokens(m.input_tokens)} in / {fmtTokens(m.output_tokens)} out
              </span>
            </span>
          ))}
        </span>
      ) : (
        <span className="usage-dim">{usage ? "no AI usage yet" : "loading…"}</span>
      )}
      <span className="rate-limits">
        {limits?.fiveHour || limits?.sevenDay ? (
          <>
            {limits.fiveHour && <RateChip label="5h" name="5-hour" w={limits.fiveHour} />}
            {limits.sevenDay && <RateChip label="Week" name="weekly" w={limits.sevenDay} />}
          </>
        ) : (
          <span
            className="rate-chip rate-empty"
            onMouseEnter={(e) => tooltip.show("Claude plan limits (Pro/Max) show up after the first reply of a Claude session opened in Orbit", e)}
            onMouseLeave={() => tooltip.hide()}
          >
            Limits —
          </span>
        )}
      </span>
      <span className="usage-dim usage-sessions">
        {/* A repo view's scope ("@repo") reads as the repo's name. */}
        {usage && usage.sessions > 0 ? `${usage.sessions} sessions · ${workspace.replace(/^@/, "")}` : workspace.replace(/^@/, "")}
      </span>

      {popup && (
        <div className="usage-popup" style={{ left: popup.x }}>
          <div className="usage-popup-title">{popup.model.model}</div>
          <div className="usage-popup-grid">
            <span className="usage-popup-label">Input</span>
            <span className="usage-popup-value">{fmtTokens(popup.model.input_tokens)} tokens</span>
            <span className="usage-popup-label">Output</span>
            <span className="usage-popup-value">{fmtTokens(popup.model.output_tokens)} tokens</span>
            <span className="usage-popup-label">Cache</span>
            <span className="usage-popup-value">{fmtTokens(popup.model.cache_tokens)} tokens</span>
            <span className="usage-popup-label">Cost</span>
            <span className="usage-popup-value">
              {popup.model.cost_usd > 0 ? `$${popup.model.cost_usd.toFixed(2)}` : "—"}
            </span>
            <span className="usage-popup-label">Sessions</span>
            <span className="usage-popup-value">{popup.model.sessions.length}</span>
            <span className="usage-popup-label">Last active</span>
            <span className="usage-popup-value">
              {popup.model.sessions[0]?.last_active
                ? fmtAgo(popup.model.sessions[0].last_active)
                : "—"}
            </span>
          </div>
        </div>
      )}
    </footer>
  );
}