import { useEffect, useState } from "react";
import { DiffEditor, type BeforeMount } from "@monaco-editor/react";
import { invoke } from "@tauri-apps/api/core";
import { GitChange, GitFileDiff } from "../types/config";
import { tooltip } from "./Tooltip";
import { useDiffNav } from "./useDiffNav";

interface Props {
  workspace: string;
  repo: string;
  /** File to review; required in working mode. */
  path: string;
  /** When set, reviews this commit's version of `path` (sha^ vs sha). */
  sha?: string;
  /** Commit message for the header (commit mode). */
  shaLabel?: string;
  onError: (msg: string) => void;
}

let themeDefined = false;

const beforeMount: BeforeMount = (monaco) => {
  if (themeDefined) return;
  themeDefined = true;
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
    ],
    colors: {
      "editor.background": "#0d0f13",
      "editor.foreground": "#e6e8ec",
      "editorLineNumber.foreground": "#4a5060",
      "editorLineNumber.activeForeground": "#9aa1ad",
      "diffEditor.insertedTextBackground": "#12281a",
      "diffEditor.removedTextBackground": "#2d1416",
      "diffEditor.insertedLineBackground": "#0e2016",
      "diffEditor.removedLineBackground": "#241012",
    },
  });
};

function langOf(path: string): string | undefined {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  if (ext === "ts" || ext === "tsx") return "typescript";
  if (ext === "js" || ext === "jsx") return "javascript";
  if (ext === "rs") return "rust";
  if (ext === "go") return "go";
  if (ext === "py") return "python";
  if (ext === "md") return "markdown";
  if (ext === "json" || ext === "yaml" || ext === "yml" || ext === "toml") return ext;
  return undefined;
}

/**
 * Diff review tab: Monaco DiffEditor over either the working tree (HEAD vs
 * file) or a commit (sha^ vs sha). Split/unified toggle like GitHub.
 */
export function ReviewPane({ workspace, repo, path, sha, shaLabel, onError }: Props) {
  const [diff, setDiff] = useState<GitFileDiff | null>(null);
  const [split, setSplit] = useState(false); // unified by default (GitHub-style)
  const nav = useDiffNav();

  useEffect(() => {
    setDiff(null);
    const cmd = sha ? "git_commit_diff" : "git_file_diff";
    const args = sha
      ? { workspace, repo, sha, path }
      : { workspace, repo, path };
    invoke<GitFileDiff>(cmd, args)
      .then(setDiff)
      .catch((e) => {
        setDiff({ original: "", modified: "" });
        onError(String(e));
      });
  }, [workspace, repo, path, sha, onError]);

  return (
    <div className="review-pane">
      <div className="editor-filebar">
        <span className="mono">
          {sha ? `${shaLabel ?? sha} · ` : ""}
          {repo}/{path || "…"}
        </span>
        <span className="editor-filebar-actions">
          {nav.count > 0 && (
            <span className="diff-nav">
              <button
                className="mode-btn"
                onMouseEnter={(e) => tooltip.show("Previous change (Shift+F7)", e)}
                onMouseLeave={() => tooltip.hide()}
                onClick={() => nav.jump(-1)}
              >
                ‹
              </button>
              <span className="diff-nav-count">
                {nav.index < 0 ? "–" : nav.index + 1}/{nav.count}
              </span>
              <button
                className="mode-btn"
                onMouseEnter={(e) => tooltip.show("Next change (F7)", e)}
                onMouseLeave={() => tooltip.hide()}
                onClick={() => nav.jump(1)}
              >
                ›
              </button>
            </span>
          )}
          <span className="mode-toggle">
            <button
              className={`mode-btn ${split ? "mode-active" : ""}`}
              onMouseEnter={(e) => tooltip.show("Split view — side by side", e)}
              onMouseLeave={() => tooltip.hide()}
              onClick={() => setSplit(true)}
            >
              ⇄
            </button>
            <button
              className={`mode-btn ${!split ? "mode-active" : ""}`}
              onMouseEnter={(e) => tooltip.show("Unified view — inline changes", e)}
              onMouseLeave={() => tooltip.hide()}
              onClick={() => setSplit(false)}
            >
              ↕
            </button>
          </span>
        </span>
      </div>
      {diff !== null ? (
        <div className="editor-host">
          <DiffEditor
            height="100%"
            theme="orbit-dark"
            beforeMount={beforeMount}
            onMount={nav.onMount}
            language={langOf(path)}
            original={diff.original}
            modified={diff.modified}
            options={{
              readOnly: true,
              renderSideBySide: split,
              fontSize: 12.5,
              minimap: { enabled: false },
              scrollBeyondLastLine: false,
              automaticLayout: true,
              renderOverviewRuler: true,
              overviewRulerLanes: 3,
              diffWordWrap: "off",
            } as never}
            loading={<div className="table-loading"><span className="spinner" /> Loading diff…</div>}
          />
        </div>
      ) : (
        <div className="table-loading">
          <span className="spinner" /> Loading diff…
        </div>
      )}
    </div>
  );
}

/** Files of a commit (used by the commit review file picker). */
export async function fetchCommitFiles(
  workspace: string,
  repo: string,
  sha: string
): Promise<GitChange[]> {
  return invoke<GitChange[]>("git_commit_files", { workspace, repo, sha });
}