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
  ownerRepo: string; // "owner/repo" for gh -R
  number: number;
  title: string;
  branch: string;
  base: string;
  author: string;
  isDraft: boolean;
  url: string;
  updatedAt: string; // ISO
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
}
