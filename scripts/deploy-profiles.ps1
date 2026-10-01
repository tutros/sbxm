<#
.SYNOPSIS
Copies the repo's profiles (profiles/*) into the profiles_dir that sbxm reads.

.DESCRIPTION
Asks sbxm where it reads profiles from (`sbxm config profiles-dir`), so the answer always follows sbxm's own
config rules: an invalid config.toml (an unknown key or table, a typo) or a missing one stops the deploy
before anything is written, with sbxm's message. -ConfigDir is handed to that call as SBXM_CONFIG_DIR.

Each folder in the source that holds a profile.toml replaces the same-named folder there (files the repo no
longer has are removed). The new copy is staged beside the old one and swapped in only once it is complete, so a
failed copy leaves the working profile in place. Profiles that exist only in profiles_dir are left alone.
Existing sandboxes show config drift until they are rebuilt (`sbxm open <project> --harness <h> --rebuild`).

.PARAMETER ProfilesDirCommand
How to run sbxm's `config profiles-dir`. The default needs `sbxm` on PATH; `just deploy-profiles` passes a
`cargo run` form so a checkout works without installing.

.EXAMPLE
./scripts/deploy-profiles.ps1
./scripts/deploy-profiles.ps1 -ConfigDir E:\other-config
./scripts/deploy-profiles.ps1 -ProfilesDirCommand { cargo run --quiet -- config profiles-dir }
#>
param(
    [string]$Source = (Join-Path $PSScriptRoot '..\profiles'),
    [string]$ConfigDir,
    [scriptblock]$ProfilesDirCommand = { sbxm config profiles-dir },
    # Copies one profile folder; a parameter only so tests can make the copy fail.
    [scriptblock]$Copier = { param($from, $to) Copy-Item -LiteralPath $from -Destination $to -Recurse }
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Source -PathType Container)) { throw "profiles source folder not found: $Source" }

function Get-ProfilesDir([scriptblock]$command, [string]$configDir) {
    $saved = $env:SBXM_CONFIG_DIR
    $global:LASTEXITCODE = 0
    try {
        if ($configDir) { $env:SBXM_CONFIG_DIR = $configDir }
        $output = @(& $command 2>&1)
        $code = $LASTEXITCODE
    }
    catch {
        throw "couldn't run sbxm to find profiles_dir ($_). Put sbxm on PATH or pass -ProfilesDirCommand; nothing was changed."
    }
    finally {
        $env:SBXM_CONFIG_DIR = $saved
    }
    if ($code -ne 0) {
        throw "sbxm config profiles-dir failed (exit code $code): $($output -join ' '); nothing was changed."
    }
    # Only the folder itself is a string; progress text from `cargo run` arrives as error records.
    # Trim only to tell an empty line from a folder: a folder name may start or end with spaces, and sbxm
    # reads it exactly as printed.
    $dir = $output | Where-Object { $_ -is [string] -and $_.Trim() } | Select-Object -First 1
    if (-not $dir) { throw 'sbxm config profiles-dir printed no folder; nothing was changed.' }
    $dir
}

# Every file (relative path and content hash) and every folder, empty ones included, so "unchanged" means the
# whole tree matches: a profile can point at an empty folder (`home_files`), and sbxm refuses it when it's
# missing. Compare the result with -ceq: a file renamed only in case is a change (-eq ignores case), so the
# entries are sorted ordinally too.
function Get-FolderState([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return $null }
    $root = (Resolve-Path -LiteralPath $dir).Path
    # -Force, or hidden files (dot files on Linux, the Hidden attribute on Windows) would be left out.
    $entries = [string[]]@(Get-ChildItem -LiteralPath $root -Recurse -Force | ForEach-Object {
            $relative = $_.FullName.Substring($root.Length)
            if ($_.PSIsContainer) { "D $relative" } else { 'F {0}={1}' -f $relative, (Get-FileHash -LiteralPath $_.FullName).Hash }
        })
    [Array]::Sort($entries, [StringComparer]::Ordinal)
    $entries -join "`n"
}

$profiles = @(Get-ChildItem -LiteralPath $Source -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'profile.toml') -PathType Leaf })
if (-not $profiles) { throw "no profiles (folders with a profile.toml) in $Source" }

$target = Get-ProfilesDir $ProfilesDirCommand $ConfigDir
New-Item -ItemType Directory -Force $target | Out-Null
Write-Host "Deploying to $target"
foreach ($profile in $profiles) {
    $dest = Join-Path $target $profile.Name
    $before = Get-FolderState $dest
    $state = if ($null -eq $before) { 'new' } elseif ($before -ceq (Get-FolderState $profile.FullName)) { 'unchanged' } else { 'updated' }
    if ($state -ne 'unchanged') {
        $id = [guid]::NewGuid().ToString('N')
        $staged = Join-Path $target ".deploy-$($profile.Name)-$id"
        $old = Join-Path $target ".old-$($profile.Name)-$id"
        try {
            & $Copier $profile.FullName $staged
            if ($state -eq 'updated') { Move-Item -LiteralPath $dest -Destination $old }
            try { Move-Item -LiteralPath $staged -Destination $dest }
            catch {
                if (Test-Path -LiteralPath $old) { Move-Item -LiteralPath $old -Destination $dest }
                throw
            }
            if (Test-Path -LiteralPath $old) { Remove-Item -LiteralPath $old -Recurse -Force }
        }
        finally {
            if (Test-Path -LiteralPath $staged) { Remove-Item -LiteralPath $staged -Recurse -Force }
        }
    }
    Write-Host "  $($profile.Name): $state"
}
