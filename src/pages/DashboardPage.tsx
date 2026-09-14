import { Config } from "../types/config";

interface Props {
  config: Config;
}

export function DashboardPage({ config }: Props) {
  const repoCount = config.services.length;
  const groupCount = Object.keys(config.groups).length;

  return (
    <div className="page">
      <div className="page-header">
        <h2>Workspaces</h2>
      </div>

      {/* ponytail: static placeholder until workspaces (worktrees) land in Phase 2 */}
      <div className="empty-state">
        <div className="empty-state-icon">🛰️</div>
        <h3>No workspaces yet</h3>
        <p>
          A workspace is a set of git worktrees — one per repo — for working on a feature across
          multiple repositories in parallel. They'll show up here once you create one.
        </p>
      </div>

      <div className="stats-row">
        <div className="stat-card">
          <span className="stat-value">{repoCount}</span>
          <span className="stat-label">repositories</span>
        </div>
        <div className="stat-card">
          <span className="stat-value">{groupCount}</span>
          <span className="stat-label">groups</span>
        </div>
      </div>
    </div>
  );
}