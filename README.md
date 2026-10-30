<p align="center">
  <img src="icon.png" alt="OpenMemoryOptimizer" width="96">
</p>

<h1 align="center">OpenMemoryOptimizer</h1>

<p align="center">A simple memory optimizer for Windows.</p>

## Download

Get the installer from the [latest release](https://github.com/luciancaetano/open-memory-optimizer/releases/latest).

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

## License

[GPL-3.0-or-later](LICENSE) © 2026 Lucian Caetano
