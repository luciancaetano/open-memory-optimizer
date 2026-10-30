@echo off
rem OpenMemoryOptimizer - release build and packaging
rem Copyright (C) 2026 Lucian Caetano
rem SPDX-License-Identifier: GPL-3.0-or-later
rem
rem Usage: build.cmd          build dist\OpenMemoryOptimizer.exe and the zip
rem        build.cmd clean    remove target\ and dist\ first

setlocal
cd /d "%~dp0"

where cargo >nul 2>&1
if errorlevel 1 (
    echo [error] Rust was not found. Install it from https://rustup.rs and try again.
    exit /b 1
)

if /i "%~1"=="clean" (
    echo Cleaning previous builds...
    cargo clean
    if exist dist rmdir /s /q dist
)

echo Building OpenMemoryOptimizer (release)...
cargo build --release
if errorlevel 1 (
    echo [error] Build failed.
    exit /b 1
)

if not exist dist mkdir dist
copy /y target\release\OpenMemoryOptimizer.exe dist\ >nul
if errorlevel 1 (
    echo [error] Could not copy the executable. Close OpenMemoryOptimizer if it is running and try again.
    exit /b 1
)
copy /y LICENSE dist\LICENSE.txt >nul

powershell -NoProfile -Command "Compress-Archive -Path 'dist\OpenMemoryOptimizer.exe','dist\LICENSE.txt' -DestinationPath 'dist\OpenMemoryOptimizer-win64.zip' -Force"
if errorlevel 1 (
    echo [error] Could not create the zip file.
    exit /b 1
)

echo.
echo Done:
echo   dist\OpenMemoryOptimizer.exe
echo   dist\OpenMemoryOptimizer-win64.zip
endlocal
