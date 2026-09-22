import { useEffect, useRef, useState } from "react";
import Editor, { BeforeMount, OnMount, loader } from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";
import { MarkdownPreview } from "./MarkdownPreview";
import { Select } from "./Select";
import { CodeViewIcon, EyeIcon } from "./Icons";

/**
 * "orbit-dark": Monaco theme matching the app's palette (bg #0d0f13,
 * panels #14171d) so the editor blends with the rest of the UI instead of
 * the stock vs-dark #1e1e1e grey-blue.
 */
function defineOrbitTheme(monaco: typeof Monaco): void {
  monaco.editor.defineTheme("orbit-dark", {
    base: "vs-dark",
    inherit: true,
    rules: [
      { token: "comment", foreground: "6b7280", fontStyle: "italic" },
      { token: "keyword", foreground: "c792ea" },
      { token: "string", foreground: "a5d6a7" },
      { token: "number", foreground: "f78c6c" },
      { token: "type", foreground: "7aa7ff" },
      { token: "function", foreground: "82aaff" },
      { token: "variable", foreground: "e6e8ec" },
      { token: "delimiter", foreground: "9aa1ad" },
    ],
    colors: {
      "editor.background": "#0d0f13",
      "editor.foreground": "#e6e8ec",
      "editorLineNumber.foreground": "#4a5060",
      "editorLineNumber.activeForeground": "#9aa1ad",
      "editor.selectionBackground": "#1c2c52",
      "editor.lineHighlightBackground": "#14171d",
      "editorCursor.foreground": "#4f7cf7",
      "editorIndentGuide.background1": "#1a1e26",
      "editorIndentGuide.activeBackground1": "#2e3440",
      "editorWidget.background": "#15181e",
      "editorWidget.border": "#232833",
      "editorGutter.background": "#0d0f13",
      "scrollbarSlider.background": "#2b303a80",
      "scrollbarSlider.hoverBackground": "#3a404d",
      "scrollbarSlider.activeBackground": "#4a5060",
      "editorBracketMatch.background": "#1c2c52",
      "editorBracketMatch.border": "#4f7cf7",
    },
  });
}

const beforeMount: BeforeMount = (monaco) => {
  defineOrbitTheme(monaco);
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

// Define the theme as soon as the monaco loader resolves (module scope, runs
// once for the whole app) — the component prop "theme" then always finds it
// registered, regardless of mount order or HMR state.
loader
  .init()
  .then((monaco) => defineOrbitTheme(monaco as unknown as typeof Monaco))
  .catch(() => null);

interface Props {
  workspace: string;
  repo: string;
  path: string;
  onError: (msg: string) => void;
  /** Opens an agent terminal to execute this plan (PLAN.md only). */
  onApplyPlan?: (agent: string, model: string) => void;
}

/**
 * Single-file editor tab: Monaco + file bar (path, dirty dot, save).
 * Markdown files get a JetBrains-style Editor/Preview toggle in the bar.
 * PLAN.md (workspace root) additionally gets an Apply bar: pick the agent
 * + model and launch a terminal session that executes the plan.
 * The file tree lives in the fixed right dock (FileTreePanel).
 */
export function EditorPane({ workspace, repo, path, onError, onApplyPlan }: Props) {
  const [content, setContent] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [mode, setMode] = useState<"editor" | "preview">("editor");
  const [ai, setAi] = useState<{ agent: string; model: string } | null>(null);
  const [models, setModels] = useState<string[]>([]);
  const saveRef = useRef<(() => void) | null>(null);
  const editorRef = useRef<Monaco.editor.IStandaloneCodeEditor | null>(null);

  const applyReveal = () => {
    const key = revealKey({ workspace, repo, path });
    const t = pendingReveal.get(key);
    const ed = editorRef.current;
    if (!t || !ed) return;
    pendingReveal.delete(key);
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspace, repo, path]);

  const save = async () => {
    if (content === null) return;
    try {
      await invoke("write_file", { workspace, repo, path, content });
      setDirty(false);
    } catch (e) {
      onError(String(e));
    }
  };
  saveRef.current = save;

  const onMount: OnMount = (editor, monaco) => {
    // Belt & suspenders: ensure the theme is applied even if beforeMount
    // raced with the monaco loader.
    defineOrbitTheme(monaco);
    monaco.editor.setTheme("orbit-dark");
    editorRef.current = editor;
    applyReveal();
    editor.onDidChangeModelContent(() => {
      setContent(editor.getValue());
      setDirty(true);
    });
  };

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
          <button className="btn-mini" onClick={save} disabled={!dirty}>
            Save
          </button>
        </span>
      </div>
      {content !== null ? (
        mode === "preview" && isMarkdown ? (
          <MarkdownPreview content={content} />
        ) : (
          <div className="editor-host">
            <Editor
              height="100%"
              theme="orbit-dark"
              beforeMount={beforeMount}
              path={`${workspace}/${repo}/${path}`}
              value={content}
              onMount={onMount}
              options={{
                fontSize: 13,
                minimap: { enabled: false },
                scrollBeyondLastLine: false,
                automaticLayout: true,
              }}
              loading={<div className="table-loading"><span className="spinner" /> Loading…</div>}
            />
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