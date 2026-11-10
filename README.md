# Orbit

A desktop workspace for shipping features that span several repositories with AI coding agents.

A feature in Orbit is a **workspace**: one branch across every repo it touches, each checked out as a git worktree. From there Orbit runs the whole loop in one window: plan the work from a tracker card, let agents (Claude Code, OpenCode, OMP or Codex) implement it, review the diff, then commit, push, open PRs, watch CI and squash-merge, with AI drafting the boring parts.

Orbit is a native Rust app: the UI is built on [GPUI](https://www.gpui.rs/) and [gpui-component](https://github.com/longbridge/gpui-kit) (`native/`), over a backend crate with no UI dependency (`crates/orbit-core/`).

## Features

- **Multi-repo workspaces**: one branch, many repos, as git worktrees next to each other. Dependency folders (`node_modules`, build output) are shared with the base clone instead of reinstalled.
- **Agent sessions**: terminals running `claude`, `opencode`, `omp` or `codex` inside the workspace, with live status (thinking, editing, waiting for you).
- **Ralph**: write a PRD and let the agent implement it story by story in a loop.
- **Delivery pipeline**: commit (AI-drafted message), push, pull requests (AI-drafted title and description that follow the repo's PR template), CI checks with re-run, squash and merge, and rebase with AI conflict resolution. Merged work is detected even after a squash.
- **Code review**: review the workspace's changes with notes, and send them to the PR.
- **Editor and search**: code editor tabs with language servers (go to definition, hover, completion, references, rename…), file tree, git changes and history, Search Everywhere (double Shift) and Find in Files (Ctrl+Shift+F).
- **Build, run and debug**: each repo's build command, run configurations, and a debugger (DAP).

`native/README.md` has the keymap, the architecture and what is left to port from the earlier Tauri app.

## Install

Linux (x64): download the AppImage or the tarball from the [latest release](https://github.com/igorsegallafa/orbit/releases/latest). The AppImage runs as is (`chmod +x`); the tarball installs into `~/.local` with `./install.sh` (`./install.sh --uninstall` removes it).

Windows and macOS: the app compiles for both, but has not been run there yet.

### Requirements

Orbit drives these CLIs, which must be on your `PATH` (Settings → Health checks them):

- [Git](https://git-scm.com) 2.38 or newer
- [GitHub CLI](https://cli.github.com), logged in with `gh auth login`
- An agent CLI: [Claude Code](https://docs.anthropic.com/en/docs/claude-code), [OpenCode](https://opencode.ai), OMP or [Codex](https://github.com/openai/codex) (pick it in Settings → AI)
- `curl`, only for the Linear, Shortcut and Figma integrations

A Vulkan driver is used when present, OpenGL otherwise.

## Getting started

1. **Settings → Repositories**: add the repos you work on (or import them from GitHub). Orbit clones each one once and creates worktrees from that clone.
2. **Settings → AI**: choose the agent and model. A faster model can be set for commit messages and PR drafts.
3. **Integrations** (optional): connect Shortcut or Linear to create workspaces from cards.
4. **Dashboard → New workspace**: pick the repos and a branch name, then work through the workspace page: changes → push → pull requests → checks.

Orbit keeps its settings in `~/.config/orbit/config.yaml` (`~/Library/Application Support/orbit` on macOS). Clones and workspaces live in `~/Documents/orbit-workspace` unless moved in Settings.

## Development

You need [Rust](https://rustup.rs). On Linux, also the headers GPUI builds against (Debian/Ubuntu: `libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libfontconfig-dev libfreetype-dev libx11-xcb-dev libxcb1-dev libvulkan-dev`).

The repository is one Cargo workspace (`crates/orbit-core`, `native`) with one `Cargo.lock` and one `target/`:

```sh
cargo run --release -p orbit-native        # build and start the app
cargo build --release -p orbit-native      # target/release/orbit-native
native/scripts/package-linux.sh            # native/dist/: the tarball, and an AppImage when appimagetool is found
```

Before a pull request, run the checks (clippy on every crate and target, then every test):

```sh
cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

`native/vendor/gpui-pre-wgpu` is a patched copy of GPUI's renderer crate (Vulkan only, GL as a fallback), wired in by `[patch.crates-io]` in the root `Cargo.toml`; `native/vendor/PATCHES.md` says what changed. `--features capture` with `ORBIT_CAPTURE=shot.png` saves each frame as a PNG, and the app has a self-test mode that drives it without OS input (`native/README.md`, *Self-test mode*).

## License

[MIT](LICENSE)
