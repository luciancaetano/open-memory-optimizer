@echo off
rem OpenMemoryOptimizer - release build and packaging
rem Copyright (C) 2026 Lucian Caetano
rem SPDX-License-Identifier: GPL-3.0-or-later
rem
rem Usage: build.cmd                    build dist\OpenMemoryOptimizer.exe and the zip
rem        build.cmd installer          also build dist\OpenMemoryOptimizer-Setup-<version>.exe
rem        build.cmd clean [installer]  remove target\ and dist\ first
rem
rem The installer needs Inno Setup 6: winget install JRSoftware.InnoSetup

setlocal
cd /d "%~dp0"

set "DO_CLEAN="
set "DO_INSTALLER="
for %%a in (%*) do (
    if /i "%%~a"=="clean" set "DO_CLEAN=1"
    if /i "%%~a"=="installer" set "DO_INSTALLER=1"
)

where cargo >nul 2>&1
if errorlevel 1 (
    echo [error] Rust was not found. Install it from https://rustup.rs and try again.
    exit /b 1
)

if defined DO_CLEAN (
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

if not defined DO_INSTALLER goto done

rem Package version from Cargo.toml (the first "version = ..." line is [package]).
set "VERSION="
for /f "tokens=2 delims== " %%v in ('findstr /b /c:"version" Cargo.toml') do if not defined VERSION set "VERSION=%%~v"
if not defined VERSION (
    echo [error] Could not read the version from Cargo.toml.
    exit /b 1
)

set "ISCC="
for /f "delims=" %%i in ('where iscc 2^>nul') do if not defined ISCC set "ISCC=%%i"
if not defined ISCC if exist "%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if not defined ISCC if exist "%ProgramFiles%\Inno Setup 6\ISCC.exe" set "ISCC=%ProgramFiles%\Inno Setup 6\ISCC.exe"
if not defined ISCC if exist "%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe" set "ISCC=%LOCALAPPDATA%\Programs\Inno Setup 6\ISCC.exe"
if not defined ISCC (
    echo [error] Inno Setup 6 was not found. Install it with: winget install JRSoftware.InnoSetup
    exit /b 1
)

echo Building the installer (version %VERSION%)...
"%ISCC%" /Q /DAppVersion=%VERSION% installer\OpenMemoryOptimizer.iss
if errorlevel 1 (
    echo [error] Installer build failed.
    exit /b 1
)

:done
echo.
echo Done:
echo   dist\OpenMemoryOptimizer.exe
echo   dist\OpenMemoryOptimizer-win64.zip
if defined DO_INSTALLER echo   dist\OpenMemoryOptimizer-Setup-%VERSION%.exe
endlocal
