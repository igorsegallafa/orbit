import { ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl as open } from "@tauri-apps/plugin-opener";
import { PullRequest } from "../types/config";
import { PrPlan, creatable, pill, summary, titleFromBranch } from "../lib/prPlan";
import { CheckBox } from "./CheckBox";
import { ConfirmDialog } from "./ConfirmDialog";
import { Skeleton } from "./Skeleton";
import { tooltip } from "./Tooltip";
import { PullRequestIcon, SparkIcon } from "./Icons";

type Phase = "idle" | "drafting" | "creating" | "created" | "failed";

interface Row {
  title: string;
  body: string;
  /** Typed in or drafted: closing would lose it. */
  touched: boolean;
  phase: Phase;
  startedAt?: number;
  error?: string;
  created?: { number: number; url: string };
}

interface Props {
  workspace: string;
  base: string;
  /** Repo names, for the list's placeholder while the plan loads. */
  repos: string[];
  /** The card's title, the default PR title when set; else one from the branch. */
  cardTitle?: string;
  onOpenPrs: (prs: PullRequest[]) => void;
  /** Some PRs were opened and the modal closed without opening the review. */
  onCreated?: () => void;
  onClose: () => void;
}

const openUrl = (url: string) => open(url).catch(() => null);

const isTextField = (el: EventTarget | null) =>
  el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement;

/** Create PRs: each repo's readiness (commits ahead, pushed, PR open) on
 *  the left, the selected repo's title and description on the right.
 *  PRs are drafted by the AI per repo or all at once, then created side by
 *  side with per-repo progress; failed ones can be retried. */
export function PrsModal({ workspace, base, repos, cardTitle, onOpenPrs, onCreated, onClose }: Props) {
  const [plans, setPlans] = useState<PrPlan[] | null>(null);
  const [planError, setPlanError] = useState<string | null>(null);
  const [rows, setRows] = useState<Record<string, Row>>({});
  const [checked, setChecked] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string | null>(null);
  const [asDraft, setAsDraft] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const [now, setNow] = useState(Date.now());
  const listRef = useRef<HTMLDivElement>(null);

  const patch = (repo: string, p: Partial<Row>) =>
    setRows((prev) => ({ ...prev, [repo]: { ...prev[repo], ...p } }));

  useEffect(() => {
    let cancelled = false;
    invoke<PrPlan[]>("ws_pr_plan", { workspace, base })
      .then((ps) => {
        if (cancelled) return;
        const init: Record<string, Row> = {};
        for (const p of ps) {
          init[p.repo] = { title: cardTitle || titleFromBranch(p.branch), body: "", touched: false, phase: "idle" };
        }
        setRows(init);
        setChecked(new Set(ps.filter(creatable).map((p) => p.repo)));
        setSelected((ps.find(creatable) ?? ps[0])?.repo ?? null);
        setPlans(ps);
      })
      .catch((e) => !cancelled && setPlanError(String(e)));
    return () => {
      cancelled = true;
    };
  }, [workspace, base, cardTitle]);

  const phases = Object.values(rows).map((r) => r.phase);
  const drafting = phases.filter((p) => p === "drafting").length;
  const creating = phases.includes("creating");
  const createdCount = phases.filter((p) => p === "created").length;

  // Elapsed seconds on drafting rows: AI drafts take 15–60 s.
  useEffect(() => {
    if (!drafting) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [drafting]);

  const toCreate = useMemo(
    () =>
      (plans ?? []).filter(
        (p) => creatable(p) && checked.has(p.repo) && !["created", "creating"].includes(rows[p.repo]?.phase),
      ),
    [plans, checked, rows],
  );
  const retrying = toCreate.length > 0 && toCreate.every((p) => rows[p.repo]?.phase === "failed");
  const waitingForAi = toCreate.some((p) => rows[p.repo]?.phase === "drafting");
  const missingTitle = toCreate.some((p) => !rows[p.repo]?.title.trim());
  const canCreate = toCreate.length > 0 && !creating && !waitingForAi && !missingTitle;

  const draft = (repo: string) => {
    patch(repo, { phase: "drafting", error: undefined, startedAt: Date.now() });
    invoke<{ title: string; body: string }>("ws_pr_draft", { workspace, repo })
      .then((d) => patch(repo, { phase: "idle", title: d.title, body: d.body, touched: true }))
      .catch((e) => patch(repo, { phase: "idle", error: `Draft failed: ${String(e)}` }));
  };

  // Rows the user wrote in keep their text: Draft all fills the others.
  const draftable = (plans ?? []).filter(
    (p) => creatable(p) && checked.has(p.repo) && rows[p.repo]?.phase === "idle" && !rows[p.repo]?.touched,
  );
  const draftAll = () => draftable.forEach((p) => draft(p.repo));

  const create = () => {
    if (!canCreate) return;
    const specs = toCreate.map((p) => ({ repo: p.repo, title: rows[p.repo].title.trim(), body: rows[p.repo].body, draft: asDraft }));
    for (const s of specs) patch(s.repo, { phase: "creating", error: undefined });
    for (const spec of specs) {
      invoke<{ number: number; url: string }>("ws_create_pr", { workspace, base, spec })
        .then((r) => {
          patch(spec.repo, { phase: "created", created: { number: r.number, url: r.url } });
          setChecked((prev) => {
            const next = new Set(prev);
            next.delete(spec.repo);
            return next;
          });
        })
        .catch((e) => {
          patch(spec.repo, { phase: "failed", error: String(e) });
          // Past the push step: the branch is on origin now.
          if (!String(e).includes("push failed"))
            setPlans((ps) => ps?.map((p) => (p.repo === spec.repo && p.state === "needsPush" ? { ...p, state: "ready", unpushed: 0, pushed: true, reason: "" } : p)) ?? ps);
        });
    }
  };

  const openReview = () => invoke<PullRequest[]>("ws_prs_flat", { workspace }).then(onOpenPrs).catch(() => onClose());

  const dirty = Object.values(rows).some((r) => r.touched && r.phase !== "created");
  const requestClose = () => {
    if (creating) return;
    if (dirty) return setConfirmClose(true);
    if (createdCount > 0) onCreated?.();
    onClose();
  };

  const move = (delta: number) => {
    if (!plans?.length) return;
    const i = Math.max(0, plans.findIndex((p) => p.repo === selected));
    const next = plans[Math.min(plans.length - 1, Math.max(0, i + delta))];
    setSelected(next.repo);
    listRef.current?.querySelector(`[data-repo="${CSS.escape(next.repo)}"]`)?.scrollIntoView({ block: "nearest" });
  };

  // Re-bound every render, so the handlers see the latest state.
  useEffect(() => {
    if (confirmClose) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        requestClose();
      } else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        create();
      } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && (e.altKey || !isTextField(e.target))) {
        e.preventDefault();
        move(e.key === "ArrowDown" ? 1 : -1);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const toggle = (repo: string, on: boolean) =>
    setChecked((prev) => {
      const next = new Set(prev);
      if (on) next.add(repo);
      else next.delete(repo);
      return next;
    });

  const sel = plans?.find((p) => p.repo === selected) ?? null;
  const selRow = sel ? rows[sel.repo] : undefined;
  const allDone = createdCount > 0 && toCreate.length === 0 && !creating;
  const anyOpen = (plans ?? []).some((p) => p.state === "hasPr");

  let primary: ReactNode;
  if (!plans) {
    primary = (
      <button type="button" disabled>
        <span className="spinner" /> Checking repositories…
      </button>
    );
  } else if (creating) {
    primary = (
      <button type="button" disabled>
        <span className="spinner" /> Creating…
      </button>
    );
  } else if (allDone || (toCreate.length === 0 && anyOpen)) {
    primary = (
      <button type="button" onClick={openReview}>
        Open in review
      </button>
    );
  } else {
    const n = toCreate.length;
    primary = (
      <button
        type="button"
        disabled={!canCreate}
        onClick={create}
        onMouseEnter={(e) =>
          waitingForAi ? tooltip.show("Waiting for the AI drafts", e) : missingTitle ? tooltip.show("Every PR needs a title", e) : undefined
        }
        onMouseLeave={() => tooltip.hide()}
      >
        {retrying ? `Retry ${n} failed` : `Create ${n} pull request${n === 1 ? "" : "s"}`}
      </button>
    );
  }

  return (
    <div className="modal-overlay" onMouseDown={requestClose}>
      <div className="modal prs-modal" onMouseDown={(e) => e.stopPropagation()} role="dialog" aria-label="Create pull requests">
        <div className="prs-head">
          <span className="prs-head-icon">
            <PullRequestIcon size={16} />
          </span>
          <div className="prs-head-text">
            <h3>Pull requests for this feature</h3>
            <div className="prs-summary">{plans ? summary(plans, checked, base) : "Checking each repository…"}</div>
          </div>
          <button
            type="button"
            className="secondary prs-draft-all"
            disabled={!plans || draftable.length === 0 || creating}
            onClick={draftAll}
            onMouseEnter={(e) =>
              tooltip.show("The AI reads each checked repo's PR template and diff and drafts its title and description. Repos you edited are kept.", e)
            }
            onMouseLeave={() => tooltip.hide()}
          >
            {drafting > 0 ? <span className="spinner" /> : <SparkIcon size={13} />}
            {drafting > 0 ? `Drafting ${drafting}…` : "Draft all with AI"}
          </button>
        </div>

        <div className="prs-panes">
          <div className="prs-list" ref={listRef} role="listbox" aria-label="Repositories">
            {!plans &&
              (repos.length ? repos : ["", "", ""]).map((r, i) => (
                <div key={i} className="prs-row prs-row-loading">
                  <span className="spinner" />
                  <div className="prs-row-main">
                    {r ? <span className="prs-row-name">{r}</span> : <Skeleton w="60%" h={12} />}
                    <Skeleton w="80%" h={9} />
                  </div>
                </div>
              ))}
            {plans?.length === 0 && <p className="prs-empty">No repositories in this workspace.</p>}
            {plans?.map((p) => {
              const row = rows[p.repo];
              const can = creatable(p) && !["created", "creating"].includes(row?.phase);
              return (
                <div
                  key={p.repo}
                  data-repo={p.repo}
                  role="option"
                  aria-selected={selected === p.repo}
                  className={`prs-row ${selected === p.repo ? "prs-row-selected" : ""} ${creatable(p) ? "" : "prs-row-off"}`}
                  onMouseDown={() => setSelected(p.repo)}
                >
                  <CheckBox
                    checked={checked.has(p.repo)}
                    disabled={!can || creating}
                    onChange={(c) => toggle(p.repo, c)}
                    label={`Create a PR for ${p.repo}`}
                  />
                  <div className="prs-row-main">
                    <span className="prs-row-name">{p.repo}</span>
                    <span className="prs-row-sub">
                      <span className="prs-row-branch">
                        {p.branch} → {p.base}
                      </span>
                      {p.commits > 0 && (
                        <span className="prs-row-commits">
                          · {p.commits} commit{p.commits === 1 ? "" : "s"}
                        </span>
                      )}
                    </span>
                  </div>
                  <RowStatus plan={p} row={row} />
                </div>
              );
            })}
            {planError && <div className="prs-error">{planError}</div>}
          </div>

          <div className="prs-editor">
            {!plans && !planError && (
              <div className="prs-editor-skeleton">
                <Skeleton w="40%" h={14} />
                <Skeleton w="100%" h={34} />
                <Skeleton w="100%" h={220} />
              </div>
            )}
            {sel && selRow && (
              <Editor
                plan={sel}
                row={selRow}
                now={now}
                disabled={creating}
                onChange={(p) => patch(sel.repo, { ...p, touched: true })}
                onDraft={() => draft(sel.repo)}
              />
            )}
          </div>
        </div>

        <div className="modal-footer prs-foot">
          <label className="prs-draft-toggle" onMouseEnter={(e) => tooltip.show("gh pr create --draft: reviewers aren't asked yet", e)} onMouseLeave={() => tooltip.hide()}>
            <CheckBox checked={asDraft} disabled={creating} onChange={setAsDraft} label="Open as draft" />
            <span>Open as draft</span>
          </label>
          <span className="prs-keys">
            <kbd>↑</kbd>
            <kbd>↓</kbd> repos · <kbd>Ctrl</kbd>+<kbd>Enter</kbd> create
          </span>
          <button type="button" className="secondary" onClick={requestClose} disabled={creating}>
            {allDone ? "Close" : "Cancel"}
          </button>
          {primary}
        </div>
      </div>

      {confirmClose && (
        <ConfirmDialog
          title="Discard PR drafts?"
          message="The titles and descriptions you wrote or drafted with AI will be lost."
          confirmLabel="Discard"
          danger
          onConfirm={() => {
            setConfirmClose(false);
            if (createdCount > 0) onCreated?.();
            onClose();
          }}
          onClose={() => setConfirmClose(false)}
        />
      )}
    </div>
  );
}

function RowStatus({ plan, row }: { plan: PrPlan; row?: Row }) {
  if (row?.phase === "drafting")
    return (
      <span className="prs-row-status">
        <span className="spinner" /> Drafting
      </span>
    );
  if (row?.phase === "creating")
    return (
      <span className="prs-row-status">
        <span className="spinner" /> Creating
      </span>
    );
  if (row?.phase === "created" && row.created)
    return (
      <button type="button" className="tag tag-ok prs-tag-link" onMouseDown={(e) => e.stopPropagation()} onClick={() => openUrl(row.created!.url)}>
        #{row.created.number} ↗
      </button>
    );
  if (row?.phase === "failed") return <span className="tag tag-bad">Failed</span>;
  const { label, tone } = pill(plan);
  if (plan.state === "hasPr" && plan.pr)
    return (
      <button type="button" className={`tag tag-${tone} prs-tag-link`} onMouseDown={(e) => e.stopPropagation()} onClick={() => openUrl(plan.pr!.url)}>
        {label} ↗
      </button>
    );
  return (
    <span
      className={`tag tag-${tone}`}
      onMouseEnter={(e) => plan.reason && tooltip.show(plan.reason, e)}
      onMouseLeave={() => tooltip.hide()}
    >
      {label}
    </span>
  );
}

interface EditorProps {
  plan: PrPlan;
  row: Row;
  now: number;
  disabled: boolean;
  onChange: (p: Partial<Row>) => void;
  onDraft: () => void;
}

function Editor({ plan, row, now, disabled, onChange, onDraft }: EditorProps) {
  const head = (
    <div className="prs-editor-head">
      <span className="prs-editor-repo">{plan.repo}</span>
      <span className="prs-editor-branch">
        {plan.branch} → {plan.base}
      </span>
      {plan.commits > 0 && (
        <span className="prs-editor-branch">
          · {plan.commits} commit{plan.commits === 1 ? "" : "s"}
        </span>
      )}
    </div>
  );

  if (row.phase === "created" && row.created) {
    const url = row.created.url;
    return (
      <>
        {head}
        <div className="prs-notice prs-notice-ok">
          <strong>Opened PR #{row.created.number}</strong>
          <span>{row.title}</span>
          <button type="button" className="link" onClick={() => openUrl(url)}>
            Open on GitHub ↗
          </button>
        </div>
      </>
    );
  }

  if (!creatable(plan)) {
    return (
      <>
        {head}
        <div className={`prs-notice ${plan.state === "hasPr" ? "prs-notice-info" : ""}`}>
          <strong>
            {plan.state === "hasPr" ? "Already has a pull request" : plan.state === "noCommits" ? "Nothing to open a PR for" : "Can't open a PR"}
          </strong>
          <span>{plan.reason}</span>
          {plan.pr && (
            <button type="button" className="link" onClick={() => openUrl(plan.pr!.url)}>
              Open PR #{plan.pr.number}
              {plan.pr.isDraft ? " (draft)" : ""} ↗
            </button>
          )}
        </div>
      </>
    );
  }

  const busy = disabled || row.phase === "drafting" || row.phase === "creating";
  const elapsed = row.startedAt ? Math.floor((now - row.startedAt) / 1000) : 0;
  return (
    <>
      {head}
      {plan.state === "needsPush" && <div className="prs-hint prs-hint-warn">{plan.reason}.</div>}
      {row.error && <div className="prs-error">{row.error}</div>}
      <label className="prs-label" htmlFor="prs-title">
        Title
      </label>
      <input
        id="prs-title"
        className="prs-title"
        placeholder="PR title"
        value={row.title}
        disabled={busy}
        onChange={(e) => onChange({ title: e.target.value })}
      />
      <div className="prs-label-row">
        <label className="prs-label" htmlFor="prs-body">
          Description <span className="prs-label-hint">Markdown</span>
        </label>
        <button
          type="button"
          className="btn-mini secondary"
          disabled={busy}
          onClick={onDraft}
          onMouseEnter={(e) => tooltip.show("The AI reads the repo's PR template and the diff, and drafts the title and description", e)}
          onMouseLeave={() => tooltip.hide()}
        >
          <SparkIcon size={11} /> {row.touched ? "Redraft with AI" : "Draft with AI"}
        </button>
      </div>
      <div className="prs-body-wrap">
        <textarea
          id="prs-body"
          className="prs-body"
          placeholder="What changed, why and how — short and objective."
          value={row.body}
          disabled={busy}
          onChange={(e) => onChange({ body: e.target.value })}
        />
        {row.phase === "drafting" && (
          <div className="prs-drafting">
            <span className="spinner" />
            <span>AI is reading the template and the diff… {elapsed}s</span>
            <span className="prs-drafting-hint">You can edit the other repos meanwhile.</span>
          </div>
        )}
      </div>
    </>
  );
}
