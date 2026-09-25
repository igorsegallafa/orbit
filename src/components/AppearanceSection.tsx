import { useState } from "react";
import { THEMES, Theme, setThemePref, systemTheme, themePref, useTheme } from "../lib/theme";

/** A miniature of the app in the theme's colors: sidebar, a panel with
 *  text lines, a primary button and the status colors. */
function ThemePreview({ theme }: { theme: Theme }) {
  const { ui } = theme;
  return (
    <div className="theme-preview" style={{ background: ui.bg, borderColor: ui.border }}>
      <div className="theme-preview-side" style={{ background: ui.panel, borderColor: ui.border }}>
        <span style={{ background: ui.accent }} />
        <span style={{ background: ui.textFaint }} />
        <span style={{ background: ui.textFaint }} />
      </div>
      <div className="theme-preview-main">
        <span className="theme-preview-line" style={{ background: ui.text, width: "70%" }} />
        <span className="theme-preview-line" style={{ background: ui.textDim, width: "50%" }} />
        <div className="theme-preview-card" style={{ background: ui.panelRaised, borderColor: ui.border }}>
          <span style={{ background: ui.ok }} />
          <span style={{ background: ui.warn }} />
          <span style={{ background: ui.danger }} />
          <span style={{ background: ui.purple }} />
        </div>
        <span className="theme-preview-btn" style={{ background: ui.accent }} />
      </div>
    </div>
  );
}

function ThemeCard({ theme, label, sub, selected, onPick }: { theme: Theme; label: string; sub: string; selected: boolean; onPick: () => void }) {
  return (
    <button type="button" className={`btn-plain theme-card ${selected ? "on" : ""}`} aria-pressed={selected} onClick={onPick}>
      <ThemePreview theme={theme} />
      <span className="theme-card-label">
        {label}
        <span className="theme-card-sub">{sub}</span>
      </span>
    </button>
  );
}

/** Settings → Appearance: the app theme, applied live (UI, editor, diffs,
 *  terminals) and remembered on this machine. */
export function AppearanceSection() {
  useTheme(); // re-render on changes (System follows the OS live)
  const system = systemTheme();
  const [pref, setPref] = useState(themePref);
  const pick = (p: string) => {
    setPref(p);
    setThemePref(p);
  };
  const dark = THEMES.filter((t) => t.dark);
  const light = THEMES.filter((t) => !t.dark);

  return (
    <div className="section">
      <div className="settings-card">
        <div className="settings-row settings-row-stack">
          <div className="settings-row-text">
            <strong>Theme</strong>
            <span>Colors for the whole app, the editor, diffs and terminals. System follows your OS light/dark setting.</span>
          </div>
          <div className="theme-grid">
            <ThemeCard
              theme={system}
              label="System"
              sub={`Now ${system.dark ? "dark" : "light"}`}
              selected={pref === "system"}
              onPick={() => pick("system")}
            />
            {[...dark, ...light].map((t) => (
              <ThemeCard key={t.id} theme={t} label={t.label} sub={t.dark ? "Dark" : "Light"} selected={pref === t.id} onPick={() => pick(t.id)} />
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
