import { useEffect, useRef, useState } from "react";
import Editor, { BeforeMount, OnMount, loader } from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";
import { MarkdownPreview } from "./MarkdownPreview";
import { Select } from "./Select";
import { CodeViewIcon, EyeIcon, RefreshIcon } from "./Icons";
import { SymbolSearch } from "./SymbolSearch";
import { tooltip } from "./Tooltip";
import { toast } from "./Toast";
import * as lsp from "../lib/lsp/manager";
import { currentTheme, defineMonacoThemes, monacoThemeName, onThemeChange, useMonacoTheme } from "../lib/theme";

interface CppSetup {
  compileCommands: string | null;
  cmake: boolean;
  cmakeInstalled: boolean;
  ninjaInstalled: boolean;
  presets: string[];
  /** Windows: Visual Studio's vcvars64.bat, loaded before running CMake. */
  vcvars: string | null;
}

// Per repo folder: clangd's build setup, banners the user dismissed, CMake
// runs started from the banner, and folders seen without a database (clangd
// started there needs a restart once one shows up).
const cppSetups = new Map<string, CppSetup>();
const cppDismissed = new Set<string>();
const cppGeneratingKeys = new Set<string>();
const cppMissing = new Set<string>();

const beforeMount: BeforeMount = (monaco) => {
  defineMonacoThemes(monaco);
};

/** A spot to show once the file is open (Find in Files results). */
export interface RevealTarget {
  workspace: string;
  repo: string;
  path: string;
  line: number;
  col: number;
  length: number;
}

// Kept until the editor for that file has mounted (the tab may be opening).
const pendingReveal = new Map<string, RevealTarget>();
const revealKey = (t: { workspace: string; repo: string; path: string }) => `${t.workspace}/${t.repo}/${t.path}`;

/** Scrolls the file's editor to the target and selects it; call right after opening its tab. */
export function revealInEditor(t: RevealTarget) {
  pendingReveal.set(revealKey(t), t);
  window.dispatchEvent(new CustomEvent("orbit-reveal", { detail: t }));
}

// Define the themes as soon as the monaco loader resolves (module scope, runs
// once for the whole app) — the component prop "theme" then always finds
// them registered, regardless of mount order or HMR state. Monaco's theme
// is global: follow the app theme for every open editor.
loader
  .init()
  .then((monaco) => {
    defineMonacoThemes(monaco as unknown as typeof Monaco);
    onThemeChange((t) => monaco.editor.setTheme(monacoThemeName(t)));
  })
  .catch(() => null);

interface Props {
  workspace: string;
  repo: string;
  path: string;
  onError: (msg: string) => void;
  /** Opens an agent terminal to execute this plan (PLAN.md only). */
  onApplyPlan?: (agent: string, model: string) => void;
  /** Runs a command in a terminal tab in this repo's folder (e.g. cmake). */
  onRunInTerminal?: (label: string, cmd: string, args: string[], repo: string) => void;
  /** Cursor moved (navigation history remembers where you were). */
  onCursor?: (line: number, column: number) => void;
  /** The cursor jumped far (go to definition, a click elsewhere): a place to come back to. */
  onJump?: (from: { line: number; column: number }, to: { line: number; column: number }) => void;
}

/** Cursor moves at least this far apart count as navigation (as in VS Code). */
const JUMP_LINES = 10;

/**
 * Single-file editor tab: Monaco + file bar (path, dirty dot, save).
 * Markdown files get a JetBrains-style Editor/Preview toggle in the bar.
 * PLAN.md (workspace root) additionally gets an Apply bar: pick the agent
 * + model and launch a terminal session that executes the plan.
 * The file tree lives in the fixed right dock (FileTreePanel).
 */
export function EditorPane({ workspace, repo, path, onError, onApplyPlan, onRunInTerminal, onCursor, onJump }: Props) {
  const monacoTheme = useMonacoTheme();
  // Latest callbacks for the editor's listeners (registered once on mount).
  const navRef = useRef({ onCursor, onJump });
  navRef.current = { onCursor, onJump };
  // The next cursor move is a reveal (opening at a spot), not the user jumping.
  const revealing = useRef(false);
  const [content, setContent] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [mode, setMode] = useState<"editor" | "preview">("editor");
  const [ai, setAi] = useState<{ agent: string; model: string } | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const saveRef = useRef<(() => void) | null>(null);
  const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);
  // The file's real location: language servers speak in file URIs.
  const [absPath, setAbsPath] = useState<string | null>(null);
  const [lspStatus, setLspStatus] = useState<lsp.LspStatus>({ state: "off", language: null, server: null, progress: null, error: null });
  const [symbolsOpen, setSymbolsOpen] = useState(false);
  const [cppSetup, setCppSetup] = useState<CppSetup | null>(null);
  const [cppPreset, setCppPreset] = useState<string>("");
  const setupKey = `${workspace}|${repo}`;
  const [cppGenerating, setCppGenerating] = useState(() => cppGeneratingKeys.has(setupKey));
  const lspOff = useRef<(() => void)[]>([]);

  const applyReveal = () => {
    const key = revealKey({ workspace, repo, path });
    const t = pendingReveal.get(key);
    const ed = editorRef.current;
    if (!t || !ed) return;
    pendingReveal.delete(key);
    revealing.current = true;
    // The cursor may already sit there (no move event): don't swallow a later jump.
    window.setTimeout(() => (revealing.current = false), 600);
    // After the tab becomes visible, so the layout has a real size.
    requestAnimationFrame(() => {
      ed.revealLineInCenter(t.line);
      ed.setSelection({ startLineNumber: t.line, startColumn: t.col, endLineNumber: t.line, endColumn: t.col + t.length });
      ed.focus();
    });
  };

  useEffect(() => {
    const onReveal = (e: Event) => {
      const t = (e as CustomEvent<RevealTarget>).detail;
      if (revealKey(t) === revealKey({ workspace, repo, path })) {
        setMode("editor");
        applyReveal();
      }
    };
    window.addEventListener("orbit-reveal", onReveal);
    return () => window.removeEventListener("orbit-reveal", onReveal);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, repo, path]);

  const isMarkdown = /\.(md|markdown)$/i.test(path);
  const isPlan = path === "PLAN.md" && repo === "";

  // Load AI settings + model list for the Apply bar
  useEffect(() => {
    if (!isPlan) return;
    invoke<{ agent: string; model: string }>("get_ai_settings")
      .then(setAi)
      .catch(() => null);
  }, [isPlan]);

  useEffect(() => {
    if (!isPlan || !ai) return;
    invoke<string[]>("list_models", { agentName: ai.agent })
      .then(setModels)
      .catch(() => setModels([]));
  }, [isPlan, ai?.agent]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        saveRef.current?.();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    setDirty(false);
    setMode("editor");
    // Empty path = repo browsing mode (no file open): the Files dock lists
    // the repo; trying to read "" would just error at the user.
    if (!path) {
      setContent(null);
      return;
    }
    invoke<string>("read_file", { workspace, repo, path })
      .then(setContent)
      .catch((e) => {
        setContent(null);
        onError(String(e));
      });
    invoke<string>("node_abs_path", { workspace, repo, path })
      .then(setAbsPath)
      .catch(() => setAbsPath(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, repo, path]);

  // Leaving the tab: the language server stops tracking the file.
  useEffect(() => () => lspOff.current.forEach((off) => off()), []);

  // clangd without a compilation database guesses includes and flags:
  // check once per repo folder, offer to generate one.
  const cppReady = lspStatus.language === "cpp" && lspStatus.state === "ready";
  useEffect(() => {
    if (!cppReady || !repo) return;
    const apply = (s: CppSetup) => {
      setCppSetup(s);
      setCppPreset(s.presets.find((p) => /debug/i.test(p)) ?? s.presets[0] ?? "");
    };
    const cached = cppSetups.get(setupKey);
    if (cached) {
      apply(cached);
      return;
    }
    invoke<CppSetup>("lsp_cpp_setup", { workspace, repo })
      .then((s) => {
        // A missing database is checked again next time: it may get generated.
        if (!s.compileCommands) {
          cppMissing.add(setupKey);
          apply(s);
        } else if (cppMissing.has(setupKey)) {
          // Generated since clangd started (outside the banner, or while
          // this editor was closed): clangd only reads it on start.
          restartServer();
        } else {
          cppSetups.set(setupKey, s);
          apply(s);
        }
      })
      .catch(() => null);
  }, [cppReady, workspace, repo, setupKey]);

  // While CMake runs in the terminal, watch for its compile_commands.json
  // and restart clangd with it once it's there.
  useEffect(() => {
    if (!cppGenerating || !repo) return;
    const timer = window.setInterval(async () => {
      const s = await invoke<CppSetup>("lsp_cpp_setup", { workspace, repo }).catch(() => null);
      if (!s?.compileCommands) return;
      window.clearInterval(timer);
      toast.success("compile_commands.json generated", { description: "Restarting clangd with it" });
      restartServer();
    }, 3000);
    return () => window.clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cppGenerating, workspace, repo]);

  const save = async () => {
    if (content === null) return;
    try {
      await invoke("write_file", { workspace, repo, path, content });
      setDirty(false);
      const model = editorRef.current?.getModel();
      if (model) lsp.saved(model);
    } catch (e) {
      onError(String(e));
    }
  };
  saveRef.current = save;

  const restartServer = () => {
    const model = editorRef.current?.getModel();
    if (!model) return;
    cppSetups.delete(setupKey);
    cppGeneratingKeys.delete(setupKey);
    cppMissing.delete(setupKey);
    setCppSetup(null);
    setCppGenerating(false);
    void lsp.restart(model);
  };

  const generateCompileCommands = async () => {
    if (!cppSetup || !onRunInTerminal) return;
    // Visual Studio's Ninja is only on PATH inside its developer environment.
    const ninja = cppSetup.ninjaInstalled || !!cppSetup.vcvars;
    const args = cppPreset
      ? ["--preset", cppPreset, "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON"]
      : ["-S", ".", "-B", "build", ...(ninja ? ["-G", "Ninja"] : []), "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON"];
    if (cppSetup.vcvars) {
      try {
        const script = await invoke<string>("lsp_cmake_script", { vcvars: cppSetup.vcvars, args });
        onRunInTerminal("cmake", "cmd", ["/d", "/c", script], repo);
      } catch (e) {
        onError(String(e));
        return;
      }
    } else {
      onRunInTerminal("cmake", "cmake", args, repo);
    }
    cppGeneratingKeys.add(setupKey);
    setCppGenerating(true);
  };

  const onMount: OnMount = (editor, monaco) => {
    // Belt & suspenders: ensure the theme is applied even if beforeMount
    // raced with the monaco loader.
    defineMonacoThemes(monaco);
    monaco.editor.setTheme(monacoThemeName(currentTheme()));
    editorRef.current = editor;
    applyReveal();
    editor.onDidChangeModelContent(() => {
      setContent(editor.getValue());
      setDirty(true);
    });
    // Navigation history: every cursor move updates "where you are"; far
    // moves (not edits) become places to go back to.
    let last = editor.getPosition();
    const R = monaco.editor.CursorChangeReason;
    editor.onDidChangeCursorPosition((e) => {
      const from = last;
      last = e.position;
      navRef.current.onCursor?.(e.position.lineNumber, e.position.column);
      const byEdit = e.source === "modelChange" || [R.ContentFlush, R.RecoverFromMarkers, R.Paste, R.Undo, R.Redo].includes(e.reason);
      if (revealing.current) {
        revealing.current = false;
        return;
      }
      if (!from || byEdit || Math.abs(e.position.lineNumber - from.lineNumber) < JUMP_LINES) return;
      navRef.current.onJump?.({ line: from.lineNumber, column: from.column }, { line: e.position.lineNumber, column: e.position.column });
    });
    editor.addAction({
      id: "orbit.workspaceSymbols",
      label: "Go to Symbol in Project…",
      keybindings: [monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyT],
      run: () => setSymbolsOpen(true),
    });
    const model = editor.getModel();
    if (model && path) {
      const refresh = () => setLspStatus(lsp.status(model));
      lspOff.current.push(lsp.attach(model, { workspace, repo, path }), lsp.onStatus(model, refresh));
      refresh();
    }
  };

  const hint = (text: string) => ({
    onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
    onMouseLeave: () => tooltip.hide(),
  });
  const showCppBanner =
    cppReady && !!cppSetup && !cppSetup.compileCommands && !cppDismissed.has(setupKey) && !!onRunInTerminal;

  return (
    <div className="editor-pane">
      <div className="editor-filebar">
        <span className="mono">
          {path ? `${repo}/${path}` : repo}
          {dirty ? " •" : ""}
        </span>
        <span className="editor-filebar-actions">
          {isPlan && ai && onApplyPlan && (
            <span className="plan-apply-bar">
              <Select
                className="plan-apply-select"
                value={ai.agent}
                options={[
                  { value: "claude", label: "Claude Code" },
                  { value: "opencode", label: "OpenCode" },
                  { value: "omp", label: "OMP" },
                ]}
                onChange={(a) => {
                  const next =
                    a === "claude"
                      ? (models.find((m) => m.startsWith("claude")) ?? models[0] ?? "")
                      : (models.find((m) => m.startsWith("aihub")) ?? models[0] ?? "");
                  setAi({ agent: a, model: next });
                }}
              />
              <Select
                className="plan-apply-select model-select"
                value={ai.model}
                options={models.map((m) => ({ value: m, label: m }))}
                onChange={(m) => setAi({ ...ai, model: m })}
                searchable
                listWidth={340}
              />
              <button className="btn-mini" onClick={() => onApplyPlan(ai.agent, ai.model)}>
                Apply
              </button>
            </span>
          )}
          {isMarkdown && (
            <span className="mode-toggle">
              <button
                className={`mode-btn ${mode === "editor" ? "mode-active" : ""}`}
                title="Editor"
                onClick={() => setMode("editor")}
              >
                <CodeViewIcon size={13} />
              </button>
              <button
                className={`mode-btn ${mode === "preview" ? "mode-active" : ""}`}
                title="Preview"
                onClick={() => setMode("preview")}
              >
                <EyeIcon size={13} />
              </button>
            </span>
          )}
          <LspChip status={lspStatus} onRestart={restartServer} hint={hint} />
          <button className="btn-mini" onClick={save} disabled={!dirty}>
            Save
          </button>
        </span>
      </div>
      {showCppBanner && (
        <div className="editor-banner">
          <span>
            <strong>clangd has no compile_commands.json</strong> for {repo}, so it guesses include paths and flags: some
            navigation and errors will be off.
          </span>
          {cppGenerating ? (
            <>
              <span className="editor-banner-note">When CMake finishes in the terminal:</span>
              <button className="btn-mini" onClick={restartServer}>
                Restart clangd
              </button>
            </>
          ) : cppSetup!.cmake && (cppSetup!.cmakeInstalled || cppSetup!.vcvars) ? (
            <>
              {cppSetup!.presets.length > 0 && (
                <Select
                  className="editor-banner-select"
                  value={cppPreset}
                  options={cppSetup!.presets.map((p) => ({ value: p, label: p }))}
                  onChange={setCppPreset}
                />
              )}
              <button
                className="btn-mini"
                onClick={generateCompileCommands}
                {...hint(
                  (cppPreset
                    ? `cmake --preset ${cppPreset} -DCMAKE_EXPORT_COMPILE_COMMANDS=ON`
                    : "cmake -S . -B build -DCMAKE_EXPORT_COMPILE_COMMANDS=ON (needs the Ninja or Makefile generator)") +
                    (cppSetup!.vcvars ? ", in the Visual Studio developer environment" : ""),
                )}
              >
                Generate with CMake
              </button>
            </>
          ) : (
            <span className="editor-banner-note">
              {cppSetup!.cmake ? "Install CMake to generate it." : "Generate it with your build system (CMake, or bear -- make)."}
            </span>
          )}
          <button
            className="icon-button editor-banner-close"
            aria-label="Dismiss"
            onClick={() => {
              cppDismissed.add(setupKey);
              setCppSetup({ ...cppSetup! });
            }}
          >
            ×
          </button>
        </div>
      )}
      {symbolsOpen && editorRef.current?.getModel() && (
        <SymbolSearch model={editorRef.current.getModel()!} onClose={() => setSymbolsOpen(false)} />
      )}
      {content !== null ? (
        mode === "preview" && isMarkdown ? (
          <MarkdownPreview content={content} />
        ) : (
          <div className="editor-host">
            {absPath && (
            <Editor
              height="100%"
              theme={monacoTheme}
              beforeMount={beforeMount}
              path={lsp.fileUri(absPath)}
              value={content}
              onMount={onMount}
              options={{
                fontSize: 13,
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                automaticLayout: true,
                "semanticHighlighting.enabled": true,
              }}
              loading={<div className="table-loading"><span className="spinner" /> Loading…</div>}
            />
            )}
          </div>
        )
      ) : (
        <div className="editor-empty">
          {path
            ? "Pick a file in the Files panel"
            : `Browsing ${repo} — pick a file in the Files panel to open it`}
        </div>
      )}
    </div>
  );
}

/** Language server state for the open file, in the file bar. */
function LspChip({
  status,
  onRestart,
  hint,
}: {
  status: lsp.LspStatus;
  onRestart: () => void;
  hint: (text: string) => { onMouseEnter: (e: React.MouseEvent) => void; onMouseLeave: () => void };
}) {
  switch (status.state) {
    case "off":
      return null;
    case "starting":
      return (
        <span className="lsp-chip">
          <span className="spinner" /> Starting language server
        </span>
      );
    case "missing":
      return (
        <span className="lsp-chip lsp-chip-missing" {...hint(`${status.error ?? ""}\n\nSettings → Languages lets you point Orbit at a server.`)}>
          No language server
        </span>
      );
    case "exited":
      return (
        <button className="btn-plain lsp-chip lsp-chip-exited" onClick={onRestart} {...hint(status.error ?? "")}>
          {status.server} stopped · Restart
        </button>
      );
    case "ready":
      return (
        <span className="lsp-chip lsp-chip-ready" {...hint(status.progress ?? `${status.server}: go to definition (F12), references (Shift+F12), symbols (Ctrl+Shift+O, Ctrl+T)`)}>
          {status.progress ? <span className="spinner" /> : <span className="lsp-dot" />}
          <span className="lsp-chip-text">{status.progress ? `${status.server} · ${status.progress}` : status.server}</span>
          <button className="btn-plain lsp-chip-restart" aria-label="Restart language server" onClick={onRestart}>
            <RefreshIcon size={11} />
          </button>
        </span>
      );
  }
}