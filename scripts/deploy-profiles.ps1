<#
.SYNOPSIS
Copies the repo's profiles (profiles/*) into the profiles_dir that sbxm reads.

.DESCRIPTION
Finds profiles_dir the way sbxm does: the `profiles_dir` key in <config dir>\config.toml, else
<config dir>\profiles, where <config dir> is -ConfigDir, $env:SBXM_CONFIG_DIR or ~/.config/sbxm. Each folder in
the source that holds a profile.toml replaces the same-named folder there (files the repo no longer has are
removed). Profiles that exist only in profiles_dir are left alone. Existing sandboxes show config drift until
they are rebuilt (`sbxm open <project> --harness <h> --rebuild`).

.EXAMPLE
./scripts/deploy-profiles.ps1
./scripts/deploy-profiles.ps1 -ConfigDir E:\other-config
#>
param(
    [string]$Source = (Join-Path $PSScriptRoot '..\profiles'),
    [string]$ConfigDir = $(if ($env:SBXM_CONFIG_DIR) { $env:SBXM_CONFIG_DIR } else { Join-Path $HOME '.config/sbxm' })
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Source -PathType Container)) { throw "profiles source folder not found: $Source" }

$target = Join-Path $ConfigDir 'profiles'
$configFile = Join-Path $ConfigDir 'config.toml'
if (Test-Path -LiteralPath $configFile) {
    $line = Get-Content -LiteralPath $configFile | Where-Object { $_ -match '^\s*profiles_dir\s*=\s*(["''])(.*)\1' } | Select-Object -First 1
    if ($line) { $null = $line -match '^\s*profiles_dir\s*=\s*(["''])(.*)\1'; $target = $Matches[2] }
}

# Hash every file by relative path, so "unchanged" means the whole folder matches.
function Get-FolderState([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return $null }
    $root = (Resolve-Path -LiteralPath $dir).Path
    (Get-ChildItem -LiteralPath $root -Recurse -File | Sort-Object FullName | ForEach-Object {
        '{0}={1}' -f $_.FullName.Substring($root.Length), (Get-FileHash -LiteralPath $_.FullName).Hash
    }) -join "`n"
}

$profiles = @(Get-ChildItem -LiteralPath $Source -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'profile.toml') })
if (-not $profiles) { throw "no profiles (folders with a profile.toml) in $Source" }

New-Item -ItemType Directory -Force $target | Out-Null
Write-Host "Deploying to $target"
foreach ($profile in $profiles) {
    $dest = Join-Path $target $profile.Name
    $before = Get-FolderState $dest
    $state = if ($null -eq $before) { 'new' } elseif ($before -eq (Get-FolderState $profile.FullName)) { 'unchanged' } else { 'updated' }
    if ($state -ne 'unchanged') {
        if (Test-Path -LiteralPath $dest) { Remove-Item -LiteralPath $dest -Recurse -Force }
        Copy-Item -LiteralPath $profile.FullName -Destination $dest -Recurse
    }
    Write-Host "  $($profile.Name): $state"
}
