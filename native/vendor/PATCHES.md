# Vendored crates

## `gpui-pre-wgpu` 0.3.11 (Zed's `gpui_wgpu`, zed@f816e1f)

Wired in by `[patch.crates-io]` in the repository root's `Cargo.toml`. The crate is only a dependency on Linux
and FreeBSD (macOS uses Metal, Windows DirectX), so the patch changes nothing on other OSes.

**What changed.** Only `src/wgpu_context.rs`: `WgpuContext::instance` asked for
`Backends::VULKAN | Backends::GL`; it now calls a new `native_backends()`:

- `WGPU_BACKEND` set (wgpu's own variable: `vulkan`, `gl`, …): use it;
- otherwise probe a Vulkan-only instance (no display); if it lists a non-CPU adapter, use
  `VULKAN`, else `GL`. The answer is cached for the process (device-lost recreation reuses it).

The rest of the crate is the crates.io package as published (`Cargo.lock` and `.cargo-ok`
dropped).

**Why.** With both backends wgpu loads every vendor's Vulkan driver *and* every EGL stack (on an
Intel + NVIDIA laptop: NVIDIA's EGL libraries plus Mesa's `libgallium` and `libLLVM`), and maps
them even though one adapter is used. Idle PSS went from 158 MB to 120 MB (see the README's
measurements). The GL path stays for machines without a Vulkan driver (old GPUs, some VMs).

**Re-applying on upgrade** (gpui-component bumps `gpui-pre`):

1. `cp -r ~/.cargo/registry/src/*/gpui-pre-wgpu-<new>/ native/vendor/gpui-pre-wgpu` (after a
   build has fetched the new version with the patch line commented out), delete `.cargo-ok`
   and `Cargo.lock`.
2. In `wgpu_context.rs`, replace the `backends:` of `instance()` with
   `Self::native_backends()` and paste `native_backends()` back (it is marked
   `Orbit patch`).
3. `cargo build`, check `cargo tree -i gpui-pre-wgpu` shows the path, and test both paths:
   a normal start, and `VK_DRIVER_FILES=/nonexistent.json` (no Vulkan: must fall back to GL and
   still print `first frame after … ms`).

Drop the vendored copy when upstream makes the backend set configurable (Zed has no such option
at f816e1f).

**Frame capture (feature `orbit-capture`).** `src/wgpu_renderer.rs` gains a small
`orbit_capture` module: when built with the feature and `ORBIT_CAPTURE=<file.png>` is set, the
surface gets `COPY_SRC` and each drawn frame is read back and written to that file. Off by
default; the native crate enables it with `--features capture`. To re-apply, copy the module
and its two call sites (the surface `usage` and the `save` call before `present`).
