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
