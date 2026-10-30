@echo off
rem OpenMemoryOptimizer - bump the version (see scripts\bump-version.ps1)
rem Copyright (C) 2026 Lucian Caetano
rem SPDX-License-Identifier: GPL-3.0-or-later
rem
rem Usage: bump-version.cmd                             interactive menu
rem        bump-version.cmd patch^|minor^|major [-Yes] [-NoCommit]

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\bump-version.ps1" %*
set "RC=%ERRORLEVEL%"
if "%~1"=="" pause
exit /b %RC%
