//! Stacked PRs: a PR whose base is another open PR's branch (same repo) sits on top of it.
//! Groups are features (one branch across repos), so a stack is a chain of groups.
use crate::github::{self, PrGroup, WsPrStatus};
use std::path::Path;

fn on_top(groups: &[PrGroup], upper: usize, lower: usize) -> bool {
    upper != lower && groups[upper].prs.iter().any(|c| groups[lower].prs.iter().any(|p| p.repo == c.repo && p.branch == c.base))
}

/// The group `g` is stacked on.
pub fn parent(groups: &[PrGroup], g: usize) -> Option<usize> {
    (0..groups.len()).find(|&p| on_top(groups, g, p))
}

/// The stack holding group `g`, bottom first; `[g]` when it stands alone.
/// shortcut: a branch with two PRs on top follows the first only; walk a tree if agents start forking stacks.
pub fn stack(groups: &[PrGroup], g: usize) -> Vec<usize> {
    let mut out = vec![g];
    while let Some(p) = parent(groups, out[0]).filter(|p| !out.contains(p)) {
        out.insert(0, p);
    }
    while let Some(c) = (0..groups.len()).find(|&c| on_top(groups, c, out[out.len() - 1]) && !out.contains(&c)) {
        out.push(c);
    }
    out
}

fn gh(args: &[&str]) -> Result<String, String> {
    crate::proc::run("gh", args, None)
}

/// Open PR numbers in `owner_repo` matching a `gh pr list` filter such as `--head x` or `--base x`.
pub fn open_prs(owner_repo: &str, filter: [&str; 2]) -> Result<Vec<u64>, String> {
    let out = gh(&["pr", "list", "-R", owner_repo, filter[0], filter[1], "--state", "open", "--json", "number", "--jq", ".[].number"])?;
    Ok(out.lines().filter_map(|l| l.trim().parse().ok()).collect())
}

/// Why merging a PR into `base` would land it in the wrong place: `base` is still another open PR's branch.
pub fn merge_guard(owner_repo: &str, base: &str) -> Result<(), String> {
    match open_prs(owner_repo, ["--head", base])?.first() {
        Some(n) => Err(format!("stacked on #{n}: merge #{n} first, or this lands in its branch {base} instead of the default branch")),
        None => Ok(()),
    }
}

/// Merges the base into PR `n` on GitHub (a merge commit, no history rewrite, no force-push).
pub fn update_branch(owner_repo: &str, n: u64) -> Result<(), String> {
    gh(&["api", "-X", "PUT", &format!("repos/{owner_repo}/pulls/{n}/update-branch")]).map(|_| ())
}

/// After `head` merged into `base`, moves its PRs onto `base` and updates them. Run it before `head` is deleted, which closes them.
pub fn restack(owner_repo: &str, head: &str, base: &str) -> Vec<String> {
    let children = match open_prs(owner_repo, ["--base", head]) {
        Ok(c) => c,
        Err(e) => return vec![format!("Couldn't list the PRs stacked on {head} ({e}): point them at {base} on GitHub")],
    };
    children
        .into_iter()
        .map(|n| {
            let s = n.to_string();
            if let Err(e) = gh(&["pr", "edit", &s, "-R", owner_repo, "--base", base]) {
                return format!("#{n}: couldn't retarget it to {base}: {e}");
            }
            match update_branch(owner_repo, n) {
                Ok(()) => format!("#{n} now targets {base} and is up to date"),
                Err(e) => format!("#{n} now targets {base} but couldn't be updated, likely a conflict: check it out to resolve ({e})"),
            }
        })
        .collect()
}

/// Commits on `parent` that `branch` lacks: the stacked PR needs an update.
pub fn behind(owner_repo: &str, branch: &str, parent: &str) -> Result<u64, String> {
    let out = gh(&["api", &format!("repos/{owner_repo}/compare/{branch}...{parent}"), "--jq", ".ahead_by"])?;
    out.trim().parse().map_err(|e| format!("bad compare answer: {e}"))
}

/// Blocks until PR `n`'s checks finish; Ok when they pass or there are none.
pub fn wait_checks(owner_repo: &str, n: u64) -> Result<(), String> {
    // The update just pushed: give GitHub time to register the new runs before watching.
    std::thread::sleep(std::time::Duration::from_secs(20));
    match gh(&["pr", "checks", &n.to_string(), "-R", owner_repo, "--watch", "--fail-fast", "--interval", "15"]) {
        Err(e) if !e.contains("no checks") => Err(format!("checks failed on #{n}")),
        _ => Ok(()),
    }
}

/// Grows a repo's PR rows to their whole stacks: up through PRs based on each branch, down through each base.
pub fn grow(owner_repo: &str, repo: &str, default_base: &str, seeds: &[String], rows: &mut Vec<WsPrStatus>) {
    let mine = |rows: &[WsPrStatus], b: &str| rows.iter().position(|r| r.repo == repo && r.branch == b);
    let mut todo: Vec<String> = seeds.iter().cloned().chain(rows.iter().filter(|r| r.repo == repo).map(|r| r.branch.clone())).collect();
    let mut seen = std::collections::HashSet::new();
    // shortcut: 20 branches bounds the gh calls of a runaway chain; raise it if stacks get that tall.
    while let Some(b) = todo.pop() {
        if b.is_empty() || b == default_base || !seen.insert(b.clone()) || seen.len() > 20 {
            continue;
        }
        if mine(rows, &b).is_none() {
            // The newest PR of the branch: its older closed ones are history.
            if let Some(mut r) = github::pr_status_list(owner_repo, "--head", &b, "all").ok().and_then(|v| v.into_iter().next()) {
                r.repo = repo.into();
                rows.push(r);
            }
        }
        if let Some(r) = mine(rows, &b).map(|i| &rows[i]) {
            todo.push(r.base.clone());
        }
        for mut c in github::pr_status_list(owner_repo, "--base", &b, "open").unwrap_or_default() {
            if mine(rows, &c.branch).is_none() {
                c.repo = repo.into();
                todo.push(c.branch.clone());
                rows.push(c);
            }
        }
    }
}

/// Commits `origin/<branch>` has that the local `branch` lacks; None when either is missing.
pub fn local_behind(dir: &Path, branch: &str) -> Option<usize> {
    let out = crate::proc::run("git", &["rev-list", "--count", &format!("{branch}..origin/{branch}")], Some(dir)).ok()?;
    out.trim().parse().ok()
}

/// Fast-forwards the local `branch` to origin: a pull when it's checked out, else a fetch into it.
pub fn sync_local(dir: &Path, branch: &str) -> Result<(), String> {
    let run = |args: &[&str]| crate::proc::run("git", args, Some(dir)).map(|_| ());
    if crate::git::current_branch(dir).as_deref() == Some(branch) {
        run(&["pull", "--ff-only", "origin", branch])
    } else {
        run(&["fetch", "origin", &format!("{branch}:{branch}")])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::PullRequest;

    fn group(repo: &str, branch: &str, base: &str) -> PrGroup {
        let pr = PullRequest { repo: repo.into(), branch: branch.into(), base: base.into(), ..Default::default() };
        PrGroup { branch: branch.into(), prs: vec![pr] }
    }

    #[test]
    fn chains_groups_by_base() {
        let g = vec![group("web", "x-3", "x-2"), group("web", "x-1", "main"), group("api", "y", "main"), group("web", "x-2", "x-1"), group("api", "z", "x-1")];
        assert_eq!(stack(&g, 0), vec![1, 3, 0]);
        assert_eq!(stack(&g, 1), vec![1, 3, 0]);
        assert_eq!(stack(&g, 2), vec![2]);
        // Same branch name in another repo isn't a parent.
        assert_eq!(stack(&g, 4), vec![4]);
        let cycle = vec![group("web", "a", "b"), group("web", "b", "a")];
        assert_eq!(stack(&cycle, 0).len(), 2);
    }
}
