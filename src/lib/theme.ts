// App themes: one palette drives the CSS variables (App.css), Monaco's
// editor/diff theme and the terminals' ANSI colors. Switching applies live.
import { useEffect, useState } from "react";
import type * as Monaco from "monaco-editor";
import type { ITheme } from "@xterm/xterm";

/** The UI palette: every App.css color resolves to one of these. */
interface Ui {
  bg: string;
  panel: string;
  panelRaised: string;
  panelHover: string;
  border: string;
  borderStrong: string;
  text: string;
  textDim: string;
  textFaint: string;
  accent: string;
  accentHover: string;
  /** Text on an accent background (primary buttons, badges). */
  onAccent: string;
  danger: string;
  ok: string;
  warn: string;
  info: string;
  purple: string;
}

/** Code colors (Monaco tokens, LSP semantic tokens). */
interface Syntax {
  comment: string;
  keyword: string;
  string: string;
  number: string;
  type: string;
  function: string;
  namespace: string;
  parameter: string;
  property: string;
  constant: string;
}

/** ANSI 0-7 then 8-15 (bright). */
type Ansi = [string, string, string, string, string, string, string, string];

export interface Theme {
  id: string;
  label: string;
  dark: boolean;
  ui: Ui;
  syntax: Syntax;
  ansi: Ansi;
  ansiBright: Ansi;
}

export const THEMES: Theme[] = [
  {
    id: "orbit-dark",
    label: "Orbit Dark",
    dark: true,
    ui: {
      bg: "#0d0f13", panel: "#14171d", panelRaised: "#1a1e26", panelHover: "#232936",
      border: "#232833", borderStrong: "#2e3440",
      text: "#e6e8ec", textDim: "#9aa1ad", textFaint: "#6b7280",
      accent: "#4f7cf7", accentHover: "#6d92ff", onAccent: "#ffffff",
      danger: "#ff5d5d", ok: "#4ade80", warn: "#e3b341", info: "#7aa7ff", purple: "#b48cff",
    },
    syntax: {
      comment: "#6b7280", keyword: "#c792ea", string: "#a5d6a7", number: "#f78c6c", type: "#7aa7ff",
      function: "#82aaff", namespace: "#7fdbca", parameter: "#e6c07b", property: "#b4c5e4", constant: "#f7b267",
    },
    ansi: ["#1a1e26", "#ff5d5d", "#4ade80", "#e3b341", "#7aa7ff", "#c792ea", "#7fdbca", "#e6e8ec"],
    ansiBright: ["#4a5060", "#ff8080", "#7ee2a8", "#f2c96b", "#9fb8ff", "#d7a8f5", "#a3eadb", "#ffffff"],
  },
  {
    id: "orbit-light",
    label: "Orbit Light",
    dark: false,
    ui: {
      bg: "#f6f7f9", panel: "#ffffff", panelRaised: "#f0f2f5", panelHover: "#e7eaef",
      border: "#e1e4ea", borderStrong: "#cfd4dc",
      text: "#1d2129", textDim: "#555d6b", textFaint: "#8a919e",
      accent: "#3b6ef5", accentHover: "#2f5fe0", onAccent: "#ffffff",
      danger: "#d83b3b", ok: "#1f9d55", warn: "#b7791f", info: "#2f6fd6", purple: "#7c4dda",
    },
    syntax: {
      comment: "#8a919e", keyword: "#8e3fb5", string: "#2e7d32", number: "#c05621", type: "#1f62c4",
      function: "#3056c9", namespace: "#0f7b75", parameter: "#9a6700", property: "#3b5b8c", constant: "#b35c00",
    },
    ansi: ["#1d2129", "#d83b3b", "#1f9d55", "#b7791f", "#3b6ef5", "#8e3fb5", "#0f7b75", "#c9ced6"],
    ansiBright: ["#555d6b", "#e55b5b", "#2fb86a", "#d19a2a", "#5b86f7", "#a660cc", "#1a9990", "#f6f7f9"],
  },
  {
    id: "dracula",
    label: "Dracula",
    dark: true,
    ui: {
      bg: "#282a36", panel: "#21222c", panelRaised: "#2f3141", panelHover: "#3a3d4f",
      border: "#343746", borderStrong: "#44475a",
      text: "#f8f8f2", textDim: "#bfc2d4", textFaint: "#6272a4",
      accent: "#bd93f9", accentHover: "#caa9fa", onAccent: "#282a36",
      danger: "#ff5555", ok: "#50fa7b", warn: "#f1fa8c", info: "#8be9fd", purple: "#ff79c6",
    },
    syntax: {
      comment: "#6272a4", keyword: "#ff79c6", string: "#f1fa8c", number: "#bd93f9", type: "#8be9fd",
      function: "#50fa7b", namespace: "#8be9fd", parameter: "#ffb86c", property: "#f8f8f2", constant: "#bd93f9",
    },
    ansi: ["#21222c", "#ff5555", "#50fa7b", "#f1fa8c", "#bd93f9", "#ff79c6", "#8be9fd", "#f8f8f2"],
    ansiBright: ["#6272a4", "#ff6e6e", "#69ff94", "#ffffa5", "#d6acff", "#ff92df", "#a4ffff", "#ffffff"],
  },
  {
    id: "tokyo-night",
    label: "Tokyo Night",
    dark: true,
    ui: {
      bg: "#1a1b26", panel: "#16161e", panelRaised: "#1f2335", panelHover: "#292e42",
      border: "#232433", borderStrong: "#3b4261",
      text: "#c0caf5", textDim: "#a9b1d6", textFaint: "#565f89",
      accent: "#7aa2f7", accentHover: "#89b4fa", onAccent: "#1a1b26",
      danger: "#f7768e", ok: "#9ece6a", warn: "#e0af68", info: "#7dcfff", purple: "#bb9af7",
    },
    syntax: {
      comment: "#565f89", keyword: "#bb9af7", string: "#9ece6a", number: "#ff9e64", type: "#2ac3de",
      function: "#7aa2f7", namespace: "#7dcfff", parameter: "#e0af68", property: "#73daca", constant: "#ff9e64",
    },
    ansi: ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#a9b1d6"],
    ansiBright: ["#414868", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"],
  },
  {
    id: "catppuccin-mocha",
    label: "Catppuccin Mocha",
    dark: true,
    ui: {
      bg: "#1e1e2e", panel: "#181825", panelRaised: "#24273a", panelHover: "#313244",
      border: "#313244", borderStrong: "#45475a",
      text: "#cdd6f4", textDim: "#a6adc8", textFaint: "#6c7086",
      accent: "#89b4fa", accentHover: "#b4befe", onAccent: "#1e1e2e",
      danger: "#f38ba8", ok: "#a6e3a1", warn: "#f9e2af", info: "#89dceb", purple: "#cba6f7",
    },
    syntax: {
      comment: "#6c7086", keyword: "#cba6f7", string: "#a6e3a1", number: "#fab387", type: "#f9e2af",
      function: "#89b4fa", namespace: "#94e2d5", parameter: "#eba0ac", property: "#b4befe", constant: "#fab387",
    },
    ansi: ["#45475a", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#bac2de"],
    ansiBright: ["#585b70", "#f38ba8", "#a6e3a1", "#f9e2af", "#89b4fa", "#f5c2e7", "#94e2d5", "#a6adc8"],
  },
  {
    id: "nord",
    label: "Nord",
    dark: true,
    ui: {
      bg: "#2e3440", panel: "#292e39", panelRaised: "#343a47", panelHover: "#3b4252",
      border: "#3b4252", borderStrong: "#4c566a",
      text: "#eceff4", textDim: "#c0c8d6", textFaint: "#7b88a1",
      accent: "#88c0d0", accentHover: "#8fbcbb", onAccent: "#2e3440",
      danger: "#bf616a", ok: "#a3be8c", warn: "#ebcb8b", info: "#81a1c1", purple: "#b48ead",
    },
    syntax: {
      comment: "#616e88", keyword: "#81a1c1", string: "#a3be8c", number: "#b48ead", type: "#8fbcbb",
      function: "#88c0d0", namespace: "#8fbcbb", parameter: "#d8dee9", property: "#d8dee9", constant: "#d08770",
    },
    ansi: ["#3b4252", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#88c0d0", "#e5e9f0"],
    ansiBright: ["#4c566a", "#bf616a", "#a3be8c", "#ebcb8b", "#81a1c1", "#b48ead", "#8fbcbb", "#eceff4"],
  },
  {
    id: "solarized-light",
    label: "Solarized Light",
    dark: false,
    ui: {
      bg: "#fdf6e3", panel: "#eee8d5", panelRaised: "#f5efdc", panelHover: "#e6dfca",
      border: "#ddd6c1", borderStrong: "#c9c2ab",
      text: "#073642", textDim: "#586e75", textFaint: "#93a1a1",
      accent: "#268bd2", accentHover: "#1f76b5", onAccent: "#fdf6e3",
      danger: "#dc322f", ok: "#859900", warn: "#b58900", info: "#2aa198", purple: "#6c71c4",
    },
    syntax: {
      comment: "#93a1a1", keyword: "#859900", string: "#2aa198", number: "#d33682", type: "#b58900",
      function: "#268bd2", namespace: "#cb4b16", parameter: "#657b83", property: "#6c71c4", constant: "#cb4b16",
    },
    ansi: ["#073642", "#dc322f", "#859900", "#b58900", "#268bd2", "#d33682", "#2aa198", "#eee8d5"],
    ansiBright: ["#002b36", "#cb4b16", "#586e75", "#657b83", "#839496", "#6c71c4", "#93a1a1", "#fdf6e3"],
  },
];

/** A theme id, or "system": Orbit Dark / Orbit Light following the OS. */
export type ThemePref = string;

const STORAGE_KEY = "orbit.theme";
const darkQuery = window.matchMedia("(prefers-color-scheme: dark)");

export function themePref(): ThemePref {
  try {
    return localStorage.getItem(STORAGE_KEY) ?? "orbit-dark";
  } catch {
    return "orbit-dark";
  }
}

function resolve(pref: ThemePref): Theme {
  if (pref === "system") return THEMES.find((t) => t.id === (darkQuery.matches ? "orbit-dark" : "orbit-light"))!;
  return THEMES.find((t) => t.id === pref) ?? THEMES[0];
}

/** What "System" resolves to right now. */
export function systemTheme(): Theme {
  return resolve("system");
}

let current: Theme = resolve(themePref());
const listeners = new Set<(t: Theme) => void>();

export function currentTheme(): Theme {
  return current;
}

/** Runs `fn` on every theme change; returns the unsubscribe. */
export function onThemeChange(fn: (t: Theme) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

const CSS_VARS: Record<keyof Ui, string> = {
  bg: "--bg",
  panel: "--panel",
  panelRaised: "--panel-raised",
  panelHover: "--panel-hover",
  border: "--border",
  borderStrong: "--border-strong",
  text: "--text",
  textDim: "--text-dim",
  textFaint: "--text-faint",
  accent: "--accent",
  accentHover: "--accent-hover",
  onAccent: "--on-accent",
  danger: "--danger",
  ok: "--ok",
  warn: "--warn",
  info: "--info",
  purple: "--purple",
};

function apply(theme: Theme) {
  current = theme;
  const root = document.documentElement;
  for (const [key, name] of Object.entries(CSS_VARS)) root.style.setProperty(name, theme.ui[key as keyof Ui]);
  root.style.colorScheme = theme.dark ? "dark" : "light";
  root.style.background = theme.ui.bg;
  root.dataset.theme = theme.id;
  for (const fn of listeners) fn(theme);
}

export function setThemePref(pref: ThemePref) {
  try {
    localStorage.setItem(STORAGE_KEY, pref);
  } catch {
    // Not persisted (storage blocked); still applies for this session.
  }
  apply(resolve(pref));
}

/** Applies the saved theme; call once before the first render. */
export function initTheme() {
  apply(current);
  darkQuery.addEventListener("change", () => themePref() === "system" && apply(resolve("system")));
}

/** Re-renders on theme change; returns the active theme. */
export function useTheme(): Theme {
  const [theme, setTheme] = useState(current);
  useEffect(() => onThemeChange(setTheme), []);
  return theme;
}

// ---------- Monaco ----------

/** `a` blended over `b` by `t` (0..1), as #rrggbb. */
function mix(a: string, b: string, t: number): string {
  const p = (h: string) => [1, 3, 5].map((i) => parseInt(h.slice(i, i + 2), 16));
  const [x, y] = [p(a), p(b)];
  return "#" + x.map((v, i) => Math.round(v * t + y[i] * (1 - t)).toString(16).padStart(2, "0")).join("");
}

/** `hex` at opacity `a` (0..1), as #rrggbbaa. */
function alpha(hex: string, a: number): string {
  return hex + Math.round(a * 255).toString(16).padStart(2, "0");
}

const bare = (hex: string) => hex.slice(1);

export const monacoThemeName = (theme: Theme) => `orbit-${theme.id}`;

/** Defines every app theme in Monaco (idempotent) so `theme=` can switch. */
export function defineMonacoThemes(monaco: typeof Monaco) {
  for (const t of THEMES) {
    const { ui, syntax: s } = t;
    const tok = (token: string, color: string, fontStyle?: string) => ({ token, foreground: bare(color), ...(fontStyle ? { fontStyle } : {}) });
    monaco.editor.defineTheme(monacoThemeName(t), {
      base: t.dark ? "vs-dark" : "vs",
      inherit: true,
      rules: [
        tok("comment", s.comment, "italic"),
        tok("keyword", s.keyword),
        tok("string", s.string),
        tok("number", s.number),
        tok("type", s.type),
        tok("function", s.function),
        tok("variable", ui.text),
        tok("delimiter", ui.textDim),
        // Semantic tokens from language servers (what a name *is*, not how it looks).
        tok("namespace", s.namespace),
        tok("class", s.type),
        tok("struct", s.type),
        tok("interface", s.type),
        tok("enum", s.type),
        tok("typeParameter", s.type, "italic"),
        tok("concept", s.keyword),
        tok("method", s.function),
        tok("macro", s.number),
        tok("parameter", s.parameter),
        tok("property", s.property),
        tok("enumMember", s.constant),
        tok("variable.readonly", s.constant),
        tok("label", ui.textDim),
      ],
      colors: {
        "editor.background": ui.bg,
        "editor.foreground": ui.text,
        "editorLineNumber.foreground": mix(ui.textFaint, ui.bg, 0.6),
        "editorLineNumber.activeForeground": ui.textDim,
        // Translucent, like the diff backgrounds below: Monaco paints those
        // over the selection, so opaque ones hid what's selected.
        "editor.selectionBackground": alpha(ui.accent, 0.35),
        "editor.inactiveSelectionBackground": alpha(ui.accent, 0.2),
        "editor.lineHighlightBackground": ui.panel,
        "editorCursor.foreground": ui.accent,
        "editorIndentGuide.background1": ui.panelRaised,
        "editorIndentGuide.activeBackground1": ui.borderStrong,
        "editorWidget.background": ui.panel,
        "editorWidget.border": ui.border,
        "editorGutter.background": ui.bg,
        "scrollbarSlider.background": ui.borderStrong + "80",
        "scrollbarSlider.hoverBackground": mix(ui.textFaint, ui.borderStrong, 0.3),
        "scrollbarSlider.activeBackground": mix(ui.textFaint, ui.bg, 0.6),
        "editorBracketMatch.background": mix(ui.accent, ui.bg, 0.28),
        "editorBracketMatch.border": ui.accent,
        "diffEditor.insertedTextBackground": alpha(ui.ok, t.dark ? 0.16 : 0.22),
        "diffEditor.removedTextBackground": alpha(ui.danger, t.dark ? 0.16 : 0.2),
        "diffEditor.insertedLineBackground": alpha(ui.ok, t.dark ? 0.09 : 0.11),
        "diffEditor.removedLineBackground": alpha(ui.danger, t.dark ? 0.09 : 0.1),
      },
    });
  }
}

/** The Monaco theme name to pass as `theme=`, following the app theme. */
export function useMonacoTheme(): string {
  return monacoThemeName(useTheme());
}

// ---------- Terminal ----------

export function terminalTheme(theme: Theme): ITheme {
  const { ui, ansi: a, ansiBright: b } = theme;
  return {
    background: ui.bg,
    foreground: ui.text,
    cursor: ui.accent,
    cursorAccent: ui.bg,
    selectionBackground: mix(ui.accent, ui.bg, 0.35),
    black: a[0], red: a[1], green: a[2], yellow: a[3], blue: a[4], magenta: a[5], cyan: a[6], white: a[7],
    brightBlack: b[0], brightRed: b[1], brightGreen: b[2], brightYellow: b[3],
    brightBlue: b[4], brightMagenta: b[5], brightCyan: b[6], brightWhite: b[7],
  };
}
