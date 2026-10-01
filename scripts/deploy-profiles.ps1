<#
.SYNOPSIS
Copies the repo's profiles (profiles/*) into the profiles_dir that sbxm reads.

.DESCRIPTION
Finds profiles_dir the way sbxm does: the `profiles_dir` key in <config dir>\config.toml, else
<config dir>\profiles, where <config dir> is -ConfigDir, $env:SBXM_CONFIG_DIR or ~/.config/sbxm. Like sbxm, it
refuses a config.toml with a key it doesn't know (a typo such as `profiles_dri` would otherwise deploy to the
default folder), and it reads `profiles_dir` as a TOML string, escapes included.

Each folder in the source that holds a profile.toml replaces the same-named folder there (files the repo no
longer has are removed). The new copy is staged beside the old one and swapped in only once it is complete, so a
failed copy leaves the working profile in place. Profiles that exist only in profiles_dir are left alone.
Existing sandboxes show config drift until they are rebuilt (`sbxm open <project> --harness <h> --rebuild`).

.EXAMPLE
./scripts/deploy-profiles.ps1
./scripts/deploy-profiles.ps1 -ConfigDir E:\other-config
#>
param(
    [string]$Source = (Join-Path $PSScriptRoot '..\profiles'),
    [string]$ConfigDir = $(if ($env:SBXM_CONFIG_DIR) { $env:SBXM_CONFIG_DIR } else { Join-Path $HOME '.config/sbxm' }),
    # Copies one profile folder; a parameter only so tests can make the copy fail.
    [scriptblock]$Copier = { param($from, $to) Copy-Item -LiteralPath $from -Destination $to -Recurse }
)

$ErrorActionPreference = 'Stop'
if (-not (Test-Path -LiteralPath $Source -PathType Container)) { throw "profiles source folder not found: $Source" }

# The top-level keys of sbxm's GlobalConfig (src/config.rs); `resources` is a table. `default_harness` is
# refused by sbxm too, so it isn't listed. Keep in step with that struct.
$knownKeys = 'base_dir', 'profiles_dir', 'default_profile', 'min_sbx_version', 'resources'

# Decodes the escapes a TOML basic string allows; anything else is an error, as it is for sbxm.
function ConvertFrom-TomlBasicString([string]$text) {
    $map = @{ '\' = '\'; '"' = '"'; 'b' = "`b"; 't' = "`t"; 'n' = "`n"; 'f' = "`f"; 'r' = "`r" }
    [regex]::Replace($text, '\\(u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8}|.)', {
            param($m)
            $e = $m.Groups[1].Value
            if ($e.Length -gt 1) { [char]::ConvertFromUtf32([Convert]::ToInt32($e.Substring(1), 16)) }
            elseif ($map.ContainsKey($e)) { $map[$e] }
            else { throw "profiles_dir in config.toml has an escape sbxm would refuse: \$e" }
        })
}

# The first segment of a TOML key (bare, "basic" or 'literal', possibly dotted: `a.b`, `"a".b`), or $null when
# the text before the `=` isn't a key. Returns the key's name and whether it is a dotted key.
function Get-TomlTopKey([string]$text) {
    $segment = '(?:[A-Za-z0-9_-]+|"(?:[^"\\]|\\.)*"|''[^'']*'')'
    if ($text -notmatch "^\s*($segment(?:\s*\.\s*$segment)*)\s*$") { return $null }
    $keyText = $Matches[1]
    $first = [regex]::Match($keyText, "^$segment").Value
    $name = switch ($first[0]) {
        '"' { ConvertFrom-TomlBasicString $first.Substring(1, $first.Length - 2) }
        "'" { $first.Substring(1, $first.Length - 2) }
        default { $first }
    }
    [pscustomobject]@{ Name = $name; Dotted = ($keyText.Length -gt $first.Length) }
}

function Get-ProfilesDir([string]$configDir) {
    $default = Join-Path $configDir 'profiles'
    $configFile = Join-Path $configDir 'config.toml'
    if (-not (Test-Path -LiteralPath $configFile)) { return $default }

    $dir = $null
    $inTable = $false
    foreach ($line in Get-Content -LiteralPath $configFile) {
        if ($line -match '^\s*\[') { $inTable = $true; continue }
        $eq = $line.IndexOf('=')
        if ($inTable -or $eq -lt 1) { continue }
        $parsed = Get-TomlTopKey $line.Substring(0, $eq)
        if (-not $parsed) { continue }
        $key = $parsed.Name; $value = $line.Substring($eq + 1).Trim()
        # A dotted `profiles_dir.x` would make it a table, which sbxm refuses too.
        if ($key -notin $knownKeys -or ($parsed.Dotted -and $key -ne 'resources')) { throw "config.toml has a key sbxm would refuse: '$key' (fix $configFile first; nothing was changed)" }
        if ($key -ne 'profiles_dir') { continue }
        if ($value -match "^'([^']*)'(\s*#.*)?$") { $dir = $Matches[1] }
        elseif ($value -match '^"((?:[^"\\]|\\.)*)"(\s*#.*)?$') { $dir = ConvertFrom-TomlBasicString $Matches[1] }
        else { throw "profiles_dir in $configFile isn't a quoted string deploy-profiles can read: $value" }
    }
    if ($dir) { $dir } else { $default }
}

# Hash every file by relative path, so "unchanged" means the whole folder matches.
function Get-FolderState([string]$dir) {
    if (-not (Test-Path -LiteralPath $dir)) { return $null }
    $root = (Resolve-Path -LiteralPath $dir).Path
    # -Force, or hidden files (dot files on Linux, the Hidden attribute on Windows) would be left out.
    (Get-ChildItem -LiteralPath $root -Recurse -File -Force | Sort-Object FullName | ForEach-Object {
        '{0}={1}' -f $_.FullName.Substring($root.Length), (Get-FileHash -LiteralPath $_.FullName).Hash
    }) -join "`n"
}

$target = Get-ProfilesDir $ConfigDir
$profiles = @(Get-ChildItem -LiteralPath $Source -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'profile.toml') })
if (-not $profiles) { throw "no profiles (folders with a profile.toml) in $Source" }

New-Item -ItemType Directory -Force $target | Out-Null
Write-Host "Deploying to $target"
foreach ($profile in $profiles) {
    $dest = Join-Path $target $profile.Name
    $before = Get-FolderState $dest
    $state = if ($null -eq $before) { 'new' } elseif ($before -eq (Get-FolderState $profile.FullName)) { 'unchanged' } else { 'updated' }
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
