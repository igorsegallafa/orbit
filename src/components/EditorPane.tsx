import { useEffect, useRef, useState } from "react";
import Editor, { BeforeMount, OnMount, loader } from "@monaco-editor/react";
import type * as Monaco from "monaco-editor";
import { invoke } from "@tauri-apps/api/core";
import { MarkdownPreview } from "./MarkdownPreview";

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
}

/**
 * Single-file editor tab: Monaco + file bar (path, dirty dot, save).
 * Markdown files get a JetBrains-style Editor/Preview toggle in the bar.
 * The file tree lives in the fixed right dock (FileTreePanel).
 */
export function EditorPane({ workspace, repo, path, onError }: Props) {
  const [content, setContent] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [mode, setMode] = useState<"editor" | "preview">("editor");
  const saveRef = useRef<(() => void) | null>(null);

  const isMarkdown = /\.(md|markdown)$/i.test(path);

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
    editor.onDidChangeModelContent(() => {
      setContent(editor.getValue());
      setDirty(true);
    });
  };

  return (
    <div className="editor-pane">
      <div className="editor-filebar">
        <span className="mono">
          {repo}/{path}
          {dirty ? " •" : ""}
        </span>
        <span className="editor-filebar-actions">
          {isMarkdown && (
            <span className="mode-toggle">
              <button
                className={`mode-btn ${mode === "editor" ? "mode-active" : ""}`}
                onClick={() => setMode("editor")}
              >
                Editor
              </button>
              <button
                className={`mode-btn ${mode === "preview" ? "mode-active" : ""}`}
                onClick={() => setMode("preview")}
              >
                Preview
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
        <div className="editor-empty">Pick a file in the Files panel</div>
      )}
    </div>
  );
}