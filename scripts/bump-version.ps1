# OpenMemoryOptimizer - bump the version and (optionally) publish a release
# Copyright (C) 2026 Lucian Caetano
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Usage: bump-version.cmd                  interactive menu
#        bump-version.cmd patch|minor|major [-Yes] [-NoCommit]
#
# Updates the version in Cargo.toml, Cargo.lock and the installer script. Pushing the
# commit to main makes the Release workflow build the installer and publish v<version>.
# No tag is created here: the workflow creates it together with the release.

param(
    [ValidateSet('major', 'minor', 'patch')]
    [string]$Part,
    [switch]$Yes,
    [switch]$NoCommit
)

$ErrorActionPreference = 'Stop'
Set-Location (Split-Path -Parent $PSScriptRoot)

function Ask-YesNo([string]$Question) {
    $answer = Read-Host "$Question [y/N]"
    return $answer -match '^(y|yes|s|sim)$'
}

function Replace-Once([string]$Path, [string]$Pattern, [string]$Replacement, [bool]$Bom) {
    $text = [IO.File]::ReadAllText((Resolve-Path $Path))
    $regex = [regex]::new($Pattern, 'Multiline')
    if (-not $regex.IsMatch($text)) { throw "Version not found in $Path" }
    $text = $regex.Replace($text, $Replacement, 1)
    [IO.File]::WriteAllText((Resolve-Path $Path), $text, [Text.UTF8Encoding]::new($Bom))
}

# Current version: the first `version = "X.Y.Z"` line in Cargo.toml is [package].
$cargo = [IO.File]::ReadAllText((Resolve-Path 'Cargo.toml'))
$m = [regex]::Match($cargo, '(?m)^version\s*=\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $m.Success) { throw 'Could not read the version from Cargo.toml' }
$major = [int]$m.Groups[1].Value
$minor = [int]$m.Groups[2].Value
$patch = [int]$m.Groups[3].Value
$current = "$major.$minor.$patch"

$next = @{
    patch = "$major.$minor.$($patch + 1)"
    minor = "$major.$($minor + 1).0"
    major = "$($major + 1).0.0"
}

if (-not $Part) {
    Write-Host ''
    Write-Host "Current version: $current" -ForegroundColor Cyan
    Write-Host ''
    Write-Host "  1) patch  -> $($next.patch)   bug fixes, small changes"
    Write-Host "  2) minor  -> $($next.minor)   new features"
    Write-Host "  3) major  -> $($next.major)   big or incompatible changes"
    Write-Host ''
    switch (Read-Host 'Choose 1-3 (anything else cancels)') {
        '1' { $Part = 'patch' }
        '2' { $Part = 'minor' }
        '3' { $Part = 'major' }
        default { Write-Host 'Cancelled.'; exit 0 }
    }
}

$new = $next[$Part]
if (-not $Yes -and -not (Ask-YesNo "Bump version $current -> $new ($Part)?")) {
    Write-Host 'Cancelled.'
    exit 0
}

Replace-Once 'Cargo.toml' '^version\s*=\s*"\d+\.\d+\.\d+"' "version = `"$new`"" $false
Replace-Once 'installer\OpenMemoryOptimizer.iss' '^(\s*#define AppVersion\s+)"[^"]*"' "`${1}`"$new`"" $true
Replace-Once 'installer\OpenMemoryOptimizer.iss' '(/DAppVersion=)\d+\.\d+\.\d+' "`${1}$new" $true

# Let cargo record the new package version in Cargo.lock.
cargo metadata --format-version 1 --offline *> $null
if ($LASTEXITCODE -ne 0) { cargo metadata --format-version 1 *> $null }
if ($LASTEXITCODE -ne 0) { throw 'cargo could not update Cargo.lock' }

Write-Host ''
Write-Host "Version updated: $current -> $new" -ForegroundColor Green
Write-Host '  Cargo.toml, Cargo.lock, installer\OpenMemoryOptimizer.iss'

if ($NoCommit) { exit 0 }

$branch = (git rev-parse --abbrev-ref HEAD).Trim()
Write-Host ''
if ($branch -ne 'main') {
    Write-Host "Note: you are on '$branch'. The release is only published from main." -ForegroundColor Yellow
}
if (-not (Ask-YesNo "Commit and push to '$branch' to publish v$new?")) {
    Write-Host ''
    Write-Host 'Not committed. When ready:'
    Write-Host '  git add Cargo.toml Cargo.lock installer/OpenMemoryOptimizer.iss'
    Write-Host "  git commit -m `"chore: release v$new`""
    Write-Host '  git push'
    exit 0
}

git add Cargo.toml Cargo.lock installer/OpenMemoryOptimizer.iss
if ($LASTEXITCODE -ne 0) { throw 'git add failed' }
git commit -m "chore: release v$new"
if ($LASTEXITCODE -ne 0) { throw 'git commit failed' }
git push
if ($LASTEXITCODE -ne 0) { throw 'git push failed' }

$remote = (git remote get-url origin).Trim() -replace '\.git$', ''
Write-Host ''
Write-Host "Pushed. The release v$new will appear in a few minutes:" -ForegroundColor Green
Write-Host "  $remote/actions"
Write-Host "  $remote/releases"
