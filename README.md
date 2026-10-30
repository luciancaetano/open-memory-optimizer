<p align="center">
  <img src="icon.png" alt="OpenMemoryOptimizer" width="96">
</p>

<h1 align="center">OpenMemoryOptimizer</h1>

<p align="center">A simple memory optimizer for Windows.</p>

<p align="center">
  <a href="https://github.com/luciancaetano/open-memory-optimizer/releases/latest"><b>⬇ Download the latest version</b></a>
</p>

## Download

1. Open the [latest release](https://github.com/luciancaetano/open-memory-optimizer/releases/latest).
2. Under **Assets**, download `OpenMemoryOptimizer-Setup-<version>.exe`.
3. Run it and follow the steps. Windows asks for administrator permission because the app needs it to free memory.

Prefer no installation? Download `OpenMemoryOptimizer-win64.zip`, extract it and run `OpenMemoryOptimizer.exe`.

If Windows shows "Windows protected your PC", click **More info → Run anyway**. The app is not
code-signed yet. To check a download, compare it with `SHA256SUMS.txt` from the same release.

Requires Windows 10 or 11 (64-bit).

## Features

- Live view of memory usage
- One-click cleanup
- Automatic cleanup when memory runs low or on a timer
- Scheduled cleanups (daily, weekly, at sign-in…)
- Runs quietly in the system tray

## Build

Requires Windows 10/11 and [Rust](https://rustup.rs).

```
build.cmd             # dist\OpenMemoryOptimizer.exe
build.cmd installer   # also dist\OpenMemoryOptimizer-Setup-<version>.exe
```

The installer needs [Inno Setup 6](https://jrsoftware.org/isinfo.php) (`winget install JRSoftware.InnoSetup`).

## Releasing a new version

```
bump-version.cmd
```

Choose **patch** (bug fixes), **minor** (new features) or **major** (big changes). The script updates the
version and offers to commit and push. Every push to `main` with a version higher than the latest release
builds the installer on GitHub Actions and publishes it as a new
[release](https://github.com/luciancaetano/open-memory-optimizer/releases).

## License

[GPL-3.0-or-later](LICENSE) © 2026 Lucian Caetano
