// The Create PRs modal's model: `ws_pr_plan` rows (orbit-core `prs::PrPlan`)
// and the pure helpers the modal derives from them.

export type PlanState = "ready" | "needsPush" | "noCommits" | "hasPr" | "blocked";

export interface PrPlan {
  repo: string;
  branch: string;
  base: string;
  commits: number;
  unpushed: number;
  pushed: boolean;
  state: PlanState;
  reason: string;
  pr: { number: number; url: string; isDraft: boolean } | null;
}

/** A readable PR title from a branch: `feat/add-items-skills` → "Add items skills". */
export function titleFromBranch(branch: string): string {
  const last = (branch.split("/").pop() ?? branch).replace(/[-_]/g, " ").split(/\s+/).filter(Boolean).join(" ");
  return last ? last[0].toUpperCase() + last.slice(1) : "";
}

/** A PR can be opened for it (pushing first when needed). */
export const creatable = (p: Pick<PrPlan, "state">) => p.state === "ready" || p.state === "needsPush";

/** The status pill's label and tone. */
export function pill(p: PrPlan): { label: string; tone: "ok" | "warn" | "info" | "muted" | "bad" } {
  switch (p.state) {
    case "ready":
      return { label: "Ready", tone: "ok" };
    case "needsPush":
      return { label: p.pushed ? "Push + PR" : "Not pushed", tone: "warn" };
    case "noCommits":
      return { label: "No commits", tone: "muted" };
    case "hasPr":
      return { label: `PR #${p.pr?.number ?? "?"} open`, tone: "info" };
    case "blocked":
      return { label: "Blocked", tone: "bad" };
  }
}

/** "3 PRs into main · from feat/x · 7 commits" for the checked repos. */
export function summary(plans: PrPlan[], checked: Set<string>, fallbackBase: string): string {
  const picked = plans.filter((p) => checked.has(p.repo) && creatable(p));
  const n = picked.length;
  if (n === 0) return "No repository selected";
  const bases = [...new Set(picked.map((p) => p.base))];
  const branches = [...new Set(picked.map((p) => p.branch))];
  const commits = picked.reduce((a, p) => a + p.commits, 0);
  const pushes = picked.filter((p) => p.state === "needsPush").length;
  const parts = [
    `${n} PR${n === 1 ? "" : "s"} into ${bases.length === 1 ? bases[0] : fallbackBase}`,
    `from ${branches.length === 1 ? branches[0] : `${branches.length} branches`}`,
    `${commits} commit${commits === 1 ? "" : "s"}`,
  ];
  if (pushes) parts.push(`${pushes} to push first`);
  return parts.join(" · ");
}
