import { describe, expect, it } from "vitest";
import { PrPlan, creatable, pill, summary, titleFromBranch } from "./prPlan";

const plan = (repo: string, patch: Partial<PrPlan> = {}): PrPlan => ({
  repo,
  branch: "feat/add-items-skills",
  base: "main",
  commits: 2,
  unpushed: 0,
  pushed: true,
  state: "ready",
  reason: "",
  pr: null,
  ...patch,
});

describe("prPlan", () => {
  it("derives the same titles as orbit-core", () => {
    expect(titleFromBranch("feat/add-items-skills")).toBe("Add items skills");
    expect(titleFromBranch("fix_login--flow")).toBe("Fix login flow");
    expect(titleFromBranch("")).toBe("");
  });

  it("only offers repos that can get a PR", () => {
    expect(creatable(plan("a"))).toBe(true);
    expect(creatable(plan("a", { state: "needsPush" }))).toBe(true);
    expect(creatable(plan("a", { state: "noCommits" }))).toBe(false);
    expect(creatable(plan("a", { state: "hasPr" }))).toBe(false);
    expect(pill(plan("a", { state: "hasPr", pr: { number: 7, url: "u", isDraft: false } })).label).toBe("PR #7 open");
    expect(pill(plan("a", { state: "needsPush", pushed: false })).label).toBe("Not pushed");
  });

  it("summarizes the checked repos", () => {
    const plans = [plan("a"), plan("b", { commits: 1, state: "needsPush" }), plan("c", { state: "noCommits" })];
    expect(summary(plans, new Set(["a", "b", "c"]), "main")).toBe(
      "2 PRs into main · from feat/add-items-skills · 3 commits · 1 to push first",
    );
    expect(summary(plans, new Set(["a"]), "main")).toBe("1 PR into main · from feat/add-items-skills · 2 commits");
    expect(summary(plans, new Set(), "main")).toBe("No repository selected");
  });
});
