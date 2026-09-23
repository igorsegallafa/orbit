# Orbit

A desktop workspace for shipping features that span several repositories with AI coding agents.

A feature in Orbit is a **workspace**: one branch across every repo it touches, each checked out as a git worktree. From there Orbit runs the whole loop in one window: plan the work from a tracker card, let agents (Claude Code, OpenCode or OMP) implement it, review the diff, then commit, push, open PRs, watch CI and squash-merge, with AI drafting the boring parts.

## Features

- **Multi-repo workspaces**: one branch, many repos, as git worktrees next to each other. Dependency folders (`node_modules`, build output) are shared with the base clone instead of reinstalled.
- **Agent sessions**: terminals running `claude`, `opencode` or `omp` inside the workspace, with live status (thinking, editing, waiting for you) and notifications when an agent finishes or needs attention.
- **Planning**: generate a `PLAN.md` from a Shortcut or Linear card, or get grilled by the agent first in an interview that surfaces the decisions you silently assumed.
- **Ralph**: write a PRD and let the agent implement it story by story in a loop.
- **Race**: run the same task with several agents in parallel variants and keep the best result.
- **Delivery pipeline**: commit (AI-drafted message), push, pull requests (AI-drafted title and description that follow the repo's PR template), CI checks with re-run and AI investigation of failures, squash and merge, and rebase with AI conflict resolution. Merged work is detected even after a squash.
- **Code review**: review GitHub PRs inside Orbit with inline comments, existing threads and pending reviews, and apply review feedback with an agent.
- **Editor and search**: Monaco editor tabs, file tree, git changes and history, Search Everywhere (double Shift) and Find in Files (Ctrl/Cmd+Shift+F).
- **Builds**: each repo's build command, skipped when nothing changed since the last build.
- **Auto-update**: new releases install from Settings → Updates or from the prompt Orbit shows when one is out.

## Install

Download the Windows installer (`Orbit_x.y.z_x64-setup.exe`) from the [latest release](https://github.com/igorsegallafa/orbit/releases/latest). Later versions install through the in-app updater.

On macOS and Linux, build from source (below).

### Requirements

Orbit drives these CLIs, which must be on your `PATH` (Settings → Health checks them):

- [Git](https://git-scm.com) 2.38 or newer
- [GitHub CLI](https://cli.github.com), logged in with `gh auth login`
- An agent CLI: [Claude Code](https://docs.anthropic.com/en/docs/claude-code), [OpenCode](https://opencode.ai) or OMP (pick it in Settings → AI)
- `curl`, only for the Linear, Shortcut and Figma integrations

## Getting started

1. **Settings → Repositories**: add the repos you work on (or import them from GitHub). Orbit clones each one once and creates worktrees from that clone.
2. **Settings → AI**: choose the agent and model. A faster model can be set for commit messages and PR drafts.
3. **Integrations** (optional): connect Shortcut or Linear to create workspaces from cards.
4. **Dashboard → New workspace**: pick the repos and a branch name, then work through the workspace page: changes → push → pull requests → checks.

Orbit keeps its settings in `~/.config/orbit/config.yaml` (`~/Library/Application Support/orbit` on macOS). Clones and workspaces live in `~/Documents/orbit-workspace` unless moved in Settings.

## Development

Orbit is a [Tauri 2](https://tauri.app) app: a Rust backend (`src-tauri/`) and a React + TypeScript frontend (`src/`). You need [Node.js](https://nodejs.org) and [Rust](https://rustup.rs), plus the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```sh
npm install
npm run tauri dev     # run the app with hot reload
npm run check         # typecheck, frontend tests, clippy and Rust tests
npm run tauri build   # installers in src-tauri/target/release/bundle
```

## Releasing

Publishing a GitHub release tagged `vX.Y.Z` builds the Windows installers and the updater manifest (`latest.json`) and attaches them to the release (`.github/workflows/release.yml`). The tag is the version; nothing needs bumping in the code.

Updates are signed: the workflow needs the `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` secrets, matching the public key in `src-tauri/tauri.conf.json`.
