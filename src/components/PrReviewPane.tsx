import { useEffect, useState } from "react";
import { DiffEditor, type BeforeMount } from "@monaco-editor/react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { PrDetail, PrFileDiff, PullRequest } from "../types/config";
import { Skeleton } from "./Skeleton";
import { tooltip } from "./Tooltip";

interface Props {
  /** All PRs of the feature group (1 for single-repo PRs). */
  prs: PullRequest[];
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
 * PR review tab, GitHub-page style: PR header + selector (for multi-repo
 * features) on the left over the changed files, diff on the right.
 * Contents come from the GitHub API (base vs head SHA), so no local
 * checkout is needed.
 */
export function PrReviewPane({ prs, onError }: Props) {
  const [activeIdx, setActiveIdx] = useState(0);
  const [detail, setDetail] = useState<PrDetail | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [diff, setDiff] = useState<PrFileDiff | null>(null);
  const [split, setSplit] = useState(false); // unified by default

  const pr = prs[activeIdx];

  // Load detail when the selected PR changes
  useEffect(() => {
    if (!pr) return;
    setDetail(null);
    setSelected(null);
    invoke<PrDetail>("pr_detail", { ownerRepo: pr.ownerRepo, number: pr.number })
      .then((d) => {
        setDetail(d);
        setSelected(d.files[0]?.path ?? null);
      })
      .catch((e) => onError(String(e)));
  }, [pr?.ownerRepo, pr?.number, onError]);

  // Load the diff when the selected file changes
  useEffect(() => {
    if (!pr || !detail || !selected) return;
    setDiff(null);
    invoke<PrFileDiff>("pr_file_diff", {
      ownerRepo: pr.ownerRepo,
      headSha: detail.headSha,
      baseSha: detail.baseSha,
      path: selected,
    })
      .then(setDiff)
      .catch((e) => {
        setDiff({ original: "", modified: "" });
        onError(String(e));
      });
  }, [pr?.ownerRepo, detail?.headSha, selected, onError]);

  if (!pr) return null;

  return (
    <div className="commit-review pr-review">
      <div className="pr-review-side">
        {/* Multi-repo feature: chips to switch between its PRs */}
        {prs.length > 1 && (
          <div className="pr-selector">
            {prs.map((p, i) => (
              <button
                key={p.ownerRepo}
                className={`chip ${i === activeIdx ? "chip-active" : ""}`}
                onClick={() => setActiveIdx(i)}
                onMouseEnter={(e) => tooltip.show(`${p.repo} #${p.number}`, e)}
                onMouseLeave={() => tooltip.hide()}
              >
                {p.repo}
              </button>
            ))}
          </div>
        )}

        {detail === null ? (
          <div className="pr-side-loading">
            <Skeleton w="80%" h={14} />
            <Skeleton w="60%" h={10} />
            <Skeleton w={40} h={11} />
            <Skeleton w="70%" h={11} />
            <Skeleton w="55%" h={11} />
          </div>
        ) : (
          <>
            <div className="pr-header">
              <a
                className="pr-header-link"
                href={pr.url}
                onClick={(e) => {
                  e.preventDefault();
                  openUrl(pr.url).catch(() => null);
                }}
                onMouseEnter={(e) => tooltip.show("Open on GitHub ↗", e)}
                onMouseLeave={() => tooltip.hide()}
              >
                {pr.repo} #{pr.number} ↗
              </a>
              <div className="pr-header-title" title={detail.title}>
                {detail.title}
              </div>
              <div className="pr-header-meta">
                <span className="mono">{detail.branch}</span> → {detail.base} · {detail.author}
              </div>
              <div className="pr-header-stats">
                <span className="git-stat-add">+{detail.additions}</span>
                <span className="git-stat-del">−{detail.deletions}</span>
                <span className="tag tag-muted">{detail.files.length} files</span>
                {pr.isDraft && <span className="tag tag-warn">draft</span>}
              </div>
            </div>

            <div className="pr-files">
              {detail.files.map((f) => (
                <button
                  key={f.path}
                  className={`tree-item git-change-row ${selected === f.path ? "tree-active" : ""}`}
                  title={f.path}
                  onClick={() => setSelected(f.path)}
                >
                  <span className="git-change-name">{f.path.split("/").pop()}</span>
                  <span className="git-change-stats">
                    {f.additions > 0 && <span className="git-stat-add">+{f.additions}</span>}
                    {f.deletions > 0 && <span className="git-stat-del">−{f.deletions}</span>}
                  </span>
                </button>
              ))}
            </div>
          </>
        )}
      </div>

      {selected !== null ? (
        <div className="pr-diff-col">
          <div className="editor-filebar">
            <span className="mono pr-diff-path">{selected}</span>
            <span className="editor-filebar-actions">
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
                language={langOf(selected)}
                original={diff.original}
                modified={diff.modified}
                options={{
                  readOnly: true,
                  renderSideBySide: split,
                  fontSize: 12.5,
                  minimap: { enabled: false },
                  scrollBeyondLastLine: false,
                  automaticLayout: true,
                  renderOverviewRuler: false,
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
      ) : (
        <div className="editor-empty">Pick a file to see its diff</div>
      )}
    </div>
  );
}