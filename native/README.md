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
scripts/package-linux.sh   # dist/: a tarball with install.sh, and an AppImage when appimagetool is found
```

The app lists workspaces and starts language servers through `orbit-core`, the same code as the
Tauri app, so it shows the same workspaces and honours Settings → Languages (server overrides,
disabled languages, the diagnostics filter). `target/` is ~3.3 GB after a
release build plus clippy and tests.

The first start after a build takes several seconds while the GPU driver compiles and caches
pipelines. Warm starts reach the first frame in 200–250 ms.

Keys (JetBrains-style, as the Tauri app; `Ctrl+Shift+A` lists every command with its keys):
`Ctrl+Shift+T` new terminal (a shell session) · `Ctrl+W` close the file tab (in a terminal the shell gets it; unsaved
changes ask Save / Don't Save / Cancel) · `Ctrl+Shift+W` close any tab without saving ·
`Ctrl+Tab` / `Ctrl+Shift+Tab` next / previous tab · `Alt+←` / `Alt+→` and the mouse side buttons
back / forward (in a terminal `Alt+←/→` go to the shell) · double `Shift` Search Everywhere
(`Tab` switches its tabs) · `Ctrl+Shift+A` Find Action (its Actions tab) · `Ctrl+Shift+F` Find in Files
(`Alt+C`/`Alt+W`/`Alt+X` case, words, regex; `Tab` to the file mask) ·
`Ctrl+Alt+A` new agent session · `Ctrl+Alt+R` resume the last session · `Ctrl+Alt+I` next session
needing input ·
`Ctrl+Q` quit · in the editor `Ctrl+S` save, `F12` or `Ctrl+click` go to definition, hover for
docs, `Shift+F12` references, `Ctrl+F12` implementation, `Ctrl+Shift+O` file outline, `Ctrl+T`
project symbols, `Ctrl+G` go to line, `F8`/`Shift+F8` next/previous problem, `Ctrl+Space`
completion (also as you type), `Ctrl+Shift+Space` parameter hints, `Ctrl+.`/`Alt+Enter` quick
fixes, `F2` rename, `Ctrl+Shift+I`/`Shift+Alt+F` format · in a terminal `Ctrl+Shift+C`/`Ctrl+Shift+V` copy/paste, drag to select (double click: word,
triple: line; `Shift` overrides an app's mouse mode), middle click pastes the primary selection.
Build/Run/Debug: `Ctrl+F9` build, `Shift+F10` run, `Shift+F9` debug, `Ctrl+F2` stop, `Ctrl+F8`
toggle breakpoint, while paused `F9` resume, `F8` step over, `F7` step into, `Shift+F8` step out.
Tabs: right click for the tab menu, middle click closes, double click keeps a preview tab.
File tree: arrows, `Home`/`End`, `Enter` opens, `F2` rename, `Delete` to the Trash, right click for
the menu; the editor's breadcrumb folders reveal in it.

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
    --do "rclick:tab:1;click:menu:Close Other Files;dump:after"   # shell steps, below
    --state /tmp/s.json                 # restore from / save to this state file (none otherwise)
    --exit-after 40 --dump out.txt      # quit after 40 s and write the dump (tabs, editor, LSP, diagnostics, terminal grid, layout, history, overlays…)
ORBIT_CAPTURE=shot.png ./target/release/orbit-native …   # built with --features capture: saves each frame as PNG
scripts/measure.sh 8 [args…]            # PSS (+SwapPss) of the process tree after 8 s; language servers apart
```

In self-test mode, copies are recorded in the dump and never written to the system clipboard.

Sessions and free-form steps run after the fixed ones (`;`-separated):

```sh
./target/release/orbit-native --session shell --session claude   # open sessions (shell, claude, opencode, omp, any command)
    --steps "status:waiting@15; dump; action:sessions::NextSessionNeedingInput; wait:Allow Bash@5; \
             type:y\n; keys:ctrl-c; sleep:800; action:sessions::ResumeLastSession"
```

`wait:<text>[@secs]` waits for text in the active terminal, `status:<status>` for a session in that
status, `action:<name>` dispatches any registered GPUI action by name, `type:`/`keys:` go through
GPUI's keystroke dispatch, `dump` writes the sessions, inbox and recent list into the log. Waits
also record how late the UI thread ran its timers ("ui timer lag"), which is how a blocked UI shows.
App commands (closing a tab…) are not GPUI actions by name: run them with `--do "cmd:<id>"`.
The dump always ends with the sessions, inbox and recent list.

Self-test runs neither save nor restore sessions in the user's config. Point `ORBIT_CONFIG_DIR` at a
scratch folder to test that (and to keep scrollback and agent settings files out of the real one).
Agent sessions are tested without a real agent: `scripts/stub-agent/claude` stands in for Claude
Code (prints its argv, posts the hooks from Orbit's `--settings` file, waits for an answer); put it
first on `PATH`. Use `SHELL=/bin/bash` (zsh may start an interactive setup wizard).

`--do` runs shell steps last, separated by `;` (`src/steps.rs`). Mouse steps target elements that
render a `probe("name")` (tabs `tab:<ix>`, menu items `menu:<label>`, dialog buttons
`dialog:<label>`, palette rows `palette:<title>` (files by name, Find matches `palette:<path>:<line>`),
Search Everywhere tabs `search-tab:<Label>`, Find's `find-toggle:case|word|regex` and
`find-repo:<repo>|All repos`, tree rows `tree:<name>`, breadcrumb folders `crumb:<ix>`, `tb-sidebar`, `tb-back`, `tb-forward`,
`tb-inbox`, `tb-dock`, `new-tab`, `resize-sidebar`, `resize-dock`, `resize-panel`,
`panel-close`, `toast-close:<id>`, `ws:<name>`) and are dispatched with `Window::dispatch_event`:
`click:` `rclick:` `mclick:` `dblclick:` `drag:<probe>:<dx>,<dy>`; plus `key:<spec>`,
`type:<text>`, `shift:<n>` (bare Shift taps as modifier events; `shift:2` is a double Shift), `mouse:back|forward`, `cmd:<command id>`, `open:<path>`, `preview:<path>`,
`toast:<text>`, `wait:<ms>` and `dump:<label>` (the full dump, into the log, at that point).
Build/Run/Debug adds (`src/run/script.rs`) `until:<text>[@secs]` (wait until the dump contains
`text`, e.g. `until:build Ok@300`, `until:debug: Stopped@60`) and `gutter:<line>` (click the active
editor's breakpoint strip at that line); its probes are `run-config`, `run-preset`, `run-build`,
`run-run`, `run-debug`, `run-stop`, `panel-build|run|debug|problems`, `panel-clear`, `problem:<ix>`,
`dbg-resume|pause|over|into|out|stop`, `dbg-eval` (the console input), `frame:<ix>`, `thread:<id>`,
`var:<name>`. The git and workspace pages' probes are listed in *Stage: git and workspace flows*.

## Commands and keymap (the extension point)

`src/commands.rs` holds every command a user can run by name or key. The palette (`Ctrl+Shift+A`,
`src/palette.rs`) lists the ones that can run now, with their first shortcut. A stage registers
its commands from an `init(cx: &mut App)` that `main.rs` calls after `app::init`:

```rust
use crate::commands::{self, Command};

pub fn init(cx: &mut App) {
    // Runs on the root view (`OrbitApp`): open tabs, panels, dialogs.
    commands::register(cx, Command::new("run.build", "Build").keys(["ctrl-f9"])
        .when(|app, cx| app.workspace().is_some())      // hidden from the palette, key passes through, when false
        .run(|app, window, cx| { /* … */ }));
    // A key for one view: bind that view's own action in its key context.
    commands::register(cx, Command::new("editor.goToLine", "Go to Line").keys(["ctrl-g"])
        .context("OrbitEditor").action(GoToLine));
}
```

- `id` is unique: registering an id again replaces the command (its title, handler and
  predicates; keys bound earlier stay bound to it). `app.newTerminal` is the one the sessions stage
  is meant to take over: the tab strip's `+` runs whatever is registered under it.
- `keys` use GPUI's syntax (`ctrl-shift-t`, `alt-left`, `f12`, `shift shift` for a double Shift);
  `commands::display_keys` turns them into `Ctrl+Shift+T`.
- `.run(f)` binds the keys to `RunCommand { id }`, handled by the root view;
  `.action(a)` binds them to the action itself, dispatched to the focused view (from the palette
  too, after it closes and the focus is back).
- `.when(p)`: can it run now? `.key_when(p)`: extra condition for the shortcut only. When either
  fails, the key propagates to the focused view: `Ctrl+W` and `Alt+←/→` reach a shell this way.
- While a dialog, menu or the palette is open, app shortcuts do nothing.
- `commands::run(id, app, window, cx)` runs one from code (menus, buttons, scripts).

Other shell extension points on `OrbitApp`: `toast(kind, title, description, cx)` /
`toast_error`; `open_menu(position, Vec<MenuItem>, …)`; `open_dialog(Dialog { title, message,
buttons, on_cancel }, …)`; `titlebar_items: Vec<AnyView>` (the run widget and the inbox go there);
`state.rs` for what is restored on start.

## Stack and why

| Piece | Choice |
|---|---|
| UI framework | `gpui-pre =0.3.11`: a crates.io snapshot of Zed's GPUI (zed@f816e1f), plus `gpui-pre-platform` for the native backend (Wayland/X11, wgpu renderer) |
| Components | `gpui-component =0.9.0` (Apache-2.0): code editor, diff view, theme and widgets |
| Editor | gpui-component's `EditorState`: rope buffer, tree-sitter highlighting (C, C++, Rust, TS/TSX, JS here), undo, search, folding, and LSP hooks (`DefinitionProvider`, `HoverProvider`, completion, code actions, semantic tokens, document colors, `show_document`) |
| Diff | gpui-component's `Diff`: virtualized unified/split patch view with app-supplied syntax colors |
| Terminal | `alacritty_terminal 0.26` (VT parsing, grid, selection) on `orbit_core::pty` (`portable-pty 0.9`, shared with the Tauri app), painted on a GPUI canvas (own code, ported from the PoC) |
| LSP | our own small JSON-RPC client (`src/lsp`), with types from `lsp-types =0.97` (the version gpui-component's traits use) |

**Why gpui-component and not crates.io `gpui 0.2.2` (the PoC's).** The editor is the gate, and
gpui-component already has an LSP-ready code editor and a diff view; writing those from scratch on
gpui 0.2.2 would be months. gpui-component 0.9 requires the `gpui-pre` snapshot rather than gpui
0.2.2, and both are on crates.io, so no Zed checkout or git dependency is needed. It built on this
machine (CachyOS, GNOME Wayland, Intel + NVIDIA, Vulkan) on the first try and needs no `libc` pin.
One thing was not obvious: `gpui-pre` ships without a platform, so `gpui-pre-platform` (with the
`wayland` and `x11` features) is a separate dependency.

**The cost: memory.** `gpui-pre` renders through wgpu, where gpui 0.2.2 used blade. Upstream, the
wgpu instance asks for the Vulkan *and* GL backends and enumerates every adapter. That maps both
vendors' Vulkan drivers, NVIDIA's EGL stack and Mesa's (`libgallium` + `libLLVM`), and opens
`/dev/nvidia0` even though the window renders on the Intel GPU. Since the packaging stage the app
asks for Vulkan only (GL when there is no Vulkan GPU), through a vendored, two-hunk patch of
`gpui-pre-wgpu`: see *GPU backend*. Anonymous (heap) memory is only ~37 MB; the rest is shared
driver libraries and the ~60 MB binary.

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

The table above predates the Vulkan-only renderer; *Stage: packaging* has the current numbers.

Other numbers:

| Metric | Value |
|---|---|
| Start to first frame | 190–260 ms warm (180–190 ms in stage 1, before the Vulkan probe and later stages); ~9 s on the very first run after a build (driver pipeline cache) |
| Binary (release, stripped, thin LTO) | 61 MB (53 MB in stage 1); the tarball is 21 MB |
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

## Verified in stage 4 (editor completeness)

Self-tests against aether-engine (read-only: nothing saved) and a scratch CMake repo in a scratch
workspace (`ORBIT_WORKSPACE_ROOT` and `ORBIT_CONFIG_DIR` pointed at a temp dir) for what writes.
`--keys` now also takes `@namespace::Action`, `wait` (which draws a frame: a hidden window gets no
frame callbacks, and gpui-component's overlays sync when drawn) and `type:text`; the dump shows the
status bar, breadcrumb, picker, completion menu, signature help and banner.

- **Completion.** Typing `PhysicsW` on an empty line of `Game::Initialize` lists `PhysicsWorld |
  Physics::PhysicsWorld | …`; Enter inserts `PhysicsWorld`. Items are ranked by match (prefix,
  substring, subsequence), then the server's `sortText`. The document is synced before each
  request: the editor asks for completions before it reports the keystroke as a change.
- **References** of `Initialize` (39:12 of `Game.h`, `Shift+F12`): `Game.h:39` and `Game.cpp:44`
  once clangd has both files; picking a row opens it. Implementation and type definition share the
  path (one result jumps, several list).
- **Outline** of `Game.h` (`Ctrl+Shift+O`): namespace `Game` with its functions and variables,
  indented, filtered by typing (`init` → `Initialize`, `UnInitialize`…).
- **Project symbols** (`Ctrl+T`, `addN`): `addNumbers  function · cpp/util.cpp:2`.
- **Quick fixes** (`Ctrl+.` on an unused include): "remove #include directive", "remove all unused
  includes"; Enter removes the line. Commands that answer with `workspace/applyEdit` (clangd's
  tweaks) are applied too.
- **Signature help**: typing `addNumbers(1, ` shows `addNumbers(int a, [int b]) -> int`.
- **Rename** (`F2` on `addNumbers` → `sumOf`): the open file in its buffer, `util.h` and `util.cpp`
  on disk (other open editors in their buffers); **format** (`Ctrl+Shift+I`) reindents with clangd;
  checked with `git diff` in the scratch repo after `Ctrl+S`.
- **Go to line** (`Ctrl+G`, `6:7`), **next problem** (`F8`, wraps, message in the status bar).
- **clangd banner**: the scratch repo without `compile_commands.json` shows it; "Generate with CMake"
  (or `@orbit_editor::GenerateCompileCommands`) runs `cmake -S . -B build -G Ninja
  -DCMAKE_EXPORT_COMPILE_COMMANDS=ON` (a preset when the repo has them), and on success the banner
  goes and clangd restarts for every editor using it.
- **Status bar**: `⨯ 0 ⚠ 2 | Unsaved | Ln 25, Col 3 | LF | Spaces: 4 | C++ | clangd · indexing:
  1/45` (indent detected from the file; the LSP chip shows `$/progress`, "stopped · Restart" when the
  server exits, and restarts on click).
- The stage-1 gate still passes (F12 on `Initialize` opens `Game.cpp` at 44:10) once clangd has
  indexed `Game.cpp`; asked in the first seconds it answers with the declaration itself.

gpui-component limits met on the way:

- No snippet engine: the client asks for plain completions (`snippetSupport: false`);
  `completionItem/resolve` is never called by the component (clangd sends docs inline anyway), and
  additional text edits are dropped (clangd runs with `--header-insertion=never`).
- No signature help UI: ours is a popup anchored above the caret.
- Its code action menu's Enter also inserted a newline into the text, so `Ctrl+.` opens our own
  list instead (the component's provider hook stays installed).
- No semantic tokens yet: the component wants range requests, which clangd doesn't serve.
- Not done: the "hide this diagnostic" quick fix and the diagnostics filter from config, Markdown
  preview, the Languages settings section. (Folder crumbs reveal in the tree since the files stage.)

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
| `src/main.rs` | app start, window (client-side decorations), each stage's `init`, state restore, LSP shutdown and state save on quit |
| `src/app.rs` | root view state: workspaces, tabs and their logic (preview, batch closes, unsaved dialog, tab menu), back/forward, pane sizes, state snapshot/restore, the app's commands |
| `src/shell.rs` | rendering: titlebar, tab strip, main area and bottom panel, the dock (Files / Git), the usage bar, resizers, inbox and New Session popovers, overlays |
| `src/sidebar.rs` | the left sidebar and the status indicator |
| `src/orbit_icons.rs` | the icons of `Icons.tsx` (generated), drawn with `icons::orbit_icon` |
| `src/commands.rs` | command registry and keymap |
| `src/nav.rs` | back/forward history (`src/lib/navHistory.ts`) |
| `src/state.rs` | saved state (`<config dir>/native-state.json`): workspace, tabs, active tab, cursors, pane sizes |
| `src/overlay.rs`, `src/toast.rs` | context menus and dialogs, toasts |
| `src/palette.rs` | the search overlay: Search Everywhere (Find Action is its Actions tab) and Find in Files, rows in a virtual list |
| `src/search/` | their commands and keys, symbol and text sources, Find's hit ranges (`mod.rs`); fuzzy ranking (`fuzzy.rs`, `src/lib/fuzzy.ts`) |
| `src/icons.rs` | asset source: gpui-kit-assets' Lucide icons plus file-type glyphs (`FileIcons.tsx`) and tab icons (`Icons.tsx`) |
| `src/probe.rs`, `src/steps.rs` | element bounds and `--do` steps for the self-test |
| `src/editor.rs` | editor tab: `EditorState`, save, dirty, actions and keys (`editor_actions()`), pickers, problems, format, rename, signature help, breadcrumb, status bar, clangd banner |
| `src/editor/provider.rs` | the LSP document (sync) and gpui-component providers: definition, hover, completion, code actions; WorkspaceEdit and location parsing |
| `src/editor/picker.rs` | the quick-pick overlay: go to line, outline, project symbols, references, rename, quick fixes |
| `src/lsp/` | `mod.rs` file → language and the server registry · `server.rs` JSON-RPC client and URI mapping over `orbit_core::lsp` (which picks, starts and feeds the process) · `paths.rs` URIs and root rebase |
| `src/diff_view.rs` | diff tab over `git diff HEAD` (untracked files too), F7 navigation, and REVIEW.md notes inline in a workspace repo |
| `src/review/` | the review tabs (`mod.rs`: tab, commands, sidebar entries, dump) · `changes.rs` changed files and notes · `prs.rs` PRs, status, checks, logs, creation · `send.rs` notes to a PR review · `notes.rs` REVIEW.md and note anchoring · `ui.rs` small widgets |
| `src/terminal/` | terminal view over `orbit_core::pty`: VT parse, status tracking, scrollback replay (`mod.rs`), grid layout and paint (`grid.rs`), keys and mouse reports (`input.rs`) |
| `src/sessions/` | sessions (`mod.rs`: keys, codenames, save/restore, inbox, recent, resume, actions), agent status heuristics and hooks (`status.rs`) |
| `src/file_tree.rs` | virtualized tree with background folder loads, reveal, refresh after disk changes, name prompt and file operations (`orbit_core::files`) |
| `src/file_ops.rs` | the tree's ties to the app: context menu, the workspace watcher (`orbit_core::watch`; tree refresh, editor reload), following the active file, breadcrumb reveal |
| `src/workspace.rs` | workspace list from `orbit_core::workspace` |
| `src/run/` | Build/Run: the store and commands (`mod.rs`: configurations, build, run terminal, problems as diagnostics), titlebar widget (`widget.rs`), bottom panel (`panel.rs`), compiler message parser (`problems.rs`), saved selection and breakpoints (`saved.rs`), self-test steps (`script.rs`) |
| `src/debug/` | the DAP session (`mod.rs`: launch sequence, stops, frames, variables, console, breakpoints), the DAP client over `orbit_core::dap` (`client.rs`), editor breakpoints and paused line (`gutter.rs`), the Debug tab (`view.rs`) |
| `src/workspace_ui/` | page tabs (`mod.rs`: `PageTab`, its in-tab modal, commands, sidebar entries, dump) · `dashboard.rs` · `workspace_page.rs` (delivery flow, repos, PRs) · `repo_page.rs` (a repo's base clone) · `forms.rs` (new workspace, add repositories) |
| `src/git/` | the git flows shown over a page: `commit.rs`, `push.rs`, `rebase.rs`, `merge.rs`, the branch drift banner (`drift.rs`); shared modal pieces in `mod.rs` |
| `src/pages/` | tool tabs (`TabKind::Tool`; `mod.rs`: `PageTab`, `PageView`, `Host`, commands, sidebar entries, dump) · `settings.rs` (sections, AI, Languages, Appearance, Health, Updates) · `repos.rs` (Repositories, Folders, Groups) · `integrations.rs` · `ralph.rs` · `usage.rs` · `artifacts.rs` · `updates.rs` (update check) · `crash.rs` (panic hook) · `ui.rs` |
| `src/script.rs` | self-test mode and dumps |
| `src/theme.rs` | the seven themes of `src/lib/theme.ts` as colour tokens (`c(theme::PANEL)` resolves against the active theme), the gpui-component theme built from them, and the shell metrics (`src/App.css`) |
| `vendor/gpui-pre-wgpu`, `vendor/PATCHES.md` | the wgpu renderer with the Vulkan-only patch, and how to re-apply it |
| `packaging/linux/`, `scripts/package-linux.sh` | `.desktop` entry, `install.sh`, and the tarball / AppImage build |

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

- `term_image.rs`: reads files named by kitty graphics commands for the webview.
  (`pty.rs` moved to core in stage 3 and is now a thin adapter, see *Stage 3*.)
- `commands.rs` (93 commands): one-line wrappers over core. (The PR commands moved to
  `orbit_core::prs` in the review stage, the git and workspace ones in the git flows stage, the
  config edits, `open_in_editor`, usage and integrations in the settings stage.)
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

## Stage 3: sessions and the terminal

**The PTY is in core** (`crates/orbit-core/src/pty.rs`), on `portable-pty 0.9` for both apps. In
`src-tauri/Cargo.lock` only portable-pty's own subtree moved (0.8.1 → 0.9.0, `nix` 0.25 → 0.28,
`serial` → `serial2`); no other Tauri dependency changed.

```rust
pub fn reserve_id() -> u32;                                   // id first, so the handler can know it
pub fn spawn(id: u32, spec: Spawn, on_output: impl FnMut(Output) + Send + 'static) -> Result<(), String>;
pub enum Output { Data(Vec<u8>), Exit }                       // bursts (8 ms / 256 KB), then Exit once
pub fn write(id, &[u8]) / resize(id, Size) / set_paused(id, bool) / kill(id);
pub fn session_dir(workspace, repo, cwd) -> Result<PathBuf>;  // cwd > repo folder > scope folder
pub fn scrollback(key) -> Vec<u8>; pub fn forget(key);       // saved tail per tab key
pub fn claude_session_exists(workspace, id) -> bool; pub fn opencode_session_id(workspace, repo, since_ms);
// Spawn { cwd, cmd (None: $SHELL), args, env, size, key, scrollback: bool, program_version }
```

`spawn` keeps everything the Tauri command did: the child environment (outer-terminal variables
dropped, `TERM`, `TERM_PROGRAM=Orbit`, claude session variables removed, AppImage host env), claude's
hooks `--settings` and `--remote-control`, spawning at the real size, 8 ms output bursts, the
256 KB scrollback tail flushed every 2 s, and pause/resume (`set_paused`) for the webview's flow
control. One change: the queue between the PTY reader and the burst thread is bounded (64 reads), so
a consumer slower than the program holds the program back instead of buffering without limit.

`src-tauri/src/pty.rs` is now a ~100-line adapter: the same commands (`pty_spawn`, `pty_write`,
`pty_resize`, `pty_flow`, `pty_kill`, `pty_scrollback`, `pty_forget`, `claude_session_exists`,
`opencode_session_id`) with the same arguments, the webview `ipc::Channel` (an empty message once
the program ended) and `pty-error` events for a failed start, as before.

**Native sessions.** Every terminal tab is a session, as in the React app: a key
(`tm:<ms>-<rand>`), a codename, a scope (the selected workspace, else `dir:~`) and what it runs.

- New: the sidebar's *Sessions* block (`+ shell`, `+ claude`, `+ opencode`, `+ omp`), `Ctrl+Shift+T`
  (a shell), or the actions. Claude starts with `--session-id <uuid>` so it can be resumed.
- Status: the spinner-verb heuristics of `agentStatus.ts` (thinking/running/editing/busy/idle/
  exited) plus Claude Code's hooks through `orbit_core::agent_hooks` with an `Events` sink into a
  GPUI channel (waiting for permission, working, done). Shown in the tab title and the sidebar dot.
  Agents without hooks get "done" after 10 s of activity ending in silence.
- Inbox: "finished" and "needs your attention" items, unread unless the session's tab is active in
  a focused window; seeing the tab settles them. No OS notifications yet.
- Closing an agent tab ends it: claude (its `--session-id`) and opencode (looked up with
  `opencode session list`) go to *Recent* for the scope; clicking one (or `ResumeLastSession`)
  reopens it with `--resume <id>` / `--session <id>`. Tabs don't minimize as in React (that needs a
  hidden-tab notion in the shell).
- Restart: open sessions and Recent are saved to `<config>/native-sessions.json` (the React app
  keeps its own in localStorage). On start, claude sessions come back with `--resume` (or
  `--session-id` when Claude has no transcript yet); the others replay their saved output with the
  screen clears turned into scrollback, under a "restored after restart" line, then start fresh.
- Flow control: the output thread parses into the terminal in 16 KB slices under the grid lock, and
  since the core's read queue is bounded, a flooding program runs at the parser's pace. The UI
  thread only learns "something changed" (one pending wake-up) and repaints once per frame.
- Actions for the command registry and keymap (`sessions::…`): `NewAgentSession` (Settings → AI
  agent), `NewClaudeSession`, `NewOpencodeSession`, `NewOmpSession`, `NewShellSession`,
  `ResumeLastSession`, `NextSessionNeedingInput`, `MarkInboxRead`, `ClearInbox`. They are global
  handlers, so they work from any focus; none is bound to a key yet.

Verified by self-test runs with `ORBIT_CONFIG_DIR` in a scratch folder, `SHELL=/bin/bash` and the
stub agent on `PATH` (no real agent was started):

- Hooks: the stub's `UserPromptSubmit` → `Notification(permission_prompt)` turned the session
  *waiting* 0.4 s after start, with an inbox item "… needs your attention · Home · Claude needs
  your permission to use Bash" (unread while another tab was active).
- `NextSessionNeedingInput` from the shell tab activated the waiting claude tab and settled its
  item; typing `y` reached the stub, whose `Stop` hook added a "finished" item.
- Closing the claude tab put its session id in Recent; `ResumeLastSession` reopened it and the
  stub's argv was `--settings …/agent-settings/<key>.json --resume <the same id>`.
- Restart: a second run on the same config dir restored both tabs: the shell replayed
  `echo RESTORE-$((40+2))` / `RESTORE-42` above "── restored after restart ──" and a new prompt;
  claude came back with `--session-id <id>` (no transcript, as expected for the stub).
- Flooding: `yes | head -n 2000000` finished in 0.66 s with the UI thread's timers at most 1 ms
  late (339 frames, 0.5 ms average layout+paint, 8 ms max); `Ctrl+C` after 3 s of an endless `yes`
  gave the prompt back within 50 ms.
- `pnpm run check` passes with the adapter (Tauri app unchanged for the frontend); core has two new
  PTY tests (round trip with exit; pause stops reading until resumed).

## Files and search stage

**How it hangs together.** The palette overlay (`src/palette.rs`) is now the search modal, with
two modes: Search Everywhere and Find in Files; `OrbitApp::palette` still gates app shortcuts, and
those now propagate while an overlay is open, so the overlay's own keys (Tab, `Alt+C`, double
Shift to close) get them. `src/file_ops.rs` attaches to the root view with `cx.observe_new`: it
handles the tree's `TreeRequest`s (context menu, toasts), moves the `orbit_core::watch` watcher
when the workspace changes (started off the UI thread: Linux walks every folder), and observes the
app to make the tree follow the active editor. File operations call `orbit_core::files`
(`create_node`, `rename_node`, `delete_node`) with the tree path split into workspace, repo and
repo-relative path; names are typed in a prompt above the tree. "Open in Terminal" starts a shell
session in that folder (`SessionMeta::cwd`, saved with the session).

**Verified** by self-tests: Search Everywhere and Find in Files on `add-items-skills`
(read-only), file operations and watching in a scratch workspace with two git repos
(`ORBIT_WORKSPACE_ROOT`, `ORBIT_CONFIG_DIR`, and `XDG_DATA_HOME` so the Trash is a scratch one):

- Double Shift (`--do "shift:2"`, real modifier events) opens All: 8 files, then `Game.cpp` ranks
  `aether-engine/src/game/Game.cpp` first; Tab → Files (60 rows), Shift+Tab twice → Text; double
  Shift again closes it. `Ctrl+Shift+A` opens Actions (`toggle` → the three Toggle commands).
- Symbols: with clangd on `Game.cpp`, `Initialize` lists 100 hits (`method Initialize
  GPUProfiler.cpp:40`…); Enter opens `GPUProfiler.cpp` at 40:23. Before clangd has indexed, the
  server answers `[]` (the editor's `Ctrl+T` shows nothing either).
- Text: `GameLoop` lists 16 matches (git grep over the four repos takes 10–15 s on this
  workspace, as in the Tauri app); Enter opens `Application.cpp` at 352 with `GameLoop` selected,
  and the tree follows to that file.
- Find in Files: `GameLoop::` → "2 matches in 1 file" under a `GameLoop.cpp` header; `Alt+C`,
  `Alt+W`, the `aether-engine` chip and a `*.h` mask (Tab to it) are applied. In the scratch
  workspace `println` → 2 matches in 2 files; Enter opens `api/src/main.rs` with `println` selected.
- Tree: opening `api/src/main.rs` expands `src` and selects it; the breadcrumb's `src` crumb
  selects the folder and focuses the tree. The context menu of `src` lists New File…, New Folder…,
  Rename…, Copy Path, Copy Relative Path, Reveal in File Manager, Open in Terminal, Delete. New
  File `new.rs` creates, selects and opens it; Rename → `renamed.rs`; Copy Relative Path gives
  `src/main.rs`; Delete moves `renamed.rs` to the (scratch) Trash with a toast; Open in Terminal
  starts a shell in `api/src`. `F2` opens the rename prompt, Escape cancels it; `Delete` trashes.
- Watching: a script changed files while the app ran. Editing `main.rs` on disk reloaded its
  unmodified editor ("changed on disk" in the buffer); a dirty editor of the same kind was left
  alone. New `added.rs` and `newdir/` appeared in the open `src` folder; `build/` and `.cache/`
  writes were dropped; removing `src/deep` removed its rows; a commit (git event) refreshed every
  open folder.
- Unit tests: the fuzzy ranking (the Tauri app's cases), Find's hit ranges and line display,
  match positions in characters, the tree's refresh merge (expanded subtrees and the selection
  kept) and path location, the noise filter.

## Stage: review and pull requests

Two review tabs per workspace, opened from the sidebar's *REVIEW* entries or the palette (*Review
Changes*, *Pull Requests and Checks*), plus review notes in every diff tab of a workspace repo.

- **Changes** (`src/review/changes.rs`): `orbit_core::git::changes` for each repo (the dock Git
  tab's list), re-read every 3 s in the background; a click opens the file's diff tab. On the right,
  the workspace's REVIEW.md notes: resolve, delete, jump (to the diff while the file has changes,
  else to the file), show/clear resolved, *Fix N notes with agent* (the prompt of `notesPrompt`, sent
  to an agent session of the workspace or a new one) and *Send to PR…*.
- **Diff tab notes** (`src/diff_view.rs`), on gpui-component's `Diff` hooks: `on_add_annotation`
  (the `+` beside the hovered line and at the end of a selection) opens a composer, `annotations` +
  `render_annotation` put each note's card under its line, `decorations` tint multi-line notes.
  Cards have Resolve/Reopen, Reply, Edit and Delete; the composer is an `InputState` (Enter saves,
  Escape cancels). Notes are written with `orbit_core::notes`, so both apps read and write the same
  REVIEW.md, and an agent ticking a box shows up within 2 s. Notes go on the new side only, as in the
  Tauri app. The patch carries the whole file (`--unified=1000000`) and the view folds unchanged
  lines, so a note can sit on any line.
- **Pull requests** (`src/review/prs.rs`): `orbit_core::prs::{list, status, checks}` side by side in
  the background; failed Actions checks get *Logs* (`check_logs`, shown in the page) and *Re-run*;
  repos with no open PR get a create form (include, title, description, *Draft with AI* through
  `agent::pr_draft`) and *Create pull requests* (`prs::create_all`, which saves the refs).
- **Send to PR** (`src/review/send.rs`): the Tauri modal's checks (file in the PR, code pushed, inside
  a hunk of the PR's diff) and one `COMMENT` review per PR, after a dialog listing what can't go and
  why; *Post and remove notes* drops the posted ones from REVIEW.md.

**Core move.** `ws_create_pr(s)`, `ws_prs(_flat)`, `ws_owner_repo`, `ws_pr_status`, `ws_checks`,
`pr_group_checks`, `check_rerun` and `check_logs` held their logic inline in
`src-tauri/src/commands.rs`. It is now `crates/orbit-core/src/prs.rs` (`create`, `create_all`,
`list`, `status`, `checks`, `group_checks`, `rerun`, `check_logs`, plus `worktree_of`, `remote_of`,
`owner_repo`), and the commands are one-line wrappers with the same arguments and results. Core
tests: the check aggregate, PR number and benign-error parsing, the log tail, and `create_all` +
`check_logs` end to end against a scratch workspace with a **stub `gh`** on `PATH` (three repos: one
PR created and saved, one "No commits between" skipped, one failure reported only when no PR
exists).

Keys: `F7` / `Shift+F7` next/previous change in a diff tab. Commands: `review.changes`,
`review.pullRequests`, `review.commentSelection`, `diff.nextChange`, `diff.previousChange`,
`diff.toggleMode`. Self-test probes: `review-changes`, `review-prs` (sidebar),
`review-file:<repo>/<path>`, `review-note:<n>[:done|:delete]`, `review-fix`, `review-send-pr`,
`review-refresh`, `review-show-done`, `review-clear-done`, `note:<n>:done|reply|edit|delete` (cards in
a diff tab, in line order), `composer:submit|cancel|input`, `diff-mode`, `diff-show-done`,
`prs-refresh`, `pr:<repo>#<n>`, `check:<repo>#<n>:<k>:logs|rerun`, `create:<repo>:include|draft`,
`create-prs`, `log-close`. The dump lists each review tab (files, notes, PRs, status, checks, create
rows, log) and each diff tab's notes with their anchored lines and the composer.

Verified by self-tests against a scratch root (`ORBIT_WORKSPACE_ROOT`, `ORBIT_CONFIG_DIR`): a
workspace with two repos cloned from local bare remotes, origins set to GitHub-looking fetch URLs
(never contacted; nothing pushed) and a stub `gh` first on `PATH`. No real PR was created.

- Changes listed `web: M main.rs +3 -2`, `web: U added.txt`, `api: D notes.txt +0 -3`.
- In `main.rs`'s diff, *Add Review Note* (no selection: the first added line), typing and Enter wrote
  ``- [ ] `web/main.rs:3` `` with its snippet to REVIEW.md; Reply appended `Reply: …`; Resolve ticked
  it; the Changes tab showed it on its next refresh.
- Show resolved → Reopen → Edit (`Ctrl+A`, retype, Save) rewrote the body. A script then put two
  lines on top of `main.rs`: within the 2 s poll the diff reloaded (`+5 −2`) and the note sat at line
  5, `moved`. Delete removed the last note, and REVIEW.md with it (as core does).
- Split → Unified with `diff.toggleMode`; F7 dispatched.
- Pull requests: `web #42` with its live state, checks `pass build` / `fail test` (aggregate `fail`).
  *Logs* on the failed one ran `gh api …/jobs/7` and `gh run view 5 -R o/web --log-failed` and showed
  201 lines starting `failed step: Run tests`. *Create pull requests* ran `gh pr create -R o/api
  --base main --head feat/review-ui --title "Review ui" --body ""`, saved `api #43` in
  `.workspace.yaml` and listed it. *Re-run* called `gh api -X POST
  repos/o/web/actions/runs/5/rerun-failed-jobs` (the stub refuses it, and the page showed the error).
- Send to PR was tested on its logic only (unit tests of the line checks): posting needs GitHub's
  GraphQL and contents APIs, which the stub doesn't answer.

gpui-component limits met: the `+` button and line selection are pointer gestures inside the `Diff`
element, which the self-test can't target (no probes inside it), so the composer was opened with the
command that shares its path; notes on folded lines show once the fold is expanded; the diff view is
read-only, so there is no editing in the diff.

## Build / Run / Debug

`src/run/` and `src/debug/`, ported from `RunWidget.tsx`, `BottomPanel.tsx`, `DebugView.tsx`,
`runStore.ts`, `problems.ts` and `src/lib/dap/*`, on `orbit_core::run` and `orbit_core::dap`.

- **Run widget** (titlebar slot): the configurations of every repo of the workspace
  (`.orbit/run.json` + `run.local.json`), the CMake preset, Build `Ctrl+F9`, Run `Shift+F10`, Debug
  `Shift+F9`, Stop `Ctrl+F2` (cancels the build, Ctrl+C to the program, or ends the debug session).
  The choice is remembered per workspace in `<config dir>/native-run.json`, with the breakpoints.
- **Bottom panel** (`layout.panel_height`, resizable, saved): Build output (virtualized, follows the
  end unless scrolled up; compiler lines are links), Run (the program on `orbit_core::pty` with the
  configuration's cwd, args and env), Problems (grouped by file, errors first), Debug. A build opens
  it; a failed build with problems switches to Problems.
- **Problems**: `problems.rs` is `problems.ts` with its tests. Problems under the repo's real root
  are mapped to the workspace path and set on open editors as diagnostics with source `build`,
  next to clangd's (`EditorTab::set_build_diagnostics`), so they count in the status bar and `F8`.
- **Debugger**: gdb `-i dap` (or lldb-dap, as `dap::adapter` picks). The launch is `session.ts`'s:
  initialize → launch (not awaited) → initialized → setBreakpoints → configurationDone → launch's
  answer. Paths are rebased between the workspace path and the real repo path both ways. Stops show
  threads, the call stack (the innermost repo frame is opened and highlighted), lazy variables and a
  console; Enter in its input evaluates (gdb gets `context: "watch"`: its `"repl"` runs CLI
  commands). `F9`/`F8`/`F7`/`Shift+F8` resume/step while paused; in the editor `F8`/`Shift+F8` fall
  back to next/previous problem otherwise, and `F7` to the review's next change.
- **Breakpoints in the editor.** gpui-component's editor has no glyph margin and no gutter click
  hook. The least invasive route: every editor gets an 8 px `GutterDecoration` strip (set up from
  `observe_new::<EditorTab>`, no editor code), breakpoints are red bars there, the paused line is
  an amber bar plus a filled `RangeDecoration`, and one mouse-down listener on the editor's wrapper
  (`editor.rs`) toggles the caret's line when the click lands in the strip (the editor has moved the
  caret by then). `Ctrl+F8` toggles at the caret.

Verified with self-tests (`ORBIT_CONFIG_DIR` / `ORBIT_WORKSPACE_ROOT` in a scratch folder) on a
tiny CMake project reached through a symlinked workspace repo:

- Build + Run: configure with the `${auto}` preset, build, run; the terminal shows
  `argc=3 greeting=hi-from-orbit sum=10 p=(3,4)` (args and env from run.json), then exits.
- A broken target: `Build Failed, 1 errors, 1 warnings`, the panel on Problems, `problem:0` opens
  `broken.cpp` at 3:12 and the editor has 3 diagnostics (clangd's error, the build's error and
  warning).
- Debug: breakpoints from gutter clicks and `Ctrl+F8` (persisted), `Shift+F9` stops at line 23 with
  `gutter main.cpp: breakpoints [14, 23], paused mark [23], paused line [23]`; frames, `Locals`
  (`numbers = std::vector of length 4`, `greeting = "hi-from-orbit"`), expanding `p` gives `x = 3`,
  `y = 4`; `F7` into `sum`, `Shift+F8` out, `F8` over (stopping at the breakpoint in `sum` when one
  is there); `p.x + numbers.size()` evaluates to `7`; `Ctrl+F2` ends the session; the frames
  report real paths and come back as the workspace paths.
- aether-engine: the widget resolves `game` with `${auto}` → `linux-gcc-relwithdebinfo` and lists
  its 8 presets; Build through the app ran `cmake --build …/build/linux-gcc-relwithdebinfo --target
  game` (`Build Ok`, 0.8 s, up to date); cancelling a running build gave `Build Cancelled`. Debugging
  the game was not completed: another build of the same tree was running at the same time and the
  link failed (`ld terminated with signal 7`), and the debug session correctly did not start.

## Stage: git and workspace flows

A new tab kind, `TabKind::Page` (`src/workspace_ui/`), holds three pages; forms and git flows open
as a modal inside the page tab (an occluding scrim), so switching tabs keeps them.

- **Dashboard** (`DashboardPage.tsx`): a card per workspace (branch, repos coloured by state with
  `↑n`, "Up to date" or "n with changes · n to push · n behind", tracker card id), onboarding when
  there are no repos or workspaces, and the configured repositories with their brief, each opening
  its repo page. Sidebar *Dashboard*, command `workspace.dashboard`.
- **Workspace page** (`WorkspaceDetailPage.tsx`): header tools (Commit, Push, Rebase, Merge,
  Changes and PRs review tabs, + Repos, Refresh, Remove), the delivery flow (Changes → Push → Pull
  requests → Checks with the React page's rules: dropped repos whose PRs were closed, integrated
  repos needing no push, "Create" when pushed without PRs, "Squash and merge" when checks allow, the
  next step highlighted), the branch drift banner, repos with badges (off branch, operation in
  progress, dirty/clean, merged, `↑ahead`, `↓behind` with *Pull* when nothing is ahead, *linked
  clone*), and the PRs with their check aggregate. Local status first, then a fetch with PRs and
  checks; again every 60 s and on Refresh. Sidebar *Overview*, command `workspace.overview`.
- **Repo page** (`RepoPage.tsx`), on the base clone (scope `@repo`): branch, upstream and
  ahead/behind, Fetch, Pull / Pull --rebase, Push or Publish (the push flow), Commit… (the commit
  flow), Stash, New workspace… (preselects the repo); a paused operation's banner (Resolve with
  AI, Continue, Abort); branches (Switch, asking Stash and switch / Bring changes when dirty;
  Delete, confirmed), Create branch; history (Undo on the last commit, Revert, confirmed); stashes
  (Restore, Drop); worktrees with Prune. Command `repo.open` (a menu of repos) or the dashboard.
- **Git flows** (`src/git/`), as the React modals: *Commit* (per repo: files with checkboxes, all in
  by default, a repo checkbox for all/none, `✦` and *Draft all with AI* through
  `agent::commit_message` for the selected files, the message box, ↺ / *Discard all* behind a
  confirm dialog (new files to the Trash), a file name opens its diff tab, commits in parallel, a
  clean repo drops out); *Push* (nothing runs before the click; up to date / rejected / failed;
  *Force push* with lease on a rejected row, *Push anyway* for a dropped repo; "Updated the open pull
  requests: site #7"; *Create pull requests →* opens the PRs review tab, which creates them);
  *Rebase* onto `origin/<base>` (conflicts pause a row with *Resolve with AI*, which continues the
  rebase or aborts on failure, and *Abort*); *Squash and merge* (one PR at a time, stopping at the
  first failure; fetches afterwards). Commands `git.commit`, `git.push`, `git.rebase`, `git.merge`,
  `git.syncBranches`; run from anywhere they open the selected workspace's page and wait for its
  first read.
- **Branch drift** (`BranchDriftBanner.tsx`): repos not on the workspace's branch (a linked repo's
  shared clone switched by another workspace), who holds them, a paused operation that blocks the
  switch; *Switch back*, then for dirty repos *Stash and switch* (re-applied when that branch
  returns); auto-switch repos go back on their own once per page. Ahead/behind against the
  upstream is on each repo row and the dashboard.
- **New workspace** (`WorkspaceCreateModal.tsx`): Manual (name, branch defaulting to
  `feat/<slug>`, base from a menu of the first selected repo's branches, groups as chips, repos with
  a *linked* note for `worktree: false` ones) or Existing branch (`create_from_branch`, problems in an
  Info toast). **Add repositories** (`AddReposModal.tsx`). Both close at once and report in toasts;
  orbit-core sets up worktrees or, for linked repos, checks the branch out in the base clone and
  links it. The app's workspace list and file tree follow (`workspace_ui::reload_workspaces`).
  **Remove workspace** asks, and asks again before discarding uncommitted changes.

**Core move.** Inline logic of the git/workspace commands moved to orbit-core: `git::file_diff`
(`git_file_diff`), `git::commit_file_diff` (`git_commit_diff`), `git::push_current` and
`prs::push` (`ws_push`), `prs::merge` / `prs::merge_remote` (`ws_pr_merge`, `pr_merge_remote`),
`repo::fetch` (`refresh_repo`), `workspace::base_branches` (`list_base_branches`), and
`ws_address_review` now uses `prs::owner_repo`. The commands keep their arguments and results. A core
test (`creates_adds_linked_repos_commits_and_pushes`) creates a workspace from a local bare origin,
adds a linked repo, diffs, commits and pushes, and fetches.

Self-test probes: sidebar `side-dashboard`, `side-workspace`; dashboard `dash-new`,
`dash-refresh`, `dash-ws:<name>`, `dash-repo:<name>`; workspace page `ws-commit`, `ws-push`,
`ws-rebase`, `ws-merge`, `ws-changes`, `ws-prs`, `ws-add-repos`, `ws-refresh`, `ws-remove`,
`ws-done-remove`, `flow-changes|push|prs|checks`, `ws-repo:<repo>`, `ws-pull:<repo>`,
`ws-repo-page:<repo>`, `ws-pr:<repo>#<n>`, `drift-switch`, `drift-stash`, `drift-cancel`; modals
`modal-cancel`, `commit-open:<repo>`, `commit-repo:<repo>`, `commit-file:<repo>/<path>`,
`commit-diff:<repo>/<path>`, `commit-discard:<repo>/<path>`, `commit-discard-all:<repo>`,
`commit-draft:<repo>`, `commit-draft-all`, `commit-msg:<repo>` (the message box), `commit-submit`,
`push-submit`, `push-force:<repo>`, `push-anyway:<repo>`, `push-create-prs`, `rebase-submit`,
`rebase-ai:<repo>`, `rebase-abort:<repo>`, `merge-submit`, `create-source-manual|branch`,
`create-name`, `create-branch`, `create-base`, `create-group:<g>`, `create-repo:<repo>`,
`create-submit`, `add-repo:<repo>`, `add-repos-submit`; repo page `repo-fetch`, `repo-pull`,
`repo-pull-rebase`, `repo-push`, `repo-commit`, `repo-stash`, `repo-new-ws`, `repo-op-ai|continue|abort`,
`repo-switch:<branch>`, `repo-delete:<branch>`, `repo-branch-name`, `repo-create-branch`, `repo-undo`,
`repo-revert:<sha>`, `repo-stash-pop:<ix>`, `repo-stash-drop:<ix>`, `repo-prune`. The dump lists each
page (`--- page …`: statuses, PRs, checks, the derived flow, drift; the dashboard's cards and repos;
the repo overview) and the open modal's rows.

Verified by self-tests on a scratch root (`ORBIT_WORKSPACE_ROOT`, `ORBIT_CONFIG_DIR`,
`XDG_DATA_HOME` for the Trash, `GIT_ALLOW_PROTOCOL=file` so nothing reaches GitHub) with three repos
cloned from local bare remotes (`api` linked, `worktree: false`), a stub `gh` and a stub `claude`
(drafts `feat: stub drafted message`, "resolves" conflicts by taking the replayed side) first on
`PATH`:

- New workspace `flows` from the form (web + api): `feat/flows`, web a worktree, api's clone switched
  in place (`linked`). Existing branch `topic` (on web's origin only) made workspace `topic` with web
  alone. The base menu offered the first repo's branches; the `backend` group chip selected api + web.
- Commit: with `main.txt` modified and `new.txt`, `notes.txt` new, `git.commit` listed both repos;
  `new.txt` unticked, api's message drafted by the stub, web's typed; *Commit 2 repos* made two
  commits, `new.txt` stayed untracked (checked with `git log --stat`). ↺ on it, confirmed, moved it
  to the scratch Trash and the repo dropped out of the modal.
- Push: both pushed with upstreams `origin/feat/flows` (bare remotes checked). After a conflicting
  commit on `origin/main`, Rebase: api clean, web `Conflicts(["main.txt"])` → *Resolve with AI* →
  `Done`, "Rebased onto origin/main"; Push then gave `Rejected: Branch diverged from origin` and
  *Force push* `Force-pushed`. (With the stub at first not resolving, the row failed and the rebase
  was aborted, leaving the branch as before.)
- Drift: workspace `other` with api switched the shared clone; `flows` showed `api on feat/other …
  OFF-BRANCH`; *Switch* left it (uncommitted `wip.txt`), *Stash and switch* gave "Switched: api
  (changes stashed)".
- Add repositories: `site` added on `feat/flows` (a new status row). With site's origin set to a
  GitHub-looking URL (pushes rewritten to the bare remote), the stub's PR `site #7` showed OPEN with
  checks `pass`; Push said `open prs: {"site": 7}`; *Squash and merge* ran `gh pr view … headRefName`,
  `gh pr merge 7 -R o/site --squash` and the branch delete, then showed `site #7 MERGED`.
- Pull: a teammate's push to `origin/feat/flows` gave web `behind 1`; *↓ 1 Pull* fast-forwarded.
- Repo page of web: Pull (behind 1), Create branch `topic`, Commit… (`@web` scope), Publish (push
  flow, upstream `origin/topic`), Switch to main, Delete `topic` (confirmed).
- Remove workspace `other`: confirmed, removed, and its tab closed.

Not done: tracker sources (Shortcut/Linear) in New workspace, the
commit modal's side-by-side diff (a file opens its diff tab instead), compare / cherry-pick on the
repo page, Ctrl+Enter to commit, Race, Plan, the dashboard's context menu. Ralph, artifacts and
usage are their own tabs (settings stage), and "Open in editor" / "Open folder" are commands.

## Stage: settings, Ralph, integrations, usage, artifacts, updates, crash log

`src/pages/`: tool tabs (`TabKind::Tool`, a `PageTab` wrapping any `PageView`) opened from the
sidebar (*WORKSPACE*: Ralph, AI usage, CI artifacts; *ORBIT*: Integrations, Settings) or the palette.

- **Settings** (`Ctrl+,`, `app.settings`; `settings.languages|health|ai`): its own section list
  with the filter (`Enter` opens the first match). Repositories (folders, add manually / local clone
  / from GitHub, edit, clone, open folder, remove with confirm; the modals are inline cards; per-OS
  build commands are kept but edited in the Tauri app only), Groups (create, edit members, remove),
  AI (agent, model and fast model from a menu of the agent's models, Remote Control, Test agent),
  Languages (state, custom command on Enter/blur, enabled, errors only, hidden codes, running
  servers with log and stop), Appearance (Orbit Dark only; the others listed as Tauri-only),
  Health (`health::run`, git identity included), Updates.
  All of it reads and writes `config.yaml` through `orbit_core::settings` / `lsp`, so the two apps
  share settings.
- **Integrations**: Shortcut, Linear, Figma; masked token, Show/Hide, Connect (validated by the
  provider, then saved), Disconnect.
- **Ralph** (`ralph.open`): target *Plan* (PLAN.md, one agent per task) or a repo's PRD
  (`scripts/ralph/prd.json`); iterations 3/5/10/until done, push toggle, Start / Pause after
  iteration / Stop; the stories with the current one highlighted, the run's counters, the live feed
  (`ralph-event`s fanned out from one `Events` sink to every open Ralph tab) and past runs. Writing
  and editing a PRD (interview, generation, editor) and the custom iteration prompt are not ported.
- **AI usage** (`workspace.usage`): per model tokens and cost, sessions on click, the Claude 5h/7d
  windows; every 30 s. **CI artifacts** (`workspace.artifacts`): latest run per workflow of each
  repo's branch; Download extracts into `~/Downloads/Orbit/<workspace>/` and opens the folder.
- **Updates**: `orbit_core::updates::check` reads the updater's `latest.json` (curl) and compares it
  with `package.json`'s version; at start and hourly, a toast once per new version, and Settings →
  Updates (*Check for updates*, *Open release page*). No install: that is the packaging stage.
- **Crash log**: a panic hook appends the message, place, version and backtrace to
  `<config dir>/native-crash.log` and leaves a marker; the next start toasts "Orbit closed after a
  crash" with the log's path. `ORBIT_TEST_PANIC=<ms>` panics on purpose.
- **Open in editor / folder**: `workspace.openInEditor` (a menu: whole workspace or one repo, VS
  Code or Zed) and `workspace.revealFolder`.

**Core move.** The config edits of `commands.rs` (`add_service`, `update_service(_settings)`,
`remove_service`, `clone_service`, `service_clone_status`, `local_repo_origin`, folders, groups,
`set_ai_settings`, `open_in_editor`) are `orbit_core::settings`; `workspace_ai_usage` is
`usage::for_scope`, `integration_connect` is `integrations::connect`, `log_render_crash` is
`crash::append`. New: `updates`. `RalphState`'s fields are public and `Limits`/`StopReason`
re-exported. Test hooks: integration tokens follow `ORBIT_CONFIG_DIR` when set, usage reads
`CLAUDE_CONFIG_DIR` (as Claude Code does), `ORBIT_DOWNLOADS_DIR` and `ORBIT_UPDATE_URL` redirect
downloads and the release check. The Tauri commands keep their names, arguments and results.

**Verified** by self-tests with `ORBIT_CONFIG_DIR`, `ORBIT_WORKSPACE_ROOT`, `CLAUDE_CONFIG_DIR`,
`ORBIT_DOWNLOADS_DIR` and `ORBIT_UPDATE_URL` in a scratch folder (a workspace with one repo cloned
from a local bare origin) and stubs first on `PATH` for `claude`, `gh` and `curl`. No paid agent,
GitHub or tracker was called.

- Repositories: Add manually (`api`, build `cargo build`, branch in base clone) wrote it to
  config.yaml; Add local clone read the checkout's origin; Save folders set `reposDir`; removing a
  repo in a group was refused with a toast; Groups: create `core`, add `web`, remove.
- AI: model and fast model picked from the menu, Remote Control on, agent → OpenCode (model reset to
  `aihub/aihub/best`); Test agent with the stub shows the failure and its output.
- Languages: cpp disabled / enabled, errors only on, rust custom command `rust-analyzer` (Enter)
  saved in `languageServers`. Health listed every check, git identity included.
- Integrations: a bad Shortcut token shows the provider's "Unauthorized"; `good-token` connected as
  "Stub User" (credentials in the scratch config dir); Disconnect.
- Updates: a `9.9.0` manifest gave the toast at start and "Orbit 9.9.0 is available" in Settings.
- Usage: a Claude log in the scratch `CLAUDE_CONFIG_DIR` → `claude-sonnet-5-5 2.0k in / 4.0k out /
  50.0k cache · 1 sessions`. Artifacts: the stub's `game-linux` (5.0 MB) listed and downloaded
  (`gh run download 5 -R o/web -n game-linux -D …`).
- Ralph with a **stub `claude`** (it ticks the next story in prd.json and commits): target `web`,
  3 iterations, Start → `US-001` then `US-002` pass, 2 commits, `■ stopped: complete after 2
  iterations` and a toast; the plan target lists PLAN.md's tasks (1/2 done). *Until done* + Stop
  ended the run with reason `stopped`.
- Crash: `ORBIT_TEST_PANIC=1500` wrote `native-crash.log` (message, place, version, backtrace) and
  the marker; the next start showed the toast and removed the marker.

Self-test probes: `nav-settings`, `nav-integrations`, `nav-ralph`, `nav-usage`, `nav-artifacts`,
`settings-filter`, `settings-nav:<section>`, `folders-repos|workspaces|save`, `repo-add`,
`repo-add-local`, `repo-add-github`, `repo:<name>:clone|reveal|edit|remove`, `form-name|url|path|build|
output|shared|worktree|branch-only|auto-switch|clone-now|save|cancel`, `local-path`, `local-add`,
`github-filter`, `github:<owner/repo>`, `group-new`, `group-create`, `group:<g>:edit|remove`,
`group-member:<repo>`, `group-save`, `ai-agent:<label>`, `ai-model`, `ai-fast-model`, `ai-remote:on|off`,
`ai-test`, `lang:<id>:command|enabled|errors-only`, `lsp:<id>:log|stop`, `health-recheck`,
`updates-check`, `integration:<kind>:token|show|connect|disconnect`, `ralph-target:<plan|repo>`,
`ralph-iterations:<n|until-done>`, `ralph-push`, `ralph-start|pause|stop`, `usage:<model>`,
`artifact:<repo>:<name>`. Inputs inside the pages fill through `click:<probe>` then `type:`.

## Stage: shell parity (themes, sidebar, frame, dock)

The frame now follows the Tauri app screen for screen (`src/App.tsx`, `src/App.css`, reference
captures of the original at 1920×977 in Nord).

- **Themes** (`src/theme.rs`): all seven of `src/lib/theme.ts`. Colours are tokens:
  `c(theme::PANEL)` resolves against the active theme when drawn, so call sites stay
  `c(theme::X)` and switch live. Tokens: the CSS variables (`BG`, `PANEL`, `PANEL_RAISED`,
  `PANEL_HOVER`, `BORDER`, `BORDER_STRONG`, `TEXT`, `TEXT_DIM`, `TEXT_FAINT`, `ACCENT`,
  `ACCENT_HOVER`, `ON_ACCENT`, `DANGER`, `OK`, `WARN`, `INFO`, `PURPLE`), the tints App.css derives
  (`ACCENT_SOFT`, `DANGER_SOFT`, `NAV_ACTIVE` = info 40% over text, `OK_SOFT`, `INFO_SOFT`,
  `ACCENT_STRONG_SOFT`, `TEXT_SOFT`), `SYN_*` syntax colours and `ANSI[16]`. A literal
  `0xrrggbb` still works in `c()`. `theme::alpha(tok, a)`, `theme::hex(tok)`, `theme::mix`.
  API: `theme::list()` (id, label, dark), `theme::current()`, `theme::pref()` (id or "system"),
  `theme::set(name, cx)` (id, label or "system"; redraws every window and re-themes
  gpui-component, its inputs, menus, scrollbars and editor highlighting). The choice is saved
  in `native-state.json` (`theme`). With none saved, the Tauri app's `orbit.theme` is read
  (read-only) from its WebKitGTK localStorage, else Nord. Palette: *Select Theme…*;
  self-test: `--theme <id>`. Fonts: Noto Sans (system sans) and Noto Sans Mono (system monospace).
- **Sidebar** (`src/sidebar.rs`, the only sidebar): brand; WORKSPACES with Dashboard, Code
  Review and each workspace (accent bar when its page is open, chevron to fold its sessions,
  saved as `ws_collapsed`; right click: Open workspace, New terminal, Open in editor, Open
  folder) with its sessions (status indicator, unread dot, minimized ones italic; editors as
  `repo/file`; right click: Open / Minimize / End session, or Open / Close) and RECENT
  SESSIONS (click resumes, × forgets); SESSIONS "+" (the New Session menu: OPEN IN a scope,
  then Terminal / Claude Code / Open Code / OMP) and folder scopes; REPOS with ↑/↓, dirty and
  "not cloned" badges (briefs re-read every 30 s) opening the repo page; GENERAL; the
  repos/groups footer. Dragged under 110 px it is the 64 px icon rail; the titlebar toggle hides it.
- **Nav pages**: Dashboard, Code Review, a repo page, Settings and Integrations are the React
  app's `navPage`: tabs kept off the strip (`Tab::is_nav`), one at a time; the app starts on
  the Dashboard. The strip only shows sessions/views then files (`is_file` = editors), hides
  when empty and on Settings.
- **Minimized sessions**: an agent tab's × (and middle click, Close Other/All Tabs) takes it
  off the strip; it keeps running and the sidebar lists it; showing it brings the tab back.
- **Focus**: activating a tab selects its workspace (`focus_workspace`), which drives the
  dock, the run widget and the bottom bar; nav pages have none.
- **Titlebar**: Orbit, sidebar toggle, back/forward, `titlebar_items` (run widget), the inbox
  bell (badge, popover with Mark all read / Clear) and the dock toggle.
- **Bottom bar** (`footer.usage-bar`, only with a workspace in focus): model chips with
  tokens, Claude 5h/Week limit bars, "N sessions · ws".
- **Dock**: Files / Find / Git icons. Files: the repo select and the tree of that repo
  (`FileTree::set_focus_root`), listing every entry as `list_files` does. Find opens Find in
  Files. Git: the repo's changes (open a diff; *Review all*) and commits.
- Self-test: `--window-size 1920x977`, `--theme`; probes `side-dashboard`, `review-prs`,
  `ws:<name>`, `chevron:<name>`, `session:<name>`, `recent:<name>`, `recent-forget:<name>`,
  `repo:<name>`, `folder:<label>`, `sessions-add`, `nsm:<row>`, `nav-settings`,
  `nav-integrations`, `tb-inbox`, `inbox-read`, `inbox-clear`, `dock-files|find|git`,
  `dock-repo`, `git-refresh`, `git-review-all`, `git-change:<name>`; `tab:<n>` counts strip
  positions. The dump has a `--- shell:` line (theme, focus, dock, minimized).

## Stage: packaging

### GPU backend

gpui's wgpu renderer (`gpui-pre-wgpu`, Linux and FreeBSD only) asked wgpu for Vulkan *and* GL,
which loads every vendor's Vulkan driver and every EGL stack. It now asks for **Vulkan only**, and
for **GL only when no hardware Vulkan adapter exists** (probed once at start). `WGPU_BACKEND`
(wgpu's own variable, e.g. `WGPU_BACKEND=gl`) forces a backend. gpui has no supported switch for
this, so `native/vendor/gpui-pre-wgpu` is the crates.io package with one function added to
`wgpu_context.rs`, wired in by `[patch.crates-io]`; `vendor/PATCHES.md` says what changed and how
to re-apply it on an upgrade. Windows (DirectX) and macOS (Metal) don't use the crate.

Idle (`--workspace add-items-skills`), PSS + SwapPss, max of 3 runs, MB:

| Renderer | Idle |
|---|---:|
| Before: Vulkan + GL | 159 |
| **Vulkan only (default now)** | **122** |
| Vulkan only, Intel ICD only (`VK_DRIVER_FILES=…/intel_icd.json`) | 62 |
| GL forced (`WGPU_BACKEND=gl`) | 140 |

The rest of the gap to the PoC is NVIDIA's Vulkan driver: the loader opens every ICD, so
`libnvidia-gpucomp`, `-rtcore` and `-glcore` (~46 MB) are mapped although the window renders on
the Intel GPU. A launcher can drop them with `VK_DRIVER_FILES` or `VK_LOADER_DRIVERS_DISABLE`;
doing it automatically means knowing the compositor's GPU before the loader runs, which was left
out (it would break NVIDIA-only and PRIME-offload setups if guessed wrong).

Checked: the GL path renders (forced, and naturally with `VK_DRIVER_FILES=/nonexistent.json`:
Mesa EGL and `libgallium` mapped, no Vulkan driver, first frame after 110 ms); the stage-1 gate
still passes on Vulkan (F12 on `Initialize`, 39:12 of `Game.h`, lands on `Game.cpp` 44:10).

### Tauri vs native

Same machine as *Measurements* (31 GB RAM, with 15 GB in swap during the runs). Both apps on a
scratch copy of the config (`ORBIT_CONFIG_DIR`), workspace `add-items-skills`, measured 15 s after
start. PSS + SwapPss per process tree, max of 3 runs, MB. "App" is the UI process plus, for Tauri,
`WebKitWebProcess` and `WebKitNetworkProcess`; terminal programs and clangd are apart.

| Scenario | Tauri | Native | Native / Tauri |
|---|---:|---:|---:|
| Idle, workspace selected | 457 | 124 | 27% |
| + tree expanded, `Game.h` and `Game.cpp` open | 581 | 138 | 24% |
| + 3 terminals running `top` | 582 (+6 `top`) | 140 (+9 shells) | 24% |
| clangd (same flags in both) | ~1,510 | 1,440–2,660 | |
| Start → first paint | 615–965 ms | 190–260 ms | |

- Tauri: a release build of this branch (`pnpm tauri build --no-bundle`) on the system WebKitGTK
  2.54.2. Its tabs (the workspace, the two editors, three `top` terminals) were seeded in a
  scratch copy of its localStorage (`XDG_DATA_HOME`), and its file tree was expanded through
  WebKit's remote inspector (`WEBKIT_INSPECTOR_HTTP_SERVER`, JS clicks on the rows: no OS input).
  First paint is the page's `first-contentful-paint` measured from the launch.
- Tree: the React dock shows one repo, so aether-engine was opened two levels below its root
  (1,261 rows); the native tree shows the four repos three levels from the workspace (1,452 rows),
  the same depth in aether-engine. Native's first frame is its own `first frame after` line; under
  this memory pressure the first run of a series sometimes took 2–3 s.
- clangd varies with how far `--background-index` has got; it costs the same in either app.

### Packaging (Linux)

- `packaging/linux/orbit-native.desktop` (app id `orbit-native`, so Wayland matches the window to
  it) and the Tauri app's icons from `src-tauri/icons` (32–512 px, nothing duplicated).
- `scripts/package-linux.sh [--no-build]` writes `dist/orbit-native-<version>-linux-<arch>.tar.gz`
  (`bin/`, `share/`, `install.sh`) and, when `appimagetool` is found (`$APPIMAGETOOL` or `PATH`),
  `dist/orbit-native-<version>-<arch>.AppImage`. The version is `package.json`'s.
  `install.sh` installs into `~/.local` (or `PREFIX`), points the entry's `Exec` at the binary and
  refreshes the desktop and icon caches; `--uninstall` removes it. Checked: the tarball, the
  `.desktop` (`desktop-file-validate`), install and uninstall into a scratch prefix. The AppImage
  path was not run here (running a downloaded `appimagetool` was not allowed); CI builds it.
- The binary links only glibc, libgcc, `libxcb`/`libxkbcommon` (Vulkan, Wayland and fonts are
  loaded at run time), so the AppImage needs no bundled libraries. As with the Tauri release, build
  it on the oldest Ubuntu to support (glibc).

**CI** (`.github/workflows/native.yml`, on PRs, pushes to main and by hand; `continue-on-error`,
so it never blocks a PR; `release.yml` is untouched): Linux on `ubuntu-latest` runs `cargo
build`, `clippy -D warnings`, `test` and the packaging, and uploads the tarball and AppImage for a
week; Windows and macOS run `cargo check --all-targets` (macOS with
`gpui_platform/runtime_shaders`, so the runner needs no Metal toolchain). Caches use
`swatinem/rust-cache` with `native/target`, one key per OS.

**Windows and macOS.** Checked here by cross-compiling (`x86_64-pc-windows-msvc`,
`aarch64-apple-darwin`; C dependencies stubbed, Metal shaders skipped, so type-checking only):
both pass `clippy --all-targets -D warnings`. One fix was needed: the terminal's primary
selection (`write_to_primary` / `read_from_primary`) exists on Linux only, so copy-on-select and
middle-click paste are Linux-only now. Nothing has been *run* on Windows or macOS yet.

**Updater (plan, not built).** `orbit_core::updates::check` already compares `latest.json` with
the running version and Settings → Updates offers the release page. Installing:

1. The release workflow adds native artifacts (Linux tarball and AppImage; later a Windows zip and
   a macOS `.app`) and a `latest-native.json` with per-platform URLs, signed with the existing
   minisign key (`TAURI_SIGNING_PRIVATE_KEY`), so the public key in `tauri.conf.json` verifies it.
2. The app downloads to a cache folder, verifies the signature (`minisign-verify`), then swaps
   itself: the AppImage replaces `$APPIMAGE` by rename, a tarball install replaces
   `bin/orbit-native` the same way (Linux keeps the running inode), Windows renames the running
   exe aside first (the `self-replace` crate does both); then it offers *Restart*.
3. Until then, updating is installing the new tarball or AppImage.

### Fixes in this stage

Self-tests on a scratch workspace (`ORBIT_CONFIG_DIR`, `ORBIT_WORKSPACE_ROOT`, a repo with a local
bare origin) unless noted:

- **Rename follows open editors.** Renaming `main.rs` → `app.rs` from the tree's menu turned the
  dirty tab into `api/app.rs` with the edit and the cursor (2:1) kept; renaming the folder `src` →
  `lib` moved both open editors, and `Ctrl+S` then wrote `lib/util.rs`. The tree emits
  `TreeRequest::Renamed`, and `OrbitApp::file_renamed` reopens each affected editor in its slot.
  Language servers now get `didClose` when an editor goes (closed tab or renamed file).
- **Diagnostics filter.** The editor applies Settings → Languages' *Errors only* and hidden codes
  to the server's diagnostics (build problems are never filtered), as the React editor does. On a
  file with clangd's `unused-includes` warning and an error: `⨯ 1 ⚠ 1` unfiltered, `⨯ 1 ⚠ 0` with
  the code hidden or *Errors only* in config, and toggling *Errors only* in Settings while the
  file is open changed the open editor from `⚠ 1` to `⚠ 0`. Unit test for the rule.
- **Tracker cards in New workspace.** Shortcut and Linear sources (marked "not connected", and
  refused with a hint, without a token), a search field (300 ms debounce, stale answers dropped)
  and the card list; picking a card fills the name (slug of the title) and branch (Linear's
  suggested branch, else `feat/<id>`), and the workspace is created with the card saved in
  `.workspace.yaml`. With a stub `curl` first on `PATH` and a scratch Shortcut token: Linear refused,
  Shortcut listed `sc-101`, `sc-102`; typing `login` made one request and listed `sc-102`;
  picking it and `api` created `fix-login-timeout` on `feat/sc-102` with `card: {kind: shortcut,
  id: sc-102, …}`. Probes: `create-source-shortcut|linear`, `create-card-search`,
  `create-card:<id>`.

## Gap list: before the native app can replace the Tauri app

Prioritized. "Must-have" means the owner's daily flow would miss it.

**Must-have**

1. **Run it on Windows and macOS.** It compiles for both, but GPUI's DirectX and Metal renderers,
   ConPTY terminals, paths and the agent hooks have never run. The team builds on Windows.
2. **Release and update.** Native artifacts in `release.yml` (with the glibc floor), the signed
   manifest and self-replace (plan above). Today an update is a manual reinstall.
3. **Agent workflow gaps.** OS notifications for sessions that need input; the Plan modal and
   PLAN.md progress (the owner's sessions start from a plan prompt), initial prompt injection;
   minimized agent tabs and session rename; the live agent feed.
4. **GitHub PR review.** Review threads and submitting a review, the Code Review / feature pages,
   PR checkout and *address review*; commit review. The local REVIEW.md flow is done.
5. **Themes.** Only Orbit Dark; the owner uses Nord. The six other themes are token tables
   (`src/lib/theme.ts`) and gpui-component takes a theme JSON, so this is mostly data.
6. **Find in the current file** (`Ctrl+F`): gpui-component's editor has search; it is unbound and
   untested here.
7. **Markdown preview** (PLAN.md, REVIEW.md, READMEs): gpui-component has a Markdown view.

**Nice-to-have**

- Workspace page: investigate/fix failing checks with AI, Race, review feedback; repo page compare
  and cherry-pick; the commit modal's inline diff; the multi-repo build modal.
- Ralph PRD writing and editing, run overrides, the custom prompt; per-OS build commands in the
  repo form.
- Editor: semantic tokens, document highlights, snippets, the "hide this diagnostic" quick fix,
  jumps inside a file in back/forward history; breakpoint conditions.
- Files: drag-and-drop move, "Add Files…", dropping a path into a terminal; the dock's Git panel;
  refresh on window focus; history entries of a renamed file still point at the old path.
- Folder scopes (`dir:<path>`) and the Folders card; kitty graphics; activity bar and brand icons.
- Choosing the compositor's GPU automatically (−60 MB on hybrid laptops, see *GPU backend*).

## Next stages

Ordered by dependency, with rough sizes (one focused agent-day ≈ 1 unit).

1. ~~**orbit-core extraction**~~: done (stage 2), except `pty` and `commands.rs`'s inline logic.
2. **Shell parity** (2): resizable panes, the activity bar and icons (Lucide is bundled), the
   status bar, toasts and notifications, the custom titlebar, back/forward history, persisted tab
   and session state.
3. ~~**Sessions**~~: mostly done (stage 3, above); left: minimized agent tabs, OS notifications,
   session rename, the "Open in" folder choice of the new-session menu, kitty graphics, the live
   agent feed. Originally: agent terminals (claude/opencode) with codenames, status from hooks and the
   spinner parser, resume, the new-session menu, the inbox. Plus terminal flow control and
   scrollback restore through `orbit-core::pty`.
4. **Editor completeness** (3): completion, signature help, code actions, references, document
   symbols, formatting and semantic tokens (providers exist in gpui-component), go to line, F8 next
   problem, the clangd setup banner, Markdown preview, and the Languages settings section.
5. ~~**File tree completeness**~~: done (files and search stage) except move/drag and "Add Files…";
   originally: create/rename/delete/move/drag, reveal, copy path, fs-watch
   refresh, file icons, and a tree-structured model (the flat list does an O(n) path search per
   folder load).
6. **Review** (4–5): mostly done (stage above: changes review, REVIEW.md notes, send-to-PR, F7,
   PRs, checks and logs); left: commit review, GitHub PR review with threads, address review.
7. ~~**Git and workspace flows**~~: mostly done (see *Stage: git and workspace flows*); left:
   compare and cherry-pick on the repo page, the commit modal's inline diff, the workspace page's
   Plan and Race. (Tracker cards: packaging stage.)
8. ~~**Run/Build/Debug**~~: done (see *Build / Run / Debug*); left: the multi-repo build modal,
   breakpoint conditions, a round breakpoint glyph (needs an editor gutter hook upstream).
9. ~~**Settings, Ralph, integrations, usage, artifacts, updates**~~: mostly done (see *Stage:
   settings, …*); left: writing/editing a Ralph PRD and its prompt, the themes, per-OS build
   commands in the repo form, installing updates.
10. ~~**Packaging**~~: done (see *Stage: packaging*): the Vulkan-only renderer, `.desktop`, icons,
    tarball and AppImage, native CI; the updater is a plan. What is left before the native app can
    replace the Tauri app is in *Gap list*.

## Feature parity checklist

Status of everything the React app does: **done**, **partial** (with what's missing) or **—**
(not started). Files are the React sources.

### App shell & layout
- [ ] partial: main area with tabs, sidebar and focused workspace (`src/App.tsx`). Tab kinds: terminal, editor, diff, review (changes, pull requests).
- [ ] partial: tab kinds: changes (review stage), dashboard, workspace and repo pages (git flows stage) are done; reviews, settings, integrations, commit, pr, feature, ralph are not (`src/App.tsx`)
- [x] done: custom titlebar (client-side decorations; gpui-component's `TitleBar` draws the window controls and moves/maximizes the window): sidebar toggle, back/forward, workspace switcher, slots for the run widget and inbox, files-panel toggle (`WindowControls.tsx`)
- [x] done: back/forward history: tab switches and cross-file jumps, with the cursor of each file; closed files reopen, closed terminals are skipped; `Alt+←/→`, mouse side buttons, titlebar arrows (`src/lib/navHistory.ts`). Jumps inside one file are not recorded yet (the editor emits no event for them).
- [x] done: resizable sidebar (snaps to icons below 110 px), files dock and bottom panel; double click resets; sizes and hidden/open state persist (`PaneResizer.tsx`, `SidebarResizer.tsx`)
- [x] done: persisted state: open workspace, editor and diff tabs (preview flag, cursor), active tab, pane sizes, restored on start. Terminal tabs are not restored (Sessions stage).
- [x] done: command registry and keymap; Find Action (`Ctrl+Shift+A`, `src/lib/commands.ts`) is Search Everywhere's Actions tab
- [x] done: bottom panel with Build, Run, Debug and Problems tabs (see *Build / Run / Debug*)
- [x] done: crash log: a panic hook writes `<config dir>/native-crash.log`; the next start toasts it (`ErrorBoundary.tsx`). No in-app recovery: a panic ends the process.
- [x] done: Dashboard page: workspace cards with status summary, onboarding, the repositories with their brief (`src/pages/DashboardPage.tsx`). No context menu, Ralph or editor/folder buttons.
- [ ] partial: ConfirmDialog and ContextMenu (`src/overlay.rs`, keyboard: ↑/↓/Enter/Esc). Skeletons, Select, CheckBox, PathInput, Tooltip not yet (gpui-component has equivalents)
- [ ] partial: icons. File-type icons in the tree (`FileIcons.tsx`), tab icons (`Icons.tsx`), Lucide for the titlebar and toasts. No activity bar or brand icons yet (`BrandIcons.tsx`).
- [ ] — Arrow-key list navigation helper (`src/lib/arrowNav.ts`); only the file tree has key navigation
- [ ] — Markdown rendering (`SafeMarkdown.tsx`, `MarkdownPreview.tsx`); gpui-component has a Markdown view

### Workspaces & sidebar
- [ ] partial: workspace list from `config.yaml` / `.workspace.yaml`, with sessions, status indicators, recent sessions, repos (`src/App.tsx`, `StatusIndicator.tsx`). No variant (race) grouping beyond the sort, no sidebar width animation.
- [x] done: Create workspace: blank (manual), from an existing branch, or from a Shortcut/Linear card (searched, pre-fills name and branch, saved as the workspace's card), groups, base branch menu, linked repos noted (`WorkspaceCreateModal.tsx`)
- [ ] partial: Workspace detail page: delivery flow, repos with badges and pull, PRs with check aggregates, remove (`WorkspaceDetailPage.tsx`). No plan, race, artifacts, usage, investigate/fix checks with AI, review feedback.
- [x] done: Add repositories (`AddReposModal.tsx`)
- [x] done: Branch drift banner: switch back, stash and switch, auto-switch (`BranchDriftBanner.tsx`)
- [ ] — Folders card (`FoldersCard.tsx`) and folder scopes `dir:<path>` (`src/lib/folderScope.ts`)
- [ ] — Open in an external editor / reveal folder
- [x] done: Workspace pull (fast-forward from a repo row) and status summary (dashboard, delivery flow)
- [ ] partial: CI artifacts and AI usage as tool tabs per workspace (`ArtifactsSection.tsx`, `UsageBar.tsx`); Race (`Race.tsx`) not ported
- [ ] — Plan modal and PLAN.md progress (`PlanModal.tsx`, `PlanProgress.tsx`)
- [x] done: Integrations page: Shortcut, Linear, Figma tokens, connect / disconnect (`IntegrationsPage.tsx`)
- [x] done: open the workspace or a repo in VS Code / Zed, reveal the workspace folder

### Repos / worktrees
- [x] done: Settings → Repositories, folders, repo form (inline), GitHub repo picker, local clones, groups (`ReposSection.tsx`, `FoldersCard.tsx`, `RepoFormModal.tsx`, `GithubRepoModal.tsx`, `GroupsSection.tsx`). Per-OS build commands are kept, not edited.
- [ ] partial: Repo page: branches (switch, create, delete), pull, push/publish, commit, stash, history (undo, revert), stashes, a paused operation, prune (`RepoPage.tsx`). No compare or cherry-pick.
- [ ] partial: repo briefs on the dashboard and the sidebar, re-read every 30 s (`src/lib/useRepoBriefs.ts`); no background fetch timer

### Sessions / agents
- [ ] partial: new session (shell, Claude Code, Open Code, OMP in the current scope) and codenames (`NewSessionMenu.tsx`, `src/lib/names.ts`). No "Open in" folder/repo picker, no rename.
- [x] done: agent status from spinner lines and Claude hooks, in tab titles and the sidebar (`src/lib/agentStatus.ts`, backend `agent_hooks.rs`)
- [ ] partial: resume claude/opencode sessions (Recent per scope, restart with `--resume`). No initial prompt injection (`TerminalPane.tsx`), minimized agent tabs as `App.tsx`.
- [ ] — Live agent feed (`AgentFeed.tsx`)
- [ ] partial: Ralph view: plan or repo PRD, stories, start (iterations, push) / pause / stop, run state, live feed, past runs (`RalphView.tsx`, `RalphRunModal.tsx`). PRD writing and editing (`RalphPrdModal.tsx`), run overrides of agent/model, the custom prompt are not ported.

### Terminal
- [x] done: PTY terminal on `orbit_core::pty` with spawn at the real size, write, resize, 10k scrollback, 256/true colour, cursor shapes, OSC title, bracketed paste, alternate-screen wheel, flow control, scrollback restore after restart and forget on close (`TerminalPane.tsx`)
- [x] done: selection (drag, word, line), copy (`Ctrl+Shift+C`, primary on select), paste (`Ctrl+Shift+V`, `Shift+Insert`, middle click)
- [x] done: mouse reporting to TUIs (X10/normal and SGR; click, drag, motion, wheel)
- [ ] — Kitty graphics (`src/lib/kittyGraphics.ts`)
- [x] done: theme-driven ANSI colours (every theme)

### File tree / right dock
- [x] done: file tree with lazy background loads, `.gitignore`, symlinked repos, keyboard navigation (arrows, Home/End, Enter, F2, Delete), single click previews, double click or Enter keeps the tab; follows the active file; context menu: new file/folder, open, rename, copy path / relative path, reveal in file manager, open in terminal, delete to the Trash (`FileTreePanel.tsx`). Not ported: drag-and-drop move, "Add Files…" (import), dropping a path into a terminal. A rename moves open editors (unsaved text and cursor kept).
- [x] done: auto-refresh on disk changes: open folders re-read (expanded ones stay expanded), unmodified editors reload; `build*/` and `.cache/` changes ignored (`src/lib/useFsChanges.ts`). No refresh on window focus yet.
- [ ] partial: Files / Find / Git dock toolbar, repo select, Git panel with changes and commits (`GitPanel.tsx`); no commit review from the panel

### Tabs
- [x] done: preview tabs (reuse the preview slot; editing or double click keeps it) (`src/lib/previewTabs.ts`)
- [x] done: sessions and files groups with a divider, dirty dot, `Ctrl+W` closes the active file tab (refused when unsaved; `Ctrl+Shift+W` discards)
- [x] done: definitions in other files open as editor tabs
- [x] done: tab context menu as the React app (Close, Keep Open, Close Other Files, Close Files to the Right, Close Saved Files, Close All Files, Close Other Tabs, Close All Tabs; unsaved files stay open with a toast), middle click closes, unsaved-changes dialog (Save / Don't Save / Cancel)
- [ ] — rename session tabs

### Editor
- [x] done: code editor with tree-sitter highlighting (C, C++, Rust, TS/TSX, JS, JSON), line numbers, undo/redo
- [x] done: `Ctrl+S` save, dirty state
- [x] done: `Ctrl+G` go to line, `F8`/`Shift+F8` problems, status bar (problems, Unsaved, Ln/Col and selection, LF/CRLF, indent, language, LSP chip with progress and restart) (`EditorPane.tsx`)
- [x] done: breadcrumb; the file opens its outline, folders reveal in the tree
- [x] done: clangd setup banner (missing compile DB, CMake with presets, restart clangd)
- [x] done: symbol search in project (`Ctrl+T`) and in file (`Ctrl+Shift+O`) (`SymbolSearch.tsx`)
- [x] done: reload when changed on disk (unmodified editors; the cursor keeps its line)
- [ ] — Markdown editor/preview toggle

### LSP
- [x] done: client over stdio, one server per (repo, language), started on demand by `orbit_core::lsp` with the Tauri app's server choice and config (`src/lib/lsp/client.ts`, `manager.ts`)
- [x] done: path mapping for symlinked repos (`src/lib/lsp/paths.ts`)
- [x] done: definition (F12, Ctrl+click), hover, diagnostics
- [x] done: references, implementation, type definition, document and workspace symbols, completion, signature help, code actions (with `workspace/applyEdit`), formatting, rename across files, `$/progress`, restart
- [ ] — declaration, document highlights, semantic tokens, snippets, completion resolve
- [ ] partial: servers for Python, Go, C#, Lua are mapped like C++/Rust/TS (orbit-core's table), untested (only clangd here)
- [ ] partial: Languages settings (server per language, custom command, enabled, errors only, hidden codes, running servers with log and stop) (`LanguagesSection.tsx`). The editor applies the diagnostics filter (errors only, hidden codes), live; no "hide this diagnostic" quick fix yet.

### Search
- [x] done: Search Everywhere (double Shift toggles, `Ctrl+Shift+A` on Actions): tabs All/Files/Symbols/Actions/Text with Tab/Shift+Tab, fuzzy-ranked files, `workspace/symbol` over the workspace's running servers, registry actions, text through orbit-core's search (debounced), "more ›" section headers (`SearchEverywhereModal.tsx`, `src/lib/fuzzy.ts`, `src/lib/commands.ts`)
- [ ] partial: Find in Files (`Ctrl+Shift+F`): case/words/regex toggles (and Alt+C/W/X), repo chips, file mask, results grouped by file with the hits highlighted, Enter opens the match selected (`FindInFiles.tsx`). No live preview pane; the Tauri app has no replace either.
- [ ] partial: find within a file. gpui-component's editor has search; it is untested and has no binding here.

### Review / diff / PR
- [x] done: diff tab for one file's working-tree changes (untracked and deleted files too), split/unified, unchanged lines folded, F7/Shift+F7 next/previous change (`ReviewPane.tsx`, `useDiffNav.ts`). No commit diffs yet.
- [x] done: changes review across repos: changed files per repo with +/− and open-note counts, refreshed from disk every 3 s; each file opens as a diff tab (`ChangesReview.tsx`). The React page shows every diff in one scrolling list; here each is a tab. No "viewed" checkboxes.
- [x] done: local review notes in REVIEW.md (same file and format as the Tauri app, through `orbit_core::notes`): `+` beside a line or a selection (or *Add Review Note on Selected Lines*) leaves one, notes show under their lines and follow moved code (`anchorNote`), resolve/reopen, reply, edit, delete, show/clear resolved, the notes list, *Fix N notes with agent* (an open agent session or a new one), *Send to PR…* (`useReviewZones.tsx`, `src/lib/notes.ts`, `SendToPrModal.tsx`). Replies are paragraphs appended to the note (`Reply: …`), since REVIEW.md has no threads. Note bodies are plain text, not Markdown.
- [ ] — Commit review (`CommitReviewPane.tsx`)
- [ ] — GitHub PR review threads and submit, Code Review page, feature page, PR checkout, address review (`PrReviewPane.tsx`, `ReviewThreads.tsx`, `SubmitReview.tsx`, `CodeReviewPage.tsx`, `FeaturePage.tsx`, `PrCheckoutModal.tsx`, `AddressReview.tsx`)

### Git operations
- [ ] partial: PRs (`PrsModal.tsx`): the workspace's open PRs and saved PRs' live state, create PRs for the repos without one (title from the branch or *Draft with AI*, description), into the workspace base.
- [x] done: commit (files, partial commits, AI drafts, discard), push (force with lease, dropped repos, open PR note), rebase (conflicts, resolve with AI, abort) and squash-and-merge flows (`CommitModal.tsx`, `PushModal.tsx`, `RebaseModal.tsx`, `MergeModal.tsx`). The commit flow opens diffs as tabs instead of a side pane.
- [ ] partial: checks: list per PR with the aggregate, failed-check logs (the tail the AI gets), re-run failed jobs. No investigate/fix with AI; discard and sync branch not ported.

### Run / Build / Debug
- [x] done: run widget with configurations (all repos of the workspace) and CMake presets, remembered per workspace, `Ctrl+F9`/`Shift+F10`/`Shift+F9`/`Ctrl+F2` (`RunWidget.tsx`, `src/lib/runStore.ts`)
- [x] done: bottom panel: build output with `file:line` links, run terminal (cwd, args, env), problems (GCC/Clang/MSVC/ld, also as editor diagnostics with source `build`), debug (`BottomPanel.tsx`, `src/lib/problems.ts`)
- [ ] — Multi-repo build modal (`BuildModal.tsx`)
- [x] done: DAP debugger: session (gdb `-i dap` / lldb-dap), gutter breakpoints (click, `Ctrl+F8`, persisted), paused line, threads, call stack, lazy variables, console with evaluate, `F9`/`F8`/`F7`/`Shift+F8`, symlink path mapping (`src/lib/dap/*`, `DebugView.tsx`). lldb-dap is wired but untested (not installed here).

### Settings / themes
- [x] done: Settings page with its filtered section list and every section: Repositories, Groups, AI, Languages, Appearance, Health, Updates (`SettingsPage.tsx`, `AiSection.tsx`, `AppearanceSection.tsx`, `HealthSection.tsx`, `UpdatesSection.tsx`). Appearance shows Orbit Dark only.
- [x] done: update check at start, hourly and on demand, with a toast once per version (`src/lib/updater.ts`); no install yet (packaging)
- [ ] partial: themes. All seven of `src/lib/theme.ts` as live tokens (`theme::set`), saved, defaulting to the Tauri app's choice; the Appearance picker belongs to the settings page.

### Notifications
- [x] done: toasts (success/error/info, auto-dismiss, at most four, ×) (`Toast.tsx`)
- [ ] partial: the inbox of agent sessions (finished / needs attention, unread until seen) in the sidebar (`Inbox.tsx`). No OS notifications or Ralph items.

### Backend services
- [x] done: the backend is shared through `crates/orbit-core` (config, workspaces, git, GitHub, LSP/DAP processes, watcher, builds, run configs, agents, Ralph, review, search, notes, files); the native app uses its workspace list and LSP manager
- [ ] partial: native UI writes state through core: workspaces (create, add repos, remove), commits, pushes, rebases, merges, branches, stashes, REVIEW.md notes, PRs; config editing not yet
- [x] done: FS/git watcher events in the native UI (`orbit_core::watch::watch` with a channel sink; the tree and editors)
- [x] done: run configs (`.orbit/run.json`, with `run.local.json` and CMake presets) through `orbit_core::run`
- [x] done: PTY registry, scrollback and flow control in core (`orbit_core::pty`); `src-tauri/src/pty.rs` is an adapter
