# Orbit native (GPUI)

Orbit's UI rewritten as a native Rust app on [GPUI](https://www.gpui.rs/) (Zed's UI framework) and
[gpui-component](https://github.com/longbridge/gpui-kit), to replace the Tauri + WebKitGTK + React
frontend, which costs 340–595 MB idle (mostly `WebKitWebProcess`).

Stage 1 built the shell and proved the riskiest part first: a code editor with a language server,
which every later stage depends on. Stage 2 moved Orbit's backend into the shared crate
`crates/orbit-core` (see *Sharing the backend*), which both apps now link. The Tauri app in the repo
root remains the product.

## Build and run

```sh
cd native
cargo build --release      # 824 crates; incremental release builds take 40 s–1.5 min (thin LTO)
./target/release/orbit-native
```

The app lists workspaces and starts language servers through `orbit-core`, the same code as the
Tauri app, so it shows the same workspaces and honours Settings → Languages (server overrides,
disabled languages). `target/` is ~3.3 GB after a
release build plus clippy and tests.

The first start after a build takes several seconds while the GPU driver compiles and caches
pipelines. Warm starts reach the first frame in 180–190 ms.

Keys: `Ctrl+Shift+T` new terminal · `Ctrl+W` close file tab (refused when unsaved) ·
`Ctrl+Shift+W` close any tab (discards) · `Ctrl+Tab` next tab · `Ctrl+Q` quit · in the editor
`Ctrl+S` save, `F12` or `Ctrl+click` go to definition, hover for docs · in a terminal
`Ctrl+Shift+C`/`Ctrl+Shift+V` copy/paste, drag to select (double click: word, triple: line;
`Shift` overrides an app's mouse mode), middle click pastes the primary selection.

### Self-test mode

There is no GUI automation (on GNOME Wayland, input simulation and screenshots trigger a
permission prompt). The app drives itself instead: keys and mouse events go through GPUI's own
dispatch (`Window::dispatch_keystroke` / `dispatch_event`), and the result is a state dump.

```sh
./target/release/orbit-native --workspace add-items-skills \
    --expand 3                          # expand 3 tree levels (loaded in the background)
    --terminals 3 --cmd top             # open terminals running a command
    --open aether-engine/src/game/Game.h --wait-lsp 30 --cursor 39:12   # open a file (repeatable; --preview opens as preview tabs), wait for clangd, place the cursor
    --type "text\n" --keys "f12 ctrl-s" # typed text, then key specs ("down*5" repeats)
    --ctrl-click --hover --diff         # Ctrl+click the word at the cursor, ask for hover, open the file's diff
    --term-drag "0,0:5,1"               # drag-select terminal cells (col,row)
    --exit-after 40 --dump out.txt      # quit after 40 s and write the dump (tabs, editor, LSP, diagnostics, terminal grid…)
scripts/measure.sh 8 [args…]            # PSS (+SwapPss) of the process tree after 8 s; language servers apart
```

In self-test mode, copies are recorded in the dump and never written to the system clipboard.

## Stack and why

| Piece | Choice |
|---|---|
| UI framework | `gpui-pre =0.3.11`: a crates.io snapshot of Zed's GPUI (zed@f816e1f), plus `gpui-pre-platform` for the native backend (Wayland/X11, wgpu renderer) |
| Components | `gpui-component =0.9.0` (Apache-2.0): code editor, diff view, theme and widgets |
| Editor | gpui-component's `EditorState`: rope buffer, tree-sitter highlighting (C, C++, Rust, TS/TSX, JS here), undo, search, folding, and LSP hooks (`DefinitionProvider`, `HoverProvider`, completion, code actions, semantic tokens, document colors, `show_document`) |
| Diff | gpui-component's `Diff`: virtualized unified/split patch view with app-supplied syntax colors |
| Terminal | `alacritty_terminal 0.26` (VT parsing, grid, selection) + `portable-pty`, painted on a GPUI canvas (own code, ported from the PoC) |
| LSP | our own small JSON-RPC client (`src/lsp`), with types from `lsp-types =0.97` (the version gpui-component's traits use) |

**Why gpui-component and not crates.io `gpui 0.2.2` (the PoC's).** The editor is the gate, and
gpui-component already has an LSP-ready code editor and a diff view; writing those from scratch on
gpui 0.2.2 would be months. gpui-component 0.9 requires the `gpui-pre` snapshot rather than gpui
0.2.2, and both are on crates.io, so no Zed checkout or git dependency is needed. It built on this
machine (CachyOS, GNOME Wayland, Intel + NVIDIA, Vulkan) on the first try and needs no `libc` pin.
One thing was not obvious: `gpui-pre` ships without a platform, so `gpui-pre-platform` (with the
`wayland` and `x11` features) is a separate dependency.

**The cost: memory.** `gpui-pre` renders through wgpu, where gpui 0.2.2 used blade. The wgpu
instance asks for the Vulkan *and* GL backends and enumerates every adapter. That maps both
vendors' Vulkan drivers, NVIDIA's EGL stack and Mesa's (`libgallium` + `libLLVM`), and opens
`/dev/nvidia0` even though the window renders on the Intel GPU. See the table below. Anonymous
(heap) memory is only ~37 MB; the rest is shared driver libraries and the 53 MB binary.

Licences: gpui-pre, gpui-component, alacritty_terminal: Apache-2.0; portable-pty: MIT. No code was
taken from Zed's GPL crates (`editor`, `terminal`, `terminal_view`).

## Measurements

i7-12650H, Intel iGPU + RTX 4060, GNOME Wayland, 1400×900 window. PSS + SwapPss summed over the
process tree (`scripts/memory.sh`'s method), max of 3 runs. The machine was under memory pressure
(13 GB in swap), and evicted file pages only lower a reading. "Restricted" runs with
`VK_DRIVER_FILES=/usr/share/vulkan/icd.d/intel_icd.json
__EGL_VENDOR_LIBRARY_FILENAMES=/usr/share/glvnd/egl_vendor.d/50_mesa.json` (Intel Vulkan and Mesa
EGL only), which a launcher can set.

| Scenario | Default | Restricted | PoC (gpui 0.2.2) | Tauri Orbit |
|---|---:|---:|---:|---:|
| Idle | 160 | 92 | 46 (24 Intel only) | 340–595 |
| Workspace tree, 3 levels (1,434 rows) | 168 | 99 | 49 | |
| Tree, 4 levels (15,710 rows) | 171 | 105 | 52 | |
| + 3 terminals running `top` (app + shells) | 179 | 112 | 76 | |
| Editor on `Game.cpp` with clangd attached (app only) | 156 | 88 | – | |
| clangd itself (same flags as the Tauri app) | 1,270–4,390 (30 s in, still indexing) | | | ~1.1 GB |

MB. clangd's figure depends on how far `--background-index` has got (1.3–4.4 GB in the first 30 s
while indexing aether-engine). It is the same in any editor, so it is kept out of the app columns.

Vulkan-only experiment (not committed): with `gpui-pre-wgpu` patched to request
`Backends::VULKAN` alone, idle was 110 MB by default and 54 MB with the Intel ICD only. Shipping
that means a `[patch.crates-io]` with a vendored copy of `gpui-pre-wgpu` (two lines changed in
`wgpu_context.rs`), or getting the backend set made configurable upstream. Worth doing before
release; the decision is the owner's.

Other numbers:

| Metric | Value |
|---|---|
| Start to first frame | 180–190 ms warm; ~9 s on the very first run after a build (driver pipeline cache) |
| Binary (release, stripped, thin LTO) | 53 MB |
| clangd start → `initialize` done | ~110 ms; the editor attaches 150 ms after opening |
| Tree, background loads | 3 levels 44 ms, 4 levels 81 ms, 5 levels (37,347 rows) 5.6 s without blocking the UI |
| Editor open (`Game.cpp`) | < 1 ms to read; highlighting is incremental tree-sitter |

## Verified in stage 1

Every item was checked by a self-test run and its dump.

- **Go to definition, across files (the gate).** In workspace `add-items-skills`, `aether-engine`
  is a symlink to `~/Desktop/ex-machina/aether-engine`. Opening `src/game/Game.h` starts clangd in
  the *real* folder with
  `--compile-commands-dir=…/build/linux-gcc-relwithdebinfo` (found by the same
  `find_compile_commands` rules as the Tauri app). With the cursor on `Initialize` at 39:12:
  - `F12` opens `aether-engine/Game.cpp` as a new tab at **44:10**, `void Initialize()`. The path
    is the workspace path, because URIs are mapped back from the real root.
  - **Ctrl+click** gives the same result, through GPUI mouse events: a Ctrl-hover asks for the
    definition, then the click follows it.
- **Hover.** clangd's markdown for `Initialize` ("function `Initialize` → `void` · Initialize Game
  Handler · `// In namespace Game`").
- **Diagnostics.** `Game.cpp` gets clangd's two "Included header … is not used directly" warnings,
  shown as editor diagnostics and counted in the path bar.
- **Editing, dirty state, save.** Typing marks the tab dirty (`●` instead of `×`, and a preview tab
  stays open). `Ctrl+S` writes the file (checked with `git diff` in a scratch repo) and clears the
  flag. didOpen, didChange (full text) and didSave keep the server in sync.
- **Diff.** The editor's "Diff" button (`--diff`) opens `git diff HEAD` of the file in
  gpui-component's diff view: split mode, `+1 −0`, toggle to unified, tree-sitter colors.
- **Terminal.** A drag over `hello world` / `second line` selects `"hello world\nsecond"`.
  Finished selections go to the primary selection; `Ctrl+Shift+C` copies.
  With an app in SGR mouse mode (`\e[?1000h\e[?1006h`), a click arrives as `\e[<0;3;4M` /
  `\e[<0;3;4m`. Wheel and motion are reported too.
- **Tree.** Folders load on the background executor. Rows show `…` while loading, and loads for an
  old workspace are dropped.

Not verified visually: no screenshot was possible (see *Self-test mode*). The hover popover, the
diagnostic underlines and the diff's look were exercised only through state. GPUI has a headless
wgpu renderer (`gpui-pre-platform`, `test-support` feature: `current_headless_renderer`) that could
render frames to images for visual checks in a later stage.

## Gate verdict

**Pass.** gpui-component's editor plus a ~400-line LSP client gives definition (F12, Ctrl+click,
across files, through the symlink mapping), hover and diagnostics against clangd on aether-engine.
The diff view is usable as a base for review: it parses patches, has split/unified modes,
virtualization, selection, gutter markers, decorations, conflict resolution and a syntax hook.

Still missing for the full review UI (the component has hooks for some of these): inline comment
threads anchored to lines (`DiffDecoration` / gutter markers are the hooks), editing in the diff,
and a file list for multi-file reviews (the state already takes many files).

Caveats found:

- Only clangd is installed here, so rust-analyzer and typescript-language-server are wired the same
  way but untested.
- Positions: we ask servers for UTF-32 (clangd agrees). A server that only speaks UTF-16 would be
  off by one column per astral-plane character on that line.
- Hover in the self-test calls the provider directly. The popover is the component's own and was
  not seen.

## Code map

| File | What |
|---|---|
| `src/main.rs` | app start, window, keymap, LSP shutdown on quit |
| `src/app.rs` | root view: sidebar, tree panel, tab strip (sessions, then files; preview tab; dirty dot), status bar, tab logic |
| `src/editor.rs` | editor tab: `EditorState`, save, dirty, LSP document sync and providers, cross-file definition |
| `src/lsp/` | `mod.rs` file → language and the server registry · `server.rs` JSON-RPC client and URI mapping over `orbit_core::lsp` (which picks, starts and feeds the process) · `paths.rs` URIs and root rebase |
| `src/diff_view.rs` | diff tab over `git diff HEAD` |
| `src/terminal/` | PTY + VT (`mod.rs`), grid layout and paint (`grid.rs`), keys and mouse reports (`input.rs`) |
| `src/file_tree.rs` | virtualized tree with background folder loads |
| `src/workspace.rs` | workspace list from `orbit_core::workspace` |
| `src/theme.rs`, `src/orbit-dark.json` | Orbit Dark tokens (`src/App.css`, `src/lib/theme.ts`) for our views and for gpui-component |
| `src/script.rs` | self-test mode and dumps |

## Sharing the backend: `orbit-core`

`crates/orbit-core` holds Orbit's backend logic with no Tauri dependency. `src-tauri` keeps the
`#[tauri::command]` wrappers (thin: argument plumbing, `spawn_blocking`, events to the webview) and
calls into it; the native app calls it directly.

**Layout.** `orbit-core` is a member of `src-tauri`'s Cargo workspace (`[workspace]` in
`src-tauri/Cargo.toml`, `package.workspace` in the core crate), so the Tauri app keeps its
`Cargo.lock`, its `src-tauri/target` and the CI cache path unchanged. `pnpm run check` runs clippy
and tests with `--workspace`, which covers the core crate. `native/` stays a separate workspace with
its own lock file and target dir and takes `orbit-core` as a path dependency: GPUI's dependency tree
never meets Tauri's, so neither can force upgrades on the other. Adding the core crate to `native/`
only added `trash`, `urlencoding` and `winreg` to its lock; no existing version moved. The cost is
that `orbit-core` compiles twice, once per target dir (a few seconds).

**Events.** Long-running work reports through `orbit_core::events`:

```rust
pub trait EventSink: Send + Sync + 'static { fn emit(&self, event: &str, payload: serde_json::Value); }
#[derive(Clone)] pub struct Events(Arc<dyn EventSink>);   // events.emit("fs-changed", change)
```

Any `Fn(&str, Value)` closure is a sink. `src-tauri` builds one per command from its `AppHandle`
(`crate::events(app)`, which calls `AppHandle::emit`); the native app would pass a closure that sends
into a GPUI channel. Event names and payload shapes are unchanged (`fs-changed`, `build-output`,
`run-output`, `agent-feed`, `agent-event`, `ralph-event`, `ralph-prd-progress`). Process managers
(`lsp`, `dap`) don't emit: `start` takes `on_message(id, raw)` and `on_exit(id, …)` callbacks, and the
Tauri wrapper turns those into `lsp-message`/`lsp-exit` and `dap-message`/`dap-exit`.

**What moved** (with their tests, 124 Rust tests in total, as before plus one for `events`):
`config`, `proc`, `git`, `github`, `links`, `workspace`, `health`, `integrations`, `usage`, `repo`,
`review`, `search`, `notes`, `files`, `artifacts`, `lsp`, `dap`, `watch`, `build`, `run`,
`agent_hooks`, `agent/*` (plan, live feed, runner, stream parsing) and `ralph/*`. Command bodies that
held logic became core functions (`notes::add`, `lsp::cpp_setup`, `ralph::start`, `repo::briefs`,
`files::*`…); the wrappers only call them.

**What stays in `src-tauri`, and why:**

- `pty.rs` + `term_image.rs`: the PTY streams output to the webview through a Tauri `ipc::Channel`
  with flow control and scrollback files, and uses `portable-pty 0.8` where the native terminal uses
  0.9 with its own VT. Unifying the two is the *Sessions* stage's job (move the PTY registry,
  scrollback and flow control to core behind an output callback, on one `portable-pty` version).
- `commands.rs` (93 commands): most are one-line wrappers over core, but some still hold logic
  inline: service/folder/group editing in the config, `ws_create_pr(s)`, `ws_checks` aggregation,
  `check_logs`, `open_in_editor`. Not moved here to keep this stage reviewable; the *Git and
  workspace flows* stage should move each into core as it gets a native UI.
- `lib.rs`: the Tauri builder, plugins, the handler list, `events()` and `blocking()`.
- Plugin calls (`tauri_plugin_opener` for reveal/open after an artifact download) stay in the wrappers.

**The native app now uses** `orbit_core::workspace::list` for the sidebar and `orbit_core::lsp::start`
/ `send` / `stop` / `log` for language servers, so server choice (config override, disabled
languages, rust-analyzer's rustup proxy check), clangd's `--compile-commands-dir` lookup, the
real-root working directory of symlinked repos and `proc::cmd`'s environment are the Tauri app's.
Its own `paths::real_path`, `find_compile_commands` and server table are gone. It also calls
`proc::refresh_path` at start (a launcher's bare PATH). Re-checked with the self-tests: F12 and
Ctrl+click on `Initialize` (39:12 of `Game.h`) open `Game.cpp` at 44:10, hover returns clangd's
markdown, the tree loads 3 levels (1,436 rows) in the background, terminals spawn.

## Next stages

Ordered by dependency, with rough sizes (one focused agent-day ≈ 1 unit).

1. ~~**orbit-core extraction**~~: done (stage 2), except `pty` and `commands.rs`'s inline logic.
2. **Shell parity** (2): resizable panes, the activity bar and icons (Lucide is bundled), the
   status bar, toasts and notifications, the custom titlebar, back/forward history, persisted tab
   and session state.
3. **Sessions** (3): agent terminals (claude/opencode) with codenames, status from hooks and the
   spinner parser, resume, the new-session menu, the inbox. Plus terminal flow control and
   scrollback restore through `orbit-core::pty`.
4. **Editor completeness** (3): completion, signature help, code actions, references, document
   symbols, formatting and semantic tokens (providers exist in gpui-component), go to line, F8 next
   problem, the clangd setup banner, Markdown preview, and the Languages settings section.
5. **File tree completeness** (1–2): create/rename/delete/move/drag, reveal, copy path, fs-watch
   refresh, file icons, and a tree-structured model (the flat list does an O(n) path search per
   folder load).
6. **Review** (4–5): changes review across repos, commit review, PR review with threads,
   send-to-PR, address review, F7 diff navigation, and review notes (REVIEW.md).
7. **Git and workspace flows** (4): commit, push, PR, rebase and merge modals, checks, branch
   drift, the repo page, workspace create and add repos, the dashboard, Search Everywhere and
   Find in Files.
8. **Run/Build/Debug** (3–4): run widget, bottom panel, problems, the DAP debugger UI (breakpoints
   in the gutter use `GutterDecoration`).
9. **Settings, Ralph, integrations, usage, artifacts, updates** (3–4).
10. **Packaging** (1): the Vulkan-only wgpu patch (or a launcher with the restricted env),
    `.desktop` and icon, an updater.

## Feature parity checklist

Status of everything the React app does: **done**, **partial** (with what's missing) or **—**
(not started). Files are the React sources.

### App shell & layout
- [ ] partial: main area with tabs, sidebar and focused workspace (`src/App.tsx`). Only terminal, editor and diff tab kinds exist.
- [ ] — Tab kinds dashboard, reviews, settings, integrations, repo, workspace, changes, commit, pr, feature, ralph (`src/App.tsx`)
- [ ] — Custom titlebar window controls (`src/components/WindowControls.tsx`); the system titlebar is used for now
- [ ] — Back/forward navigation history (`src/lib/navHistory.ts`)
- [ ] — Resizable side panes, snapping sidebar (`PaneResizer.tsx`, `SidebarResizer.tsx`)
- [ ] — Error boundary / crash log (`ErrorBoundary.tsx`)
- [ ] — Dashboard page (`src/pages/DashboardPage.tsx`)
- [ ] — Skeletons, Select, CheckBox, PathInput, ConfirmDialog, ContextMenu, Tooltip (gpui-component has equivalents)
- [ ] — Icons: activity bar, file icons, brand icons (`Icons.tsx`, `FileIcons.tsx`, `BrandIcons.tsx`)
- [ ] — Arrow-key list navigation helper (`src/lib/arrowNav.ts`); only the file tree has key navigation
- [ ] — Markdown rendering (`SafeMarkdown.tsx`, `MarkdownPreview.tsx`); gpui-component has a Markdown view

### Workspaces & sidebar
- [ ] partial: workspace list from `config.yaml` / `.workspace.yaml`, click to select. No session entries or status indicators (`src/App.tsx`, `StatusIndicator.tsx`).
- [ ] — Create workspace: blank, from branch, from a tracker card (`WorkspaceCreateModal.tsx`)
- [ ] — Workspace detail page: repos, PRs, checks, plan, artifacts, usage (`WorkspaceDetailPage.tsx`)
- [ ] — Add repositories (`AddReposModal.tsx`)
- [ ] — Branch drift banner (`BranchDriftBanner.tsx`)
- [ ] — Folders card (`FoldersCard.tsx`) and folder scopes `dir:<path>` (`src/lib/folderScope.ts`)
- [ ] — Open in an external editor / reveal folder
- [ ] — Workspace pull and status summary
- [ ] — Race (`Race.tsx`), CI artifacts (`ArtifactsSection.tsx`), AI usage bar (`UsageBar.tsx`)
- [ ] — Plan modal and PLAN.md progress (`PlanModal.tsx`, `PlanProgress.tsx`)
- [ ] — Integrations page (`IntegrationsPage.tsx`)

### Repos / worktrees
- [ ] — Settings → Repos, repo form, GitHub repo picker, groups (`ReposSection.tsx`, `RepoFormModal.tsx`, `GithubRepoModal.tsx`, `GroupsSection.tsx`)
- [ ] — Repo page: branches, pull, commit, stash, history, compare, cherry-pick, revert, prune (`RepoPage.tsx`)
- [ ] — Repo briefs refresh and background fetch (`src/lib/useRepoBriefs.ts`)

### Sessions / agents
- [ ] — New session menu (`NewSessionMenu.tsx`), codenames (`src/lib/names.ts`)
- [ ] — Agent status from spinner lines and Claude hooks (`src/lib/agentStatus.ts`, backend `agent_hooks.rs`)
- [ ] — Resume claude/opencode sessions, initial prompt injection (`TerminalPane.tsx`)
- [ ] — Live agent feed (`AgentFeed.tsx`)
- [ ] — Ralph view, PRD and run modals (`RalphView.tsx`, `RalphPrdModal.tsx`, `RalphRunModal.tsx`)

### Terminal
- [ ] partial: PTY terminal with spawn, write, resize, 10k scrollback, 256/true colour, cursor shapes, OSC title, bracketed paste, alternate-screen wheel. No flow control, scrollback restore or forget (`TerminalPane.tsx`).
- [x] done: selection (drag, word, line), copy (`Ctrl+Shift+C`, primary on select), paste (`Ctrl+Shift+V`, `Shift+Insert`, middle click)
- [x] done: mouse reporting to TUIs (X10/normal and SGR; click, drag, motion, wheel)
- [ ] — Kitty graphics (`src/lib/kittyGraphics.ts`)
- [x] done: theme-driven ANSI colours (Orbit Dark only)

### File tree / right dock
- [ ] partial: file tree with lazy background loads, `.gitignore`, symlinked repos, keyboard navigation, single click previews, double click or Enter keeps the tab. No create/rename/delete/move/import/reveal/copy path (`FileTreePanel.tsx`).
- [ ] — Auto-refresh on disk changes (`src/lib/useFsChanges.ts`)
- [ ] — Files / Git / Search dock toolbar, Git panel (`GitPanel.tsx`)

### Tabs
- [x] done: preview tabs (reuse the preview slot; editing or double click keeps it) (`src/lib/previewTabs.ts`)
- [x] done: sessions and files groups with a divider, dirty dot, `Ctrl+W` closes the active file tab (refused when unsaved; `Ctrl+Shift+W` discards)
- [x] done: definitions in other files open as editor tabs
- [ ] — Tab context menu (keep open, close others/saved), rename session tabs, unsaved-changes dialog

### Editor
- [x] done: code editor with tree-sitter highlighting (C, C++, Rust, TS/TSX, JS, JSON), line numbers, undo/redo
- [x] done: `Ctrl+S` save, dirty state
- [ ] — `Ctrl+G` go to line, `F8` next problem, line endings, Markdown editor/preview toggle (`EditorPane.tsx`)
- [ ] partial: file path bar (path, diagnostics count, server name, Diff button)
- [ ] — clangd setup banner (missing compile DB, run CMake, restart server)
- [ ] — Workspace symbol search (`SymbolSearch.tsx`)

### LSP
- [x] done: client over stdio, one server per (repo, language), started on demand by `orbit_core::lsp` with the Tauri app's server choice and config (`src/lib/lsp/client.ts`, `manager.ts`)
- [x] done: path mapping for symlinked repos (`src/lib/lsp/paths.ts`)
- [x] done: definition (F12, Ctrl+click), hover, diagnostics
- [ ] — declaration, type definition, implementation, references, highlights, document symbols, completion, signature help, formatting, semantic tokens, code actions (gpui-component has providers for completion, code actions and semantic tokens)
- [ ] — Diagnostics filter from config; Languages settings (server per language, logs, restart) (`LanguagesSection.tsx`)

### Search
- [ ] — Search Everywhere (double Shift, `Ctrl+Shift+A`) (`SearchEverywhereModal.tsx`, `src/lib/fuzzy.ts`, `src/lib/commands.ts`)
- [ ] — Find in Files (`FindInFiles.tsx`)
- [ ] partial: find within a file. gpui-component's editor has search; it is untested and has no binding here.

### Review / diff / PR
- [ ] partial: diff tab for one file's working-tree changes, split/unified (`ReviewPane.tsx`). No commit diffs, no F7 navigation (`useDiffNav.ts`).
- [ ] — Changes review across repos, commit review (`ChangesReview.tsx`, `CommitReviewPane.tsx`)
- [ ] — Inline review zones and threads, REVIEW.md notes, send to PR (`useReviewZones.tsx`, `ReviewThreads.tsx`, `src/lib/notes.ts`, `SendToPrModal.tsx`)
- [ ] — PR review and submit, Code Review page, feature page, PR checkout, address review (`PrReviewPane.tsx`, `SubmitReview.tsx`, `CodeReviewPage.tsx`, `FeaturePage.tsx`, `PrCheckoutModal.tsx`, `AddressReview.tsx`)

### Git operations
- [ ] — Commit, push, PRs, rebase, merge modals (`CommitModal.tsx`, `PushModal.tsx`, `PrsModal.tsx`, `RebaseModal.tsx`, `MergeModal.tsx`)
- [ ] — Checks: list, rerun, logs, investigate, fix; discard, sync branch

### Run / Build / Debug
- [ ] — Run widget with configs and CMake presets, `Ctrl+F9`/`Shift+F10`/`Shift+F9`/`Ctrl+F2` (`RunWidget.tsx`, `src/lib/runStore.ts`)
- [ ] — Bottom panel: build output, run terminal, problems, debug (`BottomPanel.tsx`, `src/lib/problems.ts`)
- [ ] — Multi-repo build modal (`BuildModal.tsx`)
- [ ] — DAP debugger: session, gutter breakpoints, debug view, stepping keys (`src/lib/dap/*`, `DebugView.tsx`)

### Settings / themes
- [ ] — Settings page and sections: AI, Appearance, Health, Updates (`SettingsPage.tsx`, `AiSection.tsx`, `AppearanceSection.tsx`, `HealthSection.tsx`, `UpdatesSection.tsx`)
- [ ] partial: themes. Orbit Dark only (tokens from `src/App.css` / `src/lib/theme.ts`, gpui-component theme JSON); the other six themes and the picker are not ported.

### Notifications
- [ ] — Toasts, inbox, OS notifications (`Toast.tsx`, `Inbox.tsx`); there is only a status-bar notice

### Backend services
- [x] done: the backend is shared through `crates/orbit-core` (config, workspaces, git, GitHub, LSP/DAP processes, watcher, builds, run configs, agents, Ralph, review, search, notes, files); the native app uses its workspace list and LSP manager
- [ ] partial: no native UI writes state yet; the core functions for it exist (`workspace::create`, `repo::commit`, `notes::add`, …)
- [ ] — FS/git watcher events in the native UI (`orbit_core::watch::watch` with a channel sink), run configs (`.orbit/run.json`)
- [ ] — PTY registry, scrollback and flow control in core (still `src-tauri/src/pty.rs`)
