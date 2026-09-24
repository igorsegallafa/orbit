import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Config, Service } from "../types/config";
import { CheckBox } from "./CheckBox";
import { PathInput } from "./PathInput";
import { Folders } from "./FoldersCard";
import { ChevronRightIcon } from "./Icons";

const emptyForm = {
  name: "",
  repo: "",
  build: "",
  perOs: false,
  win32: "",
  linux: "",
  darwin: "",
  buildOutput: "",
  shared: "",
  branchOnly: false,
  autoSwitch: false,
  path: "",
};
type Form = typeof emptyForm;

const OS_FIELDS = [
  ["win32", "Windows", "build.cmd"],
  ["linux", "Linux", "./build.sh"],
  ["darwin", "macOS", "./build.sh"],
] as const;

function toForm(s: Service): Form {
  const b = s.build;
  return {
    name: s.name,
    repo: s.repo,
    build: typeof b === "string" ? b : "",
    perOs: typeof b === "object",
    win32: typeof b === "object" ? b.win32 ?? "" : "",
    linux: typeof b === "object" ? b.linux ?? "" : "",
    darwin: typeof b === "object" ? b.darwin ?? "" : "",
    buildOutput: s.buildOutput ?? "",
    shared: (s.buildOutputShared ?? []).join(", "),
    branchOnly: s.worktree === false,
    autoSwitch: !!s.autoSwitch,
    path: s.path ?? "",
  };
}

function toService(f: Form): Service {
  const perOs = { win32: f.win32.trim(), linux: f.linux.trim(), darwin: f.darwin.trim() };
  const osEntries = Object.entries(perOs).filter(([, v]) => v);
  const build = f.perOs ? (osEntries.length ? Object.fromEntries(osEntries) : undefined) : f.build.trim() || undefined;
  const shared = f.shared.split(",").map((x) => x.trim()).filter(Boolean);
  return {
    name: f.name.trim(),
    repo: f.repo.trim(),
    build,
    buildOutput: f.buildOutput.trim() || undefined,
    buildOutputShared: shared.length ? shared : undefined,
    worktree: !f.branchOnly,
    autoSwitch: f.branchOnly && f.autoSwitch ? true : undefined,
    path: f.path.trim() || undefined,
  };
}

/** Picking the repo's own folder uses it as is (existing checkout); any
 *  other folder means "clone into <folder>/<name>", like GitHub Desktop. */
export function clonePathFor(folder: string, name: string): string {
  const base = folder.split(/[\\/]/).pop();
  if (!name || base === name) return folder;
  return `${folder}${folder.includes("\\") ? "\\" : "/"}${name}`;
}

export function defaultClonePath(folders: Folders | null, name: string): string {
  if (!folders) return "";
  const sep = folders.reposDir.includes("\\") ? "\\" : "/";
  return `${folders.reposDir}${sep}${name || "<name>"}`;
}

interface Props {
  /** Edit this repo; omit to add a new one. */
  service?: Service;
  cloned: boolean;
  folders: Folders | null;
  onSaved: (cfg: Config, info: { name: string; added: boolean; cloneNow: boolean }) => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

export function RepoFormModal({ service, cloned, folders, onSaved, onClose, onError }: Props) {
  const editing = !!service;
  const [form, setForm] = useState<Form>(service ? toForm(service) : emptyForm);
  const [showOptions, setShowOptions] = useState(!!service?.build || service?.worktree === false);
  const [cloneNow, setCloneNow] = useState(true);
  const [saving, setSaving] = useState(false);
  const hasBuild = form.perOs ? !!(form.win32 + form.linux + form.darwin).trim() : !!form.build.trim();
  const optionsSummary = [hasBuild && "build", form.branchOnly && "branch in base clone"].filter(Boolean).join(" · ");
  const pathLocked = editing && cloned;
  const canSave = !!form.name.trim() && !!form.repo.trim() && !saving;

  const submit = async () => {
    if (!canSave) return;
    setSaving(true);
    const args = { name: form.name.trim(), repo: form.repo.trim() };
    try {
      if (editing) await invoke<Config>("update_service", args);
      else await invoke<Config>("add_service", { ...args, path: form.path.trim() || null });
      const cfg = await invoke<Config>("update_service_settings", { service: toService(form) });
      onSaved(cfg, { name: args.name, added: !editing, cloneNow: !editing && cloneNow });
      onClose();
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onMouseDown={saving ? undefined : onClose}>
      <form
        className="modal modal-repo form"
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={(e) => {
          e.preventDefault();
          submit();
        }}
      >
        <div className="modal-header">
          <h3>{editing ? `Edit ${service!.name}` : "Add repository"}</h3>
        </div>
        <div className="modal-body repo-form-body">
          <div className="form-grid">
            <div className="field">
              <span className="field-label">Name</span>
              <input
                autoFocus={!editing}
                value={form.name}
                disabled={editing}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                placeholder="my-service"
              />
            </div>
            <div className="field">
              <span className="field-label">Repository URL</span>
              <input value={form.repo} onChange={(e) => setForm({ ...form, repo: e.target.value })} placeholder="https://github.com/org/repo.git" />
            </div>
          </div>

          <div className="field">
            <span className="field-label">Local path</span>
            <PathInput
              value={form.path}
              onChange={(path) => setForm({ ...form, path })}
              placeholder={defaultClonePath(folders, form.name.trim())}
              disabled={pathLocked}
              fromPicked={(folder) => clonePathFor(folder, form.name.trim())}
            />
            <span className="field-hint">
              {pathLocked
                ? "Already cloned here. Remove and re-add the repository to move it."
                : "Empty uses the clones folder. Pick an existing checkout to use it as is."}
            </span>
          </div>

          {editing && (
            <button type="button" className="disclosure" aria-expanded={showOptions} onClick={() => setShowOptions((v) => !v)}>
              <ChevronRightIcon size={14} className={showOptions ? "disclosure-chevron open" : "disclosure-chevron"} />
              Build &amp; checkout
              {!showOptions && optionsSummary && <span className="disclosure-summary">{optionsSummary}</span>}
            </button>
          )}

          {editing && showOptions && (
            <div className="form-options">
              <div className="field">
                <div className="field-head">
                  <span className="field-label">Build command</span>
                  <span className="mode-toggle seg">
                    <button type="button" className={`mode-btn ${!form.perOs ? "mode-active" : ""}`} onClick={() => setForm({ ...form, perOs: false })}>
                      All platforms
                    </button>
                    <button type="button" className={`mode-btn ${form.perOs ? "mode-active" : ""}`} onClick={() => setForm({ ...form, perOs: true })}>
                      Per platform
                    </button>
                  </span>
                </div>
                {form.perOs ? (
                  <div className="os-grid">
                    {OS_FIELDS.map(([k, label, ph]) => (
                      <div key={k} className="os-input">
                        <span className="os-tag">{label}</span>
                        <input value={form[k]} placeholder={ph} onChange={(e) => setForm({ ...form, [k]: e.target.value })} />
                      </div>
                    ))}
                  </div>
                ) : (
                  <input value={form.build} onChange={(e) => setForm({ ...form, build: e.target.value })} placeholder="e.g. npm install && npm run build" />
                )}
                <span className="field-hint">Runs in the repo folder with the system shell. Leave empty when the repo has no build.</span>
              </div>

              {hasBuild && (
                <div className="form-grid">
                  <div className="field">
                    <span className="field-label">Output folder</span>
                    <input value={form.buildOutput} onChange={(e) => setForm({ ...form, buildOutput: e.target.value })} placeholder="dist" />
                    <span className="field-hint">Shared with the base clone, so new workspaces start already built.</span>
                  </div>
                  <div className="field">
                    <span className="field-label">Share only these sub-folders</span>
                    <input value={form.shared} onChange={(e) => setForm({ ...form, shared: e.target.value })} placeholder="e.g. deps, cache" />
                    <span className="field-hint">Comma separated. Empty shares the whole output folder.</span>
                  </div>
                </div>
              )}

              <div className="field">
                <span className="field-label">Checkout</span>
                <div className="choice-row">
                  <div className={`choice ${!form.branchOnly ? "choice-on" : ""}`} onClick={() => setForm({ ...form, branchOnly: false })}>
                    <CheckBox label="Worktree per workspace" checked={!form.branchOnly} onChange={() => setForm({ ...form, branchOnly: false })} />
                    <span>
                      <strong>Worktree per workspace</strong>
                      <small>Each workspace gets its own checkout. Recommended.</small>
                    </span>
                  </div>
                  <div className={`choice ${form.branchOnly ? "choice-on" : ""}`} onClick={() => setForm({ ...form, branchOnly: true })}>
                    <CheckBox label="Branch in the base clone" checked={form.branchOnly} onChange={() => setForm({ ...form, branchOnly: true })} />
                    <span>
                      <strong>Branch in the base clone</strong>
                      <small>No second checkout, for repos too heavy to duplicate.</small>
                    </span>
                  </div>
                </div>
                {form.branchOnly && (
                  <>
                    <label className="check-item">
                      <CheckBox
                        label="Switch branches automatically"
                        checked={form.autoSwitch}
                        onChange={(v) => setForm({ ...form, autoSwitch: v })}
                      />
                      Switch branches automatically
                    </label>
                    <span className="field-hint">
                      Workspaces share this clone. Opening one puts the clone back on its branch without asking, unless there are
                      uncommitted changes. Off: Orbit only warns.
                    </span>
                  </>
                )}
              </div>
            </div>
          )}
        </div>
        <div className="modal-footer">
          {!editing && (
            <label className="check-item repo-clone-now">
              <CheckBox label="Clone now" checked={cloneNow} onChange={setCloneNow} />
              Clone now
            </label>
          )}
          <button type="button" className="secondary" onClick={onClose} disabled={saving}>
            Cancel
          </button>
          <button type="submit" disabled={!canSave}>
            {saving ? "Saving…" : editing ? "Save changes" : "Add repository"}
          </button>
        </div>
      </form>
    </div>
  );
}
