# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

OpenMemoryOptimizer is a Windows-only GUI memory optimizer in Rust (eframe/egui 0.37). Its cleanup
techniques were reverse-engineered from Wise Memory Optimizer 4.2.4.132 and reimplemented. Comments with
`0x...` addresses (e.g. "port of 0xc8d170") refer to routines in that original binary. Keep behavior
faithful to them unless a change is a deliberate, documented divergence. Existing divergences:
- Freed memory is reported honestly; the original faked 1–10 MB when nothing was freed.
- `SetSystemFileCacheSize(0, 0, flags)` is skipped when hard cache limits are set.
- `schtasks` gets `/f`.

Licensed GPL-3.0-or-later. Every source file (including `build.rs`) starts with the GNU license notice.
Add it to new source files.

## Commands

Requires Windows, the MSVC Rust toolchain and the Windows SDK (`rc.exe`, used by `embed-resource`).

```
build.cmd                        # release build -> dist\OpenMemoryOptimizer.exe + dist\OpenMemoryOptimizer-win64.zip
build.cmd installer              # also dist\OpenMemoryOptimizer-Setup-<version>.exe (needs Inno Setup 6)
build.cmd clean                  # same, from scratch
cargo test                       # all unit tests (lib only)
cargo test --lib auto::tests::period_mode_counts_minutes   # single test
cargo clippy --all-targets       # expected to be warning-free
cargo fmt
cargo build --features as-invoker --target-dir target/invoker   # dev build that starts without UAC
```

From Git Bash, run the batch file as `cmd //c "C:\\full\\path\\build.cmd"`. Keep `.cmd` files CRLF
without a BOM (a BOM breaks the first line); keep `installer/*.iss` CRLF with a UTF-8 BOM (Inno Setup
needs it for the Portuguese strings).

The installer (`installer/OpenMemoryOptimizer.iss`, Inno Setup 6) installs machine-wide. Its optional
"start when I sign in" task creates the scheduled task `OpenMemoryOptimizer Startup` running
`--minimized`, because a Registry `Run` entry cannot auto-start an app that requires administrator rights.
The uninstaller deletes that task and `schedule::TASK_NAME`, so keep those names in sync.

### Why tests live only in the lib

`build.rs` embeds a `requireAdministrator` manifest (and `icon.ico`) into every bin target via
`cargo:rustc-link-arg-bins`. A bin's test harness would then fail to launch unelevated
(os error 740), so the bin has `test = false` and all logic plus tests live in the library crate
`open_memory_optimizer` (`src/lib.rs`). Keep testable logic in the lib, not in `src/gui/`.

### Visual verification without touching the user's desktop

Do not capture the user's screen to check the UI. Use the debug-only snapshot mode instead. It opens
the window off-screen, never activates it, skips the tray and auto-timer, renders each page from egui's
framebuffer to PNG, then exits:

```
cargo build --features as-invoker --target-dir target/invoker
MEMOPT_SNAPSHOT=<dir> target/invoker/debug/OpenMemoryOptimizer.exe
```

The page list is `PAGES` in `src/gui/main.rs`. The `dashboard_result` page loads a fixture
(`load_snapshot_fixture`) so the result card and toast render without running a real optimization.

## Releases

`.github/workflows/release.yml` runs on every push to `main`. A Linux job compares the `[package]`
version in `Cargo.toml` with the highest existing `vX.Y.Z` release (semver, `sort -V`). Only if it is
greater does a Windows job run `cargo test --release --locked` and `build.cmd installer`, then create the release
`v<version>` with the installer, the zip and `SHA256SUMS.txt`. To release, run `bump-version.cmd`
(`scripts/bump-version.ps1`). It bumps major/minor/patch in `Cargo.toml`, `Cargo.lock` and the `.iss`,
then optionally commits `chore: release vX.Y.Z` and pushes. It must not create a git tag: the workflow
creates the tag with the release and fails if the tag already exists. The About page reads the version from
`CARGO_PKG_VERSION`.

## Architecture

**Library (`src/`)**: the core. It has no UI.
- `optimizer::run(mode, methods, log)` is one cleanup pass, ported from the original worker thread.
  The order is fixed: clipboard → defrag → trim all working sets → purge standby list → file cache →
  3 × 1 s settle → measure. `Mode::Manual` (button, scheduled task) honors every selected method.
  `Mode::Auto` (timer) skips defrag and standby and always trims. A static `RUNNING` flag makes
  concurrent calls return `None`.
- `techniques.rs` holds the five techniques. ntdll/psapi/kernel32 functions are resolved at runtime
  with `win::resolve` (LoadLibrary + GetProcAddress), as the original does, instead of being imported
  statically.
- `privilege::enable(&STARTUP_PRIVILEGES)` must run before optimizing. Standby purge and working-set
  trimming of other processes silently do little without SeProfileSingleProcess, SeDebug and the
  other startup privileges.
- `settings.rs` reads and writes `MemoryConfig.ini` next to the executable. Key names and defaults
  match the original, including the misspelled `ClearStandy` key. `trim_working_sets` is never
  persisted and is forced on at load. If neither `LowmemOpt` nor `periodOpt` is set, `LowmemOpt` is
  forced on.
- `auto::AutoState::tick` is the pure auto-trigger logic for a 60 s tick:
  - Low-memory mode checks every 5th tick, requires free memory below the threshold, and, when
    idle-only is on, requires CPU ≤ 5 %.
  - Periodic mode fires every N ticks.
  - The caller drives the ticks.
- `cpu.rs` computes CPU % from NtQuerySystemInformation idle-time deltas. The first sample returns 0.
- `schedule.rs` builds the `schtasks /create ... /RL HIGHEST` command. The task runs
  `OpenMemoryOptimizer.exe --optimize`.

**GUI (`src/gui/`)**: the single binary `OpenMemoryOptimizer`.
- `main.rs`: `main()` handles `--optimize` first: it does one silent manual pass with the saved
  settings and exits without a window. Otherwise it starts eframe with a borderless window and a
  custom title bar.
- Threading: background threads never touch UI state. The optimization worker, the auto-timer thread
  and the tray/menu event handlers send an `Event` over an mpsc channel held in `Shared` (which also
  holds the settings mutex and the `egui::Context`), then call `request_repaint()`.
  `App::handle_events` drains the channel.
- eframe 0.37 replaced `update` with `App::ui` plus `App::logic`. Only `logic` runs while the window
  is hidden in the tray (after `request_repaint`). Event handling and the "minimized → hide to tray"
  detection therefore live in `logic`. Tray restore uses `ViewportCommand::Visible/Minimized/Focus`.
- Minimize-to-tray reacts to the *transition* into minimized (`was_minimized`). This avoids
  re-hiding on the stale minimized flag that arrives right after a restore.
- `theme.rs` holds the palette, the fonts and the app icon:
  - Fonts are loaded from `%WINDIR%\Fonts` at runtime: Segoe UI, plus Segoe Fluent Icons or MDL2
    Assets for icon glyphs; code points are in `theme::icons`.
  - `icon.png` is `include_bytes!`'d and resized for the title bar, About page, window and tray
    icons. It is masked only if it has no alpha channel.
- `widgets.rs` holds custom-painted widgets: toggle, gradient ring gauge, cards, buttons, segmented
  control. Fixed-size widgets paint into an allocated rect and lay out content with
  `ui.new_child(...)`. `scope_builder` advances the parent cursor a second time inside horizontal
  layouts.

egui 0.37 API notes, since older examples won't compile:
- `Id::new` is deprecated; use `Id::unique`.
- Panels are `egui::Panel::top/left(...)`.
- `CentralPanel::show(ui, ..)` takes a `&mut Ui`.
- Painter rect strokes need a `StrokeKind`.
- egui is used through `eframe::egui`.

## UI copy

All user-facing text is plain English for non-technical users. Don't expose API names, "working
sets", "standby list", NTSTATUS codes, etc. in the GUI; map them to everyday wording, such as
"Free memory from open apps" or "Cached memory".
