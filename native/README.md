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

Keys: `Ctrl+Shift+T` new terminal (a shell session) · `Ctrl+W` close file tab (refused when unsaved) ·
`Ctrl+Shift+W` close any tab (discards) · `Ctrl+Tab` next tab · `Ctrl+Q` quit · in the editor
`Ctrl+S` save, `F12` or `Ctrl+click` go to definition, hover for docs, `Shift+F12` references,
`Ctrl+F12` implementation, `Ctrl+Shift+O` file outline, `Ctrl+T` project symbols, `Ctrl+G` go to
line, `F8`/`Shift+F8` next/previous problem, `Ctrl+Space` completion (also as you type),
`Ctrl+Shift+Space` parameter hints (also on `(` and `,`), `Ctrl+.`/`Alt+Enter` quick fixes, `F2`
rename, `Ctrl+Shift+I`/`Shift+Alt+F` format (`editor::editor_actions()` lists them all, with
their GPUI action ids, for a command palette) · in a terminal
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

Sessions and free-form steps run after the fixed ones (`;`-separated):

```sh
./target/release/orbit-native --session shell --session claude   # open sessions (shell, claude, opencode, omp, any command)
    --steps "status:waiting@15; dump; action:sessions::NextSessionNeedingInput; wait:Allow Bash@5; \
             type:y\n; keys:ctrl-c; sleep:800; action:orbit::CloseTab; action:sessions::ResumeLastSession"
```

`wait:<text>[@secs]` waits for text in the active terminal, `status:<status>` for a session in that
status, `action:<name>` dispatches any registered GPUI action by name, `type:`/`keys:` go through
GPUI's keystroke dispatch, `dump` writes the sessions, inbox and recent list into the log. Waits
also record how late the UI thread ran its timers ("ui timer lag"), which is how a blocked UI shows.
The dump always ends with the sessions, inbox and recent list.

Self-test runs neither save nor restore sessions in the user's config. Point `ORBIT_CONFIG_DIR` at a
scratch folder to test that (and to keep scrollback and agent settings files out of the real one).
Agent sessions are tested without a real agent: `scripts/stub-agent/claude` stands in for Claude
Code (prints its argv, posts the hooks from Orbit's `--settings` file, waits for an answer); put it
first on `PATH`. Use `SHELL=/bin/bash` (zsh may start an interactive setup wizard).

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
- Not done: the "hide this diagnostic" quick fix and the diagnostics filter from config, folder
  crumbs revealing in the tree (needs the tree from the shell), Markdown preview, the Languages
  settings section.

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
| `src/editor.rs` | editor tab: `EditorState`, save, dirty, actions and keys (`editor_actions()`), pickers, problems, format, rename, signature help, breadcrumb, status bar, clangd banner |
| `src/editor/provider.rs` | the LSP document (sync) and gpui-component providers: definition, hover, completion, code actions; WorkspaceEdit and location parsing |
| `src/editor/picker.rs` | the quick-pick overlay: go to line, outline, project symbols, references, rename, quick fixes |
| `src/lsp/` | `mod.rs` file → language and the server registry · `server.rs` JSON-RPC client and URI mapping over `orbit_core::lsp` (which picks, starts and feeds the process) · `paths.rs` URIs and root rebase |
| `src/diff_view.rs` | diff tab over `git diff HEAD` |
| `src/terminal/` | terminal view over `orbit_core::pty`: VT parse, status tracking, scrollback replay (`mod.rs`), grid layout and paint (`grid.rs`), keys and mouse reports (`input.rs`) |
| `src/sessions/` | sessions (`mod.rs`: keys, codenames, save/restore, inbox, recent, resume, actions), agent status heuristics and hooks (`status.rs`), the sidebar block (`panel.rs`) |
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

- `term_image.rs`: reads files named by kitty graphics commands for the webview.
  (`pty.rs` moved to core in stage 3 and is now a thin adapter, see *Stage 3*.)
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
- [ ] partial: new session (shell, Claude Code, Open Code, OMP in the current scope) and codenames (`NewSessionMenu.tsx`, `src/lib/names.ts`). No "Open in" folder/repo picker, no rename.
- [x] done: agent status from spinner lines and Claude hooks, in tab titles and the sidebar (`src/lib/agentStatus.ts`, backend `agent_hooks.rs`)
- [ ] partial: resume claude/opencode sessions (Recent per scope, restart with `--resume`). No initial prompt injection (`TerminalPane.tsx`), no minimized agent tabs (`App.tsx`).
- [ ] — Live agent feed (`AgentFeed.tsx`)
- [ ] — Ralph view, PRD and run modals (`RalphView.tsx`, `RalphPrdModal.tsx`, `RalphRunModal.tsx`)

### Terminal
- [x] done: PTY terminal on `orbit_core::pty` with spawn at the real size, write, resize, 10k scrollback, 256/true colour, cursor shapes, OSC title, bracketed paste, alternate-screen wheel, flow control, scrollback restore after restart and forget on close (`TerminalPane.tsx`)
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
- [x] done: `Ctrl+G` go to line, `F8`/`Shift+F8` problems, status bar (problems, Unsaved, Ln/Col and selection, LF/CRLF, indent, language, LSP chip with progress and restart) (`EditorPane.tsx`)
- [ ] partial: breadcrumb; the file opens its outline, folders don't reveal in the tree yet
- [x] done: clangd setup banner (missing compile DB, CMake with presets, restart clangd)
- [x] done: symbol search in project (`Ctrl+T`) and in file (`Ctrl+Shift+O`) (`SymbolSearch.tsx`)
- [ ] — Markdown editor/preview toggle, reload when changed on disk

### LSP
- [x] done: client over stdio, one server per (repo, language), started on demand by `orbit_core::lsp` with the Tauri app's server choice and config (`src/lib/lsp/client.ts`, `manager.ts`)
- [x] done: path mapping for symlinked repos (`src/lib/lsp/paths.ts`)
- [x] done: definition (F12, Ctrl+click), hover, diagnostics
- [x] done: references, implementation, type definition, document and workspace symbols, completion, signature help, code actions (with `workspace/applyEdit`), formatting, rename across files, `$/progress`, restart
- [ ] — declaration, document highlights, semantic tokens, snippets, completion resolve
- [ ] partial: servers for Python, Go, C#, Lua are mapped like C++/Rust/TS (orbit-core's table), untested (only clangd here)
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
- [ ] partial: the inbox of agent sessions (finished / needs attention, unread until seen) in the sidebar (`Inbox.tsx`). No toasts, OS notifications or Ralph items; otherwise a status-bar notice.

### Backend services
- [x] done: the backend is shared through `crates/orbit-core` (config, workspaces, git, GitHub, LSP/DAP processes, watcher, builds, run configs, agents, Ralph, review, search, notes, files); the native app uses its workspace list and LSP manager
- [ ] partial: no native UI writes state yet; the core functions for it exist (`workspace::create`, `repo::commit`, `notes::add`, …)
- [ ] — FS/git watcher events in the native UI (`orbit_core::watch::watch` with a channel sink), run configs (`.orbit/run.json`)
- [x] done: PTY registry, scrollback and flow control in core (`orbit_core::pty`); `src-tauri/src/pty.rs` is an adapter
