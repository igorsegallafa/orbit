import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { toast } from "./Toast";
import { tooltip } from "./Tooltip";
import { ChevronRightIcon, DownloadIcon } from "./Icons";

export interface ArtifactSource {
  repo: string;
  ownerRepo: string;
  branch: string;
}

interface Artifact {
  id: number;
  name: string;
  sizeBytes: number;
  runId: number;
  workflow: string;
  createdAt: string;
  headSha: string;
}

interface Row extends Artifact {
  repo: string;
  ownerRepo: string;
}

/** Names that look like something to run (vs reports, coverage, logs). */
const BUILD_HINT = /(exe|win|windows|mac|macos|darwin|linux|app|dmg|msi|setup|installer|release|binary|bin|build|bundle|apk|ipa)/i;
const SHOWN = 6;

function size(bytes: number): string {
  if (bytes < 1024 * 1024) return `${Math.max(1, Math.round(bytes / 1024))} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

function age(iso: string): string {
  const s = Math.max(0, Date.now() - new Date(iso).getTime()) / 1000;
  if (s < 3600) return `${Math.max(1, Math.floor(s / 60))}m ago`;
  if (s < 86_400) return `${Math.floor(s / 3600)}h ago`;
  return `${Math.floor(s / 86_400)}d ago`;
}

const hint = (text: string) => ({
  onMouseEnter: (e: React.MouseEvent) => tooltip.show(text, e),
  onMouseLeave: () => tooltip.hide(),
});

/**
 * CI artifacts of a feature's branches (latest run of each workflow): what
 * the Actions already built, to try the feature without compiling it.
 * Download extracts into ~/Downloads/Orbit/<folder>/ and opens the folder.
 * Renders nothing when CI produced none.
 */
export function ArtifactsSection({ sources, folder, onError }: { sources: ArtifactSource[]; folder: string; onError: (msg: string) => void }) {
  const [rows, setRows] = useState<Row[] | null>(null);
  const [all, setAll] = useState(false);
  // Collapsed by default: CI builds are a side trip, not the workspace's focus.
  const openKey = `orbit.artifacts-open:${folder}`;
  const [open, setOpen] = useState(() => {
    try {
      return localStorage.getItem(openKey) === "1";
    } catch {
      return false;
    }
  });
  const toggle = () =>
    setOpen((o) => {
      try {
        localStorage.setItem(openKey, o ? "0" : "1");
      } catch {
        // just not remembered
      }
      return !o;
    });
  const [busy, setBusy] = useState<Record<number, "downloading" | "done">>({});
  const sourcesKey = sources.map((s) => `${s.ownerRepo}@${s.branch}`).join(",");

  useEffect(() => {
    let cancelled = false;
    setRows(null);
    Promise.all(
      sources.map((s) =>
        invoke<Artifact[]>("branch_artifacts", { ownerRepo: s.ownerRepo, branch: s.branch })
          .then((list) => list.map((a) => ({ ...a, repo: s.repo, ownerRepo: s.ownerRepo })))
          .catch(() => [] as Row[])
      )
    ).then((lists) => {
      if (cancelled) return;
      const flat = lists.flat();
      // Builds first, newest first within each group.
      flat.sort(
        (a, b) =>
          Number(BUILD_HINT.test(b.name)) - Number(BUILD_HINT.test(a.name)) ||
          new Date(b.createdAt).getTime() - new Date(a.createdAt).getTime()
      );
      setRows(flat);
    });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sourcesKey]);

  if (!rows || rows.length === 0) return null;

  const download = async (a: Row) => {
    setBusy((b) => ({ ...b, [a.id]: "downloading" }));
    try {
      const dir = await invoke<string>("artifact_download", { ownerRepo: a.ownerRepo, runId: a.runId, name: a.name, folder });
      setBusy((b) => ({ ...b, [a.id]: "done" }));
      toast.success(`Downloaded ${a.name}`, { description: dir });
    } catch (e) {
      setBusy(({ [a.id]: _drop, ...rest }) => rest);
      onError(String(e));
    }
  };

  const visible = all ? rows : rows.slice(0, SHOWN);
  const builds = rows.filter((r) => BUILD_HINT.test(r.name)).length;

  return (
    <section className="ws-section">
      <button type="button" className="btn-plain ws-section-head artifact-toggle" aria-expanded={open} onClick={toggle}>
        <h3>
          <ChevronRightIcon size={11} className={open ? "ws-chevron ws-chevron-open" : "ws-chevron"} />
          Artifacts <span className="section-count">{rows.length}</span>
        </h3>
        {!open && (
          <span className="artifact-summary">
            From CI{builds > 0 ? ` · ${builds} build${builds === 1 ? "" : "s"} to try without compiling` : ""}
          </span>
        )}
      </button>
      {open && (
        <>
      <div className="repo-list ws-repos">
        {visible.map((a) => {
          const state = busy[a.id];
          const likelyBuild = BUILD_HINT.test(a.name);
          return (
            <div key={`${a.ownerRepo}/${a.id}`} className="repo-row ws-repo-row artifact-row">
              <span className={`repo-icon ${likelyBuild ? "artifact-build" : ""}`}>
                <DownloadIcon size={15} />
              </span>
              <div className="repo-main">
                <span className="repo-name">{a.name}</span>
                <span className="repo-sub">
                  {a.repo} · {a.workflow} · {a.headSha.slice(0, 7)} · {age(a.createdAt)}
                </span>
              </div>
              <div className="ws-repo-badges">
                <span className="artifact-size">{size(a.sizeBytes)}</span>
                <button
                  className="btn-mini secondary"
                  disabled={state === "downloading"}
                  onClick={() => download(a)}
                  {...hint(`Download and extract into Downloads/Orbit/${folder}, then open the folder`)}
                >
                  {state === "downloading" ? (
                    <>
                      <span className="spinner" /> Downloading…
                    </>
                  ) : state === "done" ? (
                    "Download again"
                  ) : (
                    "Download"
                  )}
                </button>
              </div>
            </div>
          );
        })}
      </div>
      {rows.length > SHOWN && (
        <button className="btn-link artifact-more" onClick={() => setAll(!all)}>
          {all ? "Show fewer" : `Show all ${rows.length}`}
        </button>
      )}
        </>
      )}
    </section>
  );
}
