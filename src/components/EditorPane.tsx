import { useEffect, useRef, useState } from "react";
import Editor, { OnMount } from "@monaco-editor/react";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  workspace: string;
  repo: string;
  path: string;
  onError: (msg: string) => void;
}

/**
 * Single-file editor tab: Monaco + file bar (path, dirty dot, save).
 * The file tree lives in the fixed right dock (FileTreePanel).
 */
export function EditorPane({ workspace, repo, path, onError }: Props) {
  const [content, setContent] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const saveRef = useRef<(() => void) | null>(null);

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

  const onMount: OnMount = (editor) => {
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
        <button className="btn-mini" onClick={save} disabled={!dirty}>
          Save
        </button>
      </div>
      {content !== null ? (
        <div className="editor-host">
          <Editor
            height="100%"
            theme="vs-dark"
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
      ) : (
        <div className="editor-empty">Pick a file in the Files panel</div>
      )}
    </div>
  );
}