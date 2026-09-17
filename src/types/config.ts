export interface Service {
  name: string;
  repo: string;
}

export interface Config {
  services: Service[];
  groups: Record<string, string[]>;
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
}

export interface AiSettings {
  agent: string;
  model: string;
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
}
