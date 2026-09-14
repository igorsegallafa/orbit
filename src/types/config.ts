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

export interface Workspace {
  name: string;
  branch: string;
  base: string;
  repos: string[];
}

export interface RepoStatus {
  repo: string;
  branch: string | null;
  dirty: boolean;
  ahead: number;
  behind: number;
}
