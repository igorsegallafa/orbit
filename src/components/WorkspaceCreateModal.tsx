import { useCallback, useEffect, useMemo, useState } from "react";
import { toast } from "./Toast";
import { invoke } from "@tauri-apps/api/core";
import { Config, Workspace } from "../types/config";
import { Skeleton } from "./Skeleton";
import { CheckBox } from "./CheckBox";
import { Select } from "./Select";
import { ShortcutLogo, LinearLogo } from "./BrandIcons";

interface CardInfo {
  id: string;
  title: string;
  state: string;
  url: string;
  /** Branch name the tracker suggests (Linear). */
  branch?: string;
}

type Source = "manual" | "branch" | "shortcut" | "linear";

interface Props {
  config: Config;
  /** Repos checked when the modal opens (e.g. from a repository view). */
  initialRepos?: string[];
  onCreated: () => void;
  onClose: () => void;
  onError: (msg: string) => void;
}

function slugify(s: string): string {
  return s
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function WorkspaceCreateModal({ config, initialRepos, onCreated, onClose, onError }: Props) {
  const [source, setSource] = useState<Source>("manual");
  const [name, setName] = useState("");
  const [branch, setBranch] = useState("");
  const [base, setBase] = useState("main");
  const [baseOptions, setBaseOptions] = useState<string[]>(["main", "master"]);
  const [selected, setSelected] = useState<Set<string>>(() => new Set(initialRepos ?? []));

  // Card picker (from-tracker mode)
  const [cardQuery, setCardQuery] = useState("");
  const [cards, setCards] = useState<CardInfo[] | null>(null);
  const [cardsLoading, setCardsLoading] = useState(false);
  const [pickedCard, setPickedCard] = useState<CardInfo | null>(null);
  // Which trackers are connected (disables their pills when not)
  const [connected, setConnected] = useState<Record<string, boolean>>({});

  const fromBranch = source === "branch";

  const effectiveBranch = useMemo(() => {
    if (branch.trim()) return branch.trim();
    if (pickedCard) return `feat/${pickedCard.id}`;
    const slug = slugify(name);
    return slug ? `feat/${slug}` : "";
  }, [branch, name, pickedCard]);

  // Load tracker connection status once on open
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const out: Record<string, boolean> = {};
      for (const kind of ["shortcut", "linear"]) {
        try {
          const st = await invoke<{ connected: boolean }>("integration_status", { kind });
          out[kind] = st.connected;
        } catch {
          out[kind] = false;
        }
      }
      if (!cancelled) setConnected(out);
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const fetchCards = useCallback(async () => {
    setCardsLoading(true);
    try {
      const result = await invoke<CardInfo[]>("integration_fetch_cards", {
        kind: source,
        query: cardQuery.trim() || null,
      });
      setCards(result);
    } catch (e) {
      setCards([]);
      onError(String(e));
    } finally {
      setCardsLoading(false);
    }
  }, [source, cardQuery, onError]);

  // Load cards when entering a tracker tab (debounced on query too)
  useEffect(() => {
    if (source === "manual" || source === "branch") return;
    const t = setTimeout(fetchCards, cardQuery ? 300 : 0);
    return () => clearTimeout(t);
  }, [source, cardQuery, fetchCards]);

  const pickCard = (card: CardInfo) => {
    setPickedCard(card);
    // Pre-fill the workspace name from the card title
    setName(slugify(card.title).slice(0, 40));
    setBranch(card.branch ?? `feat/${card.id}`);
  };

  // Base branch suggestions from the first selected (cloned) repo
  useEffect(() => {
    const repos = Array.from(selected);
    if (repos.length === 0) return;
    let cancelled = false;
    invoke<string[]>("list_base_branches", { repos })
      .then((branches) => {
        if (!cancelled && branches.length > 0) {
          setBaseOptions(branches);
          // keep current base if still valid, else snap to the first option
          setBase((b) => (branches.includes(b) ? b : branches[0]));
        }
      })
      .catch(() => null);
    return () => {
      cancelled = true;
    };
  }, [selected]);

  const toggleRepo = (repo: string) => {
    setSelected((s) => {
      const next = new Set(s);
      if (next.has(repo)) next.delete(repo);
      else next.add(repo);
      return next;
    });
  };

  const toggleGroup = (group: string) => {
    const members = config.groups[group] ?? [];
    const allIn = members.every((m) => selected.has(m));
    setSelected((s) => {
      const next = new Set(s);
      for (const m of members) {
        if (allIn) next.delete(m);
        else next.add(m);
      }
      return next;
    });
  };

  const createFromBranch = async () => {
    const wsName = name.trim() || slugify(branch.replace(/^feat\//, ""));
    if (!branch.trim() || !wsName) {
      onError("Enter the existing branch name.");
      return;
    }
    // Fetching and checking out can take a while: close now, report in a toast.
    const branchName = branch.trim();
    const t = toast.loading(`Creating workspace ${wsName}…`, { description: `Checking out ${branchName}` });
    onClose();
    try {
      const res = await invoke<{ workspace: Workspace; failures: string[] }>("create_workspace_from_branch", { name: wsName, branch: branchName });
      if (res.failures.length) {
        toast.update(t, "info", `Workspace ${wsName} created with problems`, { description: res.failures.join("\n") });
      } else {
        toast.update(t, "success", `Workspace ${wsName} created`, { description: `${res.workspace.repos.length} repo(s) on ${branchName}` });
      }
      onCreated();
    } catch (e) {
      toast.update(t, "error", `Couldn't create ${wsName}`, { description: String(e) });
    }
  };

  const create = async () => {
    if (fromBranch) return createFromBranch();
    if (!name.trim() || selected.size === 0 || !effectiveBranch) {
      onError("Name and at least one repository are required.");
      return;
    }
    const wsName = name.trim();
    const repos = Array.from(selected);
    const card = pickedCard ? { kind: source, id: pickedCard.id, title: pickedCard.title, url: pickedCard.url } : null;
    // Cloning/fetching/worktrees can take a while: close now, report in a toast.
    const t = toast.loading(`Creating workspace ${wsName}…`, { description: `${repos.length} repo(s) on ${effectiveBranch}` });
    onClose();
    try {
      await invoke<Workspace>("create_workspace", { name: wsName, branch: effectiveBranch, base: base.trim() || "main", repos, card });
      toast.update(t, "success", `Workspace ${wsName} created`, { description: `${repos.length} repo(s) on ${effectiveBranch}` });
      onCreated();
    } catch (e) {
      toast.update(t, "error", `Couldn't create ${wsName}`, { description: String(e) });
    }
  };

  const hasGroups = Object.keys(config.groups).length > 0;

  const sourceTabs: { id: Source; label: string; logo?: React.ReactNode }[] = [
    { id: "manual", label: "Manual" },
    { id: "branch", label: "Existing branch" },
    { id: "shortcut", label: "Shortcut", logo: <ShortcutLogo size={14} /> },
    { id: "linear", label: "Linear", logo: <LinearLogo size={14} /> },
  ];

  return (
    <div className="modal-overlay" onMouseDown={onClose}>
      <div className="modal modal-wizard" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h3>New workspace</h3>
        </div>

        <div className="wizard-body">
          {/* Source: manual entry vs picking a tracker card. Plain buttons with
              comfortable hit areas — no clipped segmented control.
              Tracker pills are disabled until connected in Integrations. */}
          <div className="wizard-source-pills">
            {sourceTabs.map((tab) => {
              const needsConn = tab.id === "shortcut" || tab.id === "linear";
              const disabled = needsConn && connected[tab.id] !== true;
              return (
                <button
                  type="button"
                  key={tab.id}
                  className={`wizard-source-pill ${source === tab.id ? "wizard-source-active" : ""}`}
                  disabled={disabled}
                  title={disabled ? `Connect ${tab.label} in Integrations first` : undefined}
                  onClick={() => {
                    if (source === tab.id) return;
                    setSource(tab.id);
                    setPickedCard(null);
                    setCardQuery("");
                    setCards(null);
                    // Drop the card pre-fill; keep user-typed text intact
                    if (pickedCard) {
                      setBranch("");
                    }
                  }}
                >
                  {tab.logo}
                  <span>{tab.label}</span>
                </button>
              );
            })}
          </div>

          {fromBranch && (
            <p className="wizard-picked">
              Sets up a branch that already exists on origin, in every cloned repository that has it.
            </p>
          )}

          {source !== "manual" && !fromBranch && (
            <div className="wizard-field">
              <label>Pick a card</label>
              <input
                className="wizard-card-search"
                value={cardQuery}
                placeholder={`Search ${source} cards…`}
                onChange={(e) => setCardQuery(e.target.value)}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                name="orbit-card-search"
              />
              <div className="wizard-card-list">
                {cardsLoading && (
                  <>
                    <div className="modal-row"><Skeleton w={44} h={12} /> <Skeleton w="70%" h={12} /></div>
                    <div className="modal-row"><Skeleton w={44} h={12} /> <Skeleton w="55%" h={12} /></div>
                    <div className="modal-row"><Skeleton w={44} h={12} /> <Skeleton w="62%" h={12} /></div>
                  </>
                )}
                {!cardsLoading && cards !== null && cards.length === 0 && (
                  <p className="empty">No cards found. Connect {source} in Integrations or try another search.</p>
                )}
                {!cardsLoading &&
                  cards?.map((c) => (
                    <button
                      type="button"
                      key={c.id}
                      className={`modal-row wizard-card-row ${pickedCard?.id === c.id ? "modal-row-selected" : ""}`}
                      onClick={() => pickCard(c)}
                      title={c.url || c.title}
                    >
                      <span className="tag tag-info">{c.id}</span>
                      <span className="wizard-card-title">{c.title}</span>
                      {c.state && <span className="tag tag-muted">{c.state}</span>}
                    </button>
                  ))}
              </div>
              {pickedCard && (
                <p className="wizard-picked">
                  Selected <strong>{pickedCard.id}</strong> — fields below pre-filled from the card.
                </p>
              )}
            </div>
          )}

<div className="wizard-field">
              <label>Workspace name</label>
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={fromBranch ? slugify(branch.replace(/^feat\//, "")) || "derived from the branch" : "checkout-improvements"}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                name="orbit-ws-name"
              />
            </div>

          <div className="wizard-row">
            <div className="wizard-field">
              <label>Branch</label>
              <input
                value={branch}
                onChange={(e) => setBranch(e.target.value)}
                placeholder={fromBranch ? "feat/existing-branch" : effectiveBranch || "feat/my-workspace"}
                autoComplete="off"
                autoCorrect="off"
                autoCapitalize="off"
                spellCheck={false}
                name="orbit-ws-branch"
              />
            </div>
            {!fromBranch && <div className="wizard-field">
              <label>Base branch</label>
              <Select
                value={base}
                options={baseOptions.map((b) => ({ value: b, label: b }))}
                onChange={setBase}
              />
            </div>}
          </div>

          {!fromBranch && <div className="wizard-field">
            <label>
              Repositories
              <span className="wizard-count">{selected.size} selected</span>
            </label>
            {hasGroups && (
              <div className="wizard-chips">
                {Object.keys(config.groups).map((g) => {
                  const members = config.groups[g] ?? [];
                  const allIn = members.length > 0 && members.every((m) => selected.has(m));
                  return (
                    <button
                      type="button"
                      key={g}
                      className={`chip ${allIn ? "chip-active" : ""}`}
                      onClick={() => toggleGroup(g)}
                    >
                      {g}
                    </button>
                  );
                })}
              </div>
            )}
            <div className="wizard-picker">
              {config.services.map((s) => {
                const checked = selected.has(s.name);
                return (
                  <label key={s.name} className={`modal-row modal-row-check ${checked ? "modal-row-selected" : ""}`}>
                    <CheckBox label={s.name} checked={checked} onChange={() => toggleRepo(s.name)} />
                    <span className="modal-row-name">{s.name}</span>
                  </label>
                );
              })}
              {config.services.length === 0 && (
                <p className="empty">No repositories configured. Add some in Settings first.</p>
              )}
            </div>
          </div>}
        </div>

        <div className="modal-footer">
          <button type="button" className="secondary" onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            disabled={fromBranch ? !branch.trim() : !name.trim() || selected.size === 0}
            onClick={create}
          >
            Create workspace
          </button>
        </div>
      </div>
    </div>
  );
}