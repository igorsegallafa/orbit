/** One command for every platform, or one per platform (Node platform keys). */
export type BuildCmd = string | { win32?: string; linux?: string; darwin?: string };

export interface Service {
  name: string;
  repo: string;
  build?: BuildCmd;
  /** Build output dir shared with the base clone (default "dist"). */
  buildOutput?: string;
  /** Share only these sub-paths of the output dir. */
  buildOutputShared?: string[];
  /** false = branch-only: checked out in the base clone, no worktree. */
  worktree?: boolean;
  /** Custom clone location (existing checkout or chosen clone folder). */
  path?: string;
}

export interface Config {
  services: Service[];
  groups: Record<string, string[]>;
  /** Clones folder override (default <root>/repos). */
  reposDir?: string;
  /** Workspaces folder override (default <root>/workspaces). */
  workspacesDir?: string;
}

export const emptyConfig: Config = {
  services: [],
  groups: {},
};

export interface GithubRepo {
  name: string;
  nameWithOwner: string;
  sshUrl: string;
  isPrivate: boolean;
}

export interface CardRef {
  kind: "shortcut" | "linear" | string;
  id: string;
  title: string;
  url: string;
}

export interface CardDetail {
  id: string;
  title: string;
  description: string;
  state: string;
  url: string;
}

export interface Workspace {
  name: string;
  branch: string;
  base: string;
  repos: string[];
  card?: CardRef;
  /** PRs created from this workspace (persisted in .workspace.yaml). */
  prRefs?: { repo: string; number: number; url: string }[];
  /** Race variant: the workspace it competes for. */
  variant_of?: string;
  /** Agent racing in this variant ("claude · model"). */
  agent?: string;
}

export interface AiSettings {
  agent: string;
  model: string;
  /** Model for commit messages and PR drafts; unset = agent default. */
  fast_model?: string | null;
}

export interface GrillOption {
  label: string;
  description: string;
  recommended: boolean;
}

// ---------- Code Review (PRs) ----------

export interface PullRequest {
  repo: string; // Orbit service name
  ownerRepo: string; // "owner/repo" for gh - R
  number: number;
  title: string;
  branch: string;
  base: string;
  author: string;
  isDraft: boolean;
  url: string;
  updatedAt: string; // ISO
}

/** Workspace home PR tracker row (any state). */
export interface WsPrStatus {
  repo: string;
  number: number;
  title: string;
  url: string;
  author: string;
  state: "OPEN" | "MERGED" | "CLOSED" | string;
  isDraft: boolean;
  commits: number;
  updatedAt: string; // ISO
}

/** CI check of one PR (GitHub Actions or external status). */
export interface PrCheck {
  name: string;
  state: string;
  bucket: "pass" | "fail" | "pending" | "skipping" | string;
  workflow: string;
  link: string;
  startedAt: string;
  completedAt: string;
}

/** Checks of one PR + aggregate (pass | fail | running | none). */
export interface WsCheck {
  repo: string;
  prNumber: number;
  checks: PrCheck[];
  status: string;
}

/** AI analysis of a failed check. */
export interface CheckAnalysis {
  problem: string;
  fix: string;
  actionable: boolean;
}

export interface PrGroup {
  branch: string;
  prs: PullRequest[];
}

export interface PrFile {
  path: string;
  additions: number;
  deletions: number;
}

export interface PrDetail {
  title: string;
  author: string;
  url: string;
  branch: string;
  base: string;
  body: string;
  additions: number;
  deletions: number;
  files: PrFile[];
  headSha: string;
  baseSha: string;
}

export interface PrFileDiff {
  original: string;
  modified: string;
}

export interface GitChange {
  path: string;
  status: "M" | "A" | "D" | "U" | string;
  added: number;
  deleted: number;
}

export interface GitCommit {
  sha: string;
  message: string;
  author: string;
  when: string;
}

/** A repo's base clone as the repository view shows it (see repo.rs). */
export interface RepoOverview {
  name: string;
  path: string;
  cloned: boolean;
  /** null = detached HEAD. */
  branch: string | null;
  head: string;
  defaultBranch: string | null;
  /** Tracking branch ("origin/feat/x"); null = never pushed. */
  upstream: string | null;
  /** The upstream was deleted on the remote (e.g. its PR was merged). */
  upstreamGone: boolean;
  ahead: number;
  behind: number;
  changes: number;
  ownerRepo: string | null;
  /** A paused "rebase" | "merge" | "cherry-pick" | "revert". */
  operation: string | null;
  conflicts: string[];
  stashes: StashEntry[];
  /** Unix seconds of the last fetch. */
  lastFetch: number | null;
  branches: BranchInfo[];
  worktrees: WorktreeInfo[];
}

export interface BranchInfo {
  name: string;
  current: boolean;
  /** Only on origin: switching creates the local tracking branch. */
  remoteOnly: boolean;
  upstream: string | null;
  gone: boolean;
  ahead: number;
  behind: number;
  subject: string;
  updated: string;
  updatedUnix: number;
  /** Checked out in another worktree. */
  worktree: string | null;
}

export interface WorktreeInfo {
  path: string;
  branch: string | null;
  main: boolean;
  workspace: string | null;
  missing: boolean;
}

/** How the current branch and another differ (repo view's Compare tab). */
export interface Comparison {
  /** Commits only on the current branch. */
  ahead: GitCommit[];
  /** Commits only on the other branch (cherry-pick candidates). */
  behind: GitCommit[];
}

/** Sidebar status of a repo's clone. */
export interface RepoBrief {
  name: string;
  cloned: boolean;
  branch: string | null;
  changes: number;
  ahead: number;
  behind: number;
}

export interface StashEntry {
  index: number;
  message: string;
  when: string;
}

export interface GitFileDiff {
  original: string;
  modified: string;
}

export interface RepoStatus {
  repo: string;
  branch: string | null;
  dirty: boolean;
  ahead: number;
  behind: number;
  /** Commits on the remote branch that the base lacks — PR-worthy content. */
  prCommits: number;
  /** The branch's work is already in the base (e.g. squash-merged): `ahead` needs no push. */
  integrated: boolean;
}
