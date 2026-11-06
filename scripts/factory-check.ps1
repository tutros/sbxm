<#
.SYNOPSIS
The scheduled factory check (decision 269, issue 327): starts an agent to prepare recurring work, and writes its
drafts to local files.

.DESCRIPTION
The agent only prepares: it reads what this script gathers on the host and writes a draft. Nothing here opens a
PR, sets a label, posts a comment or changes an issue.

Guards first, before any GitHub read: the run is skipped while any `sbxm-` sandbox runs (`sbx ls --json`) or a
`task loop` holds <base_dir>/.sbxm/task-loop.lock with a live process named sbxm*; a lock that can't be read skips
it too. It stops when the drive of sbx's sandbox state has less than min_free_disk free.

Then three jobs, each at most one agent run a day:
- readiness: open issues without the ready-for-worker, waiting-on-trigger or follow-up label (and never the living
  backlog #405) that have no issue-<n>.md draft yet, or whose updatedAt is newer than it; at most 5, oldest first.
- recheck: open ready-for-worker issues with PRs merged since the label was added (and since their last
  recheck-<n>.md draft); at most 5, oldest first (decision 295).
- retro: 10 or more merged PRs since the window end of the newest record in sdlc/retros/ (or no record at all).

Each job writes a spec, runs `sbxm task start --spec` from the dedicated checkout <base_dir>/factory/checkout (its
sbxm-task.toml is written from scripts/factory/sbxm-task.toml: no gates, the factory profile), reads draft.md out
of the task's repo.git with a hardened git, writes the drafts, then runs `sbxm task rm --spec --yes --force`.

Locations come from sbxm's config (base_dir): drafts and summary.md in <base_dir>/.sbxm/factory/drafts/<date>/,
specs and their run logs in <base_dir>/.sbxm/factory/specs/. summary.md gets one log line per draft (cost, time)
for sdlc/batches/<date>-factory.md.

The dedicated checkout is made once by hand: git clone https://github.com/<owner>/<repo>.git <base_dir>/factory/checkout

.EXAMPLE
./scripts/factory-check.ps1
./scripts/factory-check.ps1 -ConfigDir E:\other-config
#>
param(
    # sbxm's config folder, handed to sbxm as SBXM_CONFIG_DIR; SBXM_CONFIG_DIR, else ~/.config/sbxm.
    [string]$ConfigDir,
    # owner/name on GitHub; default: the dedicated checkout's origin.
    [string]$Repo,
    # The folder holding the spec templates and the checkout's sbxm-task.toml.
    [string]$TemplateDir = (Join-Path $PSScriptRoot 'factory'),
    # Where sbx keeps its sandboxes when config.toml has no sandbox_state_dir (as sbxm's own default).
    [string]$DefaultStateDir = $(if ($IsWindows -and $env:LOCALAPPDATA) { Join-Path $env:LOCALAPPDATA 'DockerSandboxes' }),
    [datetime]$Now = (Get-Date),
    # The commands, parameters only so tests can stand in for them.
    [scriptblock]$Sbxm = { sbxm @args },
    [scriptblock]$Sbx = { sbx @args },
    [scriptblock]$Gh = { gh @args },
    # git on the dedicated checkout (fetch and reads of origin/HEAD); the draft is read by Read-Draft.
    [scriptblock]$Git = { git @args },
    [scriptblock]$FreeSpace = { param($path) [IO.DriveInfo]::new($path).AvailableFreeSpace },
    # The name of the live process with this id, or nothing when there is none.
    [scriptblock]$ProcessName = { param($id) (Get-Process -Id $id -ErrorAction SilentlyContinue).ProcessName }
)

$ErrorActionPreference = 'Stop'
# A tool's exit code is read and reported here, not turned into an error by PowerShell.
$PSNativeCommandUseErrorActionPreference = $false

# Labels whose issues never get a readiness draft, and issues never drafted by number (the user, 2026-11-05).
$SkipLabels = @('ready-for-worker', 'waiting-on-trigger', 'follow-up')
$NeverDraft = @(405)
$ReadyLabel = 'ready-for-worker'
$MaxIssues = 5
$RetroDue = 10
# gh list calls ask for this many; a result this long could be cut off, so the run stops instead.
$ListLimit = 500

# ---- Running the tools ----------------------------------------------------------------------------------------

# Runs one of the command parameters; returns its standard output as one string, or throws with its error text.
# Error records (stderr) are kept apart, so JSON on stdout stays parseable.
function Invoke-Tool([string]$name, [scriptblock]$command, [string[]]$arguments, [switch]$AllowFailure) {
    $global:LASTEXITCODE = 0
    try { $all = @(& $command @arguments 2>&1) }
    catch { throw "couldn't run $name $($arguments -join ' '): $_" }
    $code = $LASTEXITCODE
    $out = @($all | Where-Object { $_ -isnot [System.Management.Automation.ErrorRecord] } | ForEach-Object { "$_" }) -join "`n"
    $err = @($all | Where-Object { $_ -is [System.Management.Automation.ErrorRecord] } | ForEach-Object { "$_" }) -join "`n"
    if ($code -ne 0 -and -not $AllowFailure) { throw "$name $($arguments -join ' ') failed (exit code $code): $err $out" }
    [pscustomobject]@{ Out = $out; Err = $err; Code = $code }
}

function Invoke-Gh([string[]]$arguments) { (Invoke-Tool 'gh' $Gh $arguments).Out }

function Invoke-GhJson([string[]]$arguments) {
    $text = Invoke-Gh $arguments
    if (-not $text.Trim()) { return @() }
    @($text | ConvertFrom-Json)
}

# A gh list result as long as the limit may be cut off, so it is refused rather than used.
function Assert-NotCut([object[]]$items, [string]$what) {
    if ($items.Count -ge $ListLimit) { throw "gh returned $($items.Count) $what, the limit; raise `$ListLimit in factory-check.ps1" }
}

# ConvertFrom-Json turns ISO dates into [datetime]; a string is parsed. Always UTC.
function ConvertTo-Utc($value) {
    if ($value -is [datetime]) { return $value.ToUniversalTime() }
    [datetimeoffset]::Parse([string]$value, [cultureinfo]::InvariantCulture).UtcDateTime
}

# ---- sbxm's config ----------------------------------------------------------------------------------------------

# The top-level string keys of config.toml (before its first table). sbxm has already validated the file
# (Read-SbxmConfig), so this reads only the keys the script needs: a basic "..." string with \\ and \" escapes,
# or a literal '...' string.
function Read-TopLevelStrings([string]$path) {
    $keys = @{}
    foreach ($line in [IO.File]::ReadAllLines($path)) {
        if ($line -match '^\s*\[') { break }
        if ($line -match '^\s*([A-Za-z0-9_-]+)\s*=\s*"((?:[^"\\]|\\.)*)"\s*(#.*)?$') {
            $key = $Matches[1]
            # Pairs are read left to right, so \\ followed by a letter is a backslash and that letter.
            $keys[$key] = [regex]::Replace($Matches[2], '\\(.)', {
                    param($m)
                    if ($m.Groups[1].Value -ceq '\' -or $m.Groups[1].Value -ceq '"') { return $m.Groups[1].Value }
                    throw "can't read $key in $path`: write it as a 'literal string'"
                })
        }
        elseif ($line -match "^\s*([A-Za-z0-9_-]+)\s*=\s*'([^']*)'\s*(#.*)?$") {
            $keys[$Matches[1]] = $Matches[2]
        }
    }
    $keys
}

# min_free_disk as sbxm reads it: "0", or a whole number with k, m, g or t (powers of 1024, either case).
function ConvertFrom-Size([string]$text) {
    if ($text -eq '0') { return [decimal]0 }
    if ($text -notmatch '^(\d+)([kKmMgGtT])$') { throw "min_free_disk `"$text`" isn't a size" }
    $shift = @{ k = 10; m = 20; g = 30; t = 40 }[$Matches[2].ToLowerInvariant()]
    [decimal]$Matches[1] * [decimal][math]::Pow(2, $shift)
}

# base_dir, min_free_disk and the sandbox state folder. sbxm reads config.toml first (`config profiles-dir`
# loads it), so an invalid one stops the run with sbxm's own message.
function Read-SbxmConfig {
    $dir = if ($ConfigDir) { $ConfigDir } elseif ($env:SBXM_CONFIG_DIR) { $env:SBXM_CONFIG_DIR } else { Join-Path $HOME '.config/sbxm' }
    $saved = $env:SBXM_CONFIG_DIR
    try {
        $env:SBXM_CONFIG_DIR = $dir
        $check = Invoke-Tool 'sbxm' $Sbxm @('config', 'profiles-dir') -AllowFailure
    }
    finally { $env:SBXM_CONFIG_DIR = $saved }
    if ($check.Code -ne 0) { throw "sbxm can't read its config (exit code $($check.Code)): $($check.Err) $($check.Out)" }
    $keys = Read-TopLevelStrings (Join-Path $dir 'config.toml')
    if (-not $keys['base_dir']) { throw "no base_dir in $(Join-Path $dir 'config.toml')" }
    $state = if ($keys['sandbox_state_dir']) { $keys['sandbox_state_dir'] } else { $DefaultStateDir }
    [pscustomobject]@{
        ConfigDir   = $dir
        BaseDir     = $keys['base_dir']
        MinFreeDisk = $(if ($null -ne $keys['min_free_disk']) { ConvertFrom-Size $keys['min_free_disk'] } else { [decimal]10GB })
        StateDir    = $state
    }
}

# ---- Guards -----------------------------------------------------------------------------------------------------

# Why the run is skipped (an sbxm task or the loop is running), or nothing.
function Get-SkipReason($config) {
    $ls = (Invoke-Tool 'sbx' $Sbx @('ls', '--json')).Out
    $sandboxes = @(($ls | ConvertFrom-Json).sandboxes)
    $running = @($sandboxes | Where-Object { "$($_.name)".StartsWith('sbxm-') -and $_.status -eq 'running' })
    if ($running) { return "sandbox $($running[0].name) is running" }

    $lock = Join-Path $config.BaseDir '.sbxm/task-loop.lock'
    if (-not (Test-Path -LiteralPath $lock)) { return $null }
    try {
        $held = [IO.File]::ReadAllText($lock) | ConvertFrom-Json
        $id = [int]$held.pid
        if ($id -le 0) { throw 'no process id' }
    }
    catch { return "the task loop lock $lock can't be read ($_)" }
    # A reused pid only skips one run, the safe direction; the start time isn't compared (the user, 2026-11-05).
    $name = & $ProcessName $id
    if ($name -and "$name" -like 'sbxm*') { return "a task loop is running (process $id)" }
    $null
}

# Why the run stops (too little free disk), or nothing. Like sbxm, an unknown or missing state folder isn't checked.
function Get-StopReason($config) {
    if ($config.MinFreeDisk -eq 0) { return $null }
    if (-not $config.StateDir -or -not (Test-Path -LiteralPath $config.StateDir)) {
        Write-Host "  sandbox drive not checked: no sandbox state folder (set sandbox_state_dir in config.toml)"
        return $null
    }
    $free = [decimal](& $FreeSpace $config.StateDir)
    if ($free -lt $config.MinFreeDisk) {
        return "only $free bytes free on the drive of $($config.StateDir), under min_free_disk ($($config.MinFreeDisk) bytes)"
    }
    $null
}

# ---- Drafts -----------------------------------------------------------------------------------------------------

# The newest write time (UTC) of <name> in any dated drafts folder, or nothing.
function Get-DraftTime([string]$draftsRoot, [string]$name) {
    if (-not (Test-Path -LiteralPath $draftsRoot)) { return $null }
    $times = @(Get-ChildItem -LiteralPath $draftsRoot -Directory |
            Where-Object { $_.Name -match '^\d{4}-\d{2}-\d{2}$' } |
            ForEach-Object { Join-Path $_.FullName $name } |
            Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
            ForEach-Object { (Get-Item -LiteralPath $_).LastWriteTimeUtc })
    if (-not $times) { return $null }
    @($times | Sort-Object)[-1]
}

# A fence longer than any backtick run in $text, so untrusted text can't close it.
function Format-Fenced([string]$text) {
    $longest = 0
    foreach ($m in [regex]::Matches($text, '`+')) { if ($m.Length -gt $longest) { $longest = $m.Length } }
    $fence = '`' * [math]::Max(3, $longest + 1)
    "$fence text`n$text`n$fence"
}

# An issue as the agent reads it: title, labels, dates, body and comments, the untrusted parts fenced.
function Format-Issue($issue) {
    $labels = @($issue.labels | ForEach-Object { $_.name }) -join ', '
    $text = "### Issue #$($issue.number): $($issue.title)`n`nLabels: $labels. Created $($issue.createdAt), updated $($issue.updatedAt).`n`n"
    $text += "Body:`n`n$(Format-Fenced "$($issue.body)")`n"
    foreach ($c in @($issue.comments)) {
        $text += "`nComment by $($c.author.login) on $($c.createdAt):`n`n$(Format-Fenced "$($c.body)")`n"
    }
    $text
}

function Get-Issue([int]$number) {
    Invoke-Gh @('issue', 'view', "$number", '--repo', $Repo, '--json', 'number,title,body,labels,createdAt,updatedAt,comments') | ConvertFrom-Json
}

# The spec template with its placeholders filled; the inputs go in last, so text in them is never replaced.
function New-SpecText([string]$job, [hashtable]$values, [string]$inputs) {
    $text = [IO.File]::ReadAllText((Join-Path $TemplateDir "$job.md"))
    foreach ($key in $values.Keys) { $text = $text.Replace("{{$key}}", [string]$values[$key]) }
    $text.Replace('{{inputs}}', $inputs)
}

# The parts of draft.md between `<!-- factory-draft: <name> -->` lines, by name, for the names asked only.
function Split-Draft([string]$draft, [string[]]$names) {
    $parts = [ordered]@{}
    $pieces = [regex]::Split($draft, '(?m)^<!-- factory-draft: ([a-z]+-\d+) -->[ \t]*\r?$')
    for ($i = 1; $i + 1 -lt $pieces.Count; $i += 2) {
        $name = $pieces[$i]
        if ($names -contains $name -and -not $parts.Contains($name)) { $parts[$name] = $pieces[$i + 1].Trim() + "`n" }
    }
    $parts
}

# draft.md on the task's branch, read from its host-owned repo.git with a git that reads no user or system config
# and no GIT_* variable, runs no textconv, and allows no protocol (as sbxm's own src/git.rs runs it); or nothing
# when the branch has no draft.md.
function Read-Draft([string]$repoGit, [string]$branch) {
    $info = [Diagnostics.ProcessStartInfo]::new('git')
    foreach ($a in @('--git-dir', $repoGit, '-c', 'core.fsmonitor=false', '-c', 'core.autocrlf=false', '-c', 'protocol.allow=never',
            '--no-pager', 'show', '--no-textconv', "refs/heads/$branch`:draft.md")) { $info.ArgumentList.Add($a) }
    foreach ($name in @($info.Environment.Keys)) { if ($name -like 'GIT_*') { [void]$info.Environment.Remove($name) } }
    $info.Environment['GIT_CONFIG_NOSYSTEM'] = '1'
    $info.Environment['GIT_CONFIG_GLOBAL'] = $(if ($IsWindows) { 'NUL' } else { '/dev/null' })
    $info.Environment['GIT_TERMINAL_PROMPT'] = '0'
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $info.UseShellExecute = $false
    $p = [Diagnostics.Process]::Start($info)
    $err = $p.StandardError.ReadToEndAsync()
    $out = $p.StandardOutput.ReadToEnd()
    $p.WaitForExit()
    if ($p.ExitCode -ne 0) { Write-Host "  no draft.md on $branch`: $($err.Result.Trim())"; return $null }
    $out
}

# The task folder `task start --spec $specPath` made: spec-<the file's sanitized name>-<hash> with that file name
# as its title. sbxm hashes the canonical path, which isn't recomputed here; the file name is unique per run.
function Find-Task([string]$baseDir, [string]$specPath) {
    $fileName = Split-Path $specPath -Leaf
    $stem = [IO.Path]::GetFileNameWithoutExtension($fileName).ToLowerInvariant() -replace '[^a-z0-9]+', '-'
    $root = Join-Path $baseDir '.sbxm/tasks'
    if (-not (Test-Path -LiteralPath $root)) { return $null }
    $found = @(Get-ChildItem -LiteralPath $root -Directory | Where-Object {
            $_.Name -match "^spec-$([regex]::Escape($stem.Trim('-')))-[0-9a-f]{6}$" -and
            (Test-Path -LiteralPath (Join-Path $_.FullName 'task.json'))
        } | Where-Object { ([IO.File]::ReadAllText((Join-Path $_.FullName 'task.json')) | ConvertFrom-Json).title -eq $fileName })
    if ($found.Count -gt 1) { throw "more than one task for $fileName under $root" }
    if ($found) { $found[0] } else { $null }
}

# One agent run: the spec in, the drafts out, the task removed once they are written. Returns the log rows.
function Invoke-DraftJob($ctx, [string]$job, [string]$specText, [System.Collections.IDictionary]$outputs) {
    $stamp = $Now.ToString('yyyyMMdd-HHmmss')
    $specs = Join-Path $ctx.Config.BaseDir '.sbxm/factory/specs'
    New-Item -ItemType Directory -Force $specs | Out-Null
    $specPath = Join-Path $specs "factory-$job-$stamp.md"
    [IO.File]::WriteAllText($specPath, $specText)
    Add-Summary $ctx "- $job`: ran (spec $specPath)"

    $started = Get-Date
    Push-Location -LiteralPath $ctx.Checkout
    try { $run = Invoke-Tool 'sbxm' $Sbxm @('task', 'start', '--spec', $specPath) -AllowFailure }
    finally { Pop-Location }
    [IO.File]::WriteAllText("$specPath.log", "exit code $($run.Code)`n$($run.Out)`n$($run.Err)")
    $elapsed = [int]((Get-Date) - $started).TotalSeconds

    $task = Find-Task $ctx.Config.BaseDir $specPath
    if (-not $task) { throw "task start made no task for $specPath (exit code $($run.Code); see $specPath.log)" }
    $record = [IO.File]::ReadAllText((Join-Path $task.FullName 'task.json')) | ConvertFrom-Json
    $branch = "$($record.branch)"
    if ($branch -notmatch '^[a-z0-9][a-z0-9-]*$') { throw "task $($task.Name) has an unusable branch name `"$branch`"" }
    $cost = $record.worker.run.usage.cost_usd
    $seconds = if ($null -ne $record.worker.run.duration_s) { [int]$record.worker.run.duration_s } else { $elapsed }

    $draft = Read-Draft (Join-Path $task.FullName 'repo.git') $branch
    if ($null -eq $draft) {
        throw "task $($task.Name) has no draft.md; it is kept for a look (remove it with: sbxm task rm --spec $specPath --yes --force)"
    }
    $parts = if ($outputs.Count -eq 1 -and $outputs.Contains('*')) { @{ '*' = $draft } } else { Split-Draft $draft @($outputs.Keys) }

    New-Item -ItemType Directory -Force $ctx.Today | Out-Null
    $rows = @()
    $costText = if ($null -ne $cost) { [string]::Format([cultureinfo]::InvariantCulture, '${0:F2}', [double]$cost) } else { 'unknown' }
    $share = if ($parts.Count -gt 1) { " (one run for $($parts.Count) drafts)" } else { '' }
    $missing = @()
    foreach ($key in $outputs.Keys) {
        $file = $outputs[$key]
        if (-not $parts.Contains($key)) { $missing += $file; continue }
        [IO.File]::WriteAllText((Join-Path $ctx.Today $file), $parts[$key])
        $rows += "| $($ctx.Date) | $file | $job | $($task.Name) | $costText$share | $([math]::Floor($seconds / 60))m $($seconds % 60)s | | |"
    }
    if ($missing) {
        foreach ($row in $rows) { Add-Summary $ctx $row }
        throw "draft.md of task $($task.Name) has no section for $($missing -join ', '); the task is kept for a look (remove it with: sbxm task rm --spec $specPath --yes --force)"
    }

    # Every draft is written, so removing the task loses nothing (its result is read out, never fetched, hence --force).
    Push-Location -LiteralPath $ctx.Checkout
    try { $rm = Invoke-Tool 'sbxm' $Sbxm @('task', 'rm', '--spec', $specPath, '--yes', '--force') -AllowFailure }
    finally { Pop-Location }
    if ($rm.Code -ne 0) { Add-Summary $ctx "  - task rm failed for $($task.Name) (exit code $($rm.Code)): $($rm.Err) $($rm.Out)" }
    $rows
}

# ---- Jobs -------------------------------------------------------------------------------------------------------

function Get-ReadinessCandidates($ctx) {
    $issues = Invoke-GhJson @('issue', 'list', '--repo', $Repo, '--state', 'open', '--limit', "$ListLimit", '--json', 'number,title,labels,createdAt,updatedAt')
    Assert-NotCut $issues 'open issues'
    @($issues | Where-Object {
            $names = @($_.labels | ForEach-Object { $_.name })
            -not @($names | Where-Object { $SkipLabels -contains $_ }) -and $NeverDraft -notcontains [int]$_.number
        } | Where-Object {
            $drafted = Get-DraftTime $ctx.DraftsRoot "issue-$($_.number).md"
            $null -eq $drafted -or (ConvertTo-Utc $_.updatedAt) -gt $drafted
        } | Sort-Object { ConvertTo-Utc $_.createdAt }, { [int]$_.number } | Select-Object -First $MaxIssues)
}

function Invoke-Readiness($ctx) {
    $chosen = @(Get-ReadinessCandidates $ctx)
    if (-not $chosen) { Add-Summary $ctx '- readiness: no issue needs a draft'; return @() }
    $issues = @($chosen | ForEach-Object { Get-Issue ([int]$_.number) })
    $outputs = [ordered]@{}
    foreach ($i in $issues) { $outputs["issue-$($i.number)"] = "issue-$($i.number).md" }
    $markers = @($outputs.Keys | ForEach-Object { "<!-- factory-draft: $_ -->" }) -join "`n"
    $spec = New-SpecText 'readiness' @{ date = $ctx.Date; repo = $Repo; markers = $markers } (@($issues | ForEach-Object { Format-Issue $_ }) -join "`n")
    Invoke-DraftJob $ctx 'readiness' $spec $outputs
}

# The date the ready-for-worker label was last added, from the issue's timeline, or nothing. Read a page at a
# time: --paginate prints one JSON array per page, which isn't one JSON document.
function Get-LabelTime([int]$number) {
    $times = @()
    for ($page = 1; $page -le 50; $page++) {
        $events = @(Invoke-GhJson @('api', "repos/$Repo/issues/$number/timeline?per_page=100&page=$page"))
        $times += @($events | Where-Object { $_.event -eq 'labeled' -and $_.label.name -eq $ReadyLabel } | ForEach-Object { ConvertTo-Utc $_.created_at })
        if ($events.Count -lt 100) { break }
    }
    if (-not $times) { return $null }
    @($times | Sort-Object)[-1]
}

function Format-Prs([object[]]$prs, [switch]$Files) {
    if (-not $prs) { return '(none)' }
    @($prs | Sort-Object { ConvertTo-Utc $_.mergedAt } | ForEach-Object {
            $line = "- #$($_.number) $($_.title) (merged $($_.mergedAt))"
            if ($Files) { $line += ': ' + (@($_.files | ForEach-Object { $_.path }) -join ', ') }
            $line
        }) -join "`n"
}

function Invoke-Recheck($ctx) {
    $ready = Invoke-GhJson @('issue', 'list', '--repo', $Repo, '--state', 'open', '--label', $ReadyLabel, '--limit', "$ListLimit", '--json', 'number,title,createdAt,updatedAt')
    Assert-NotCut $ready 'ready issues'
    if (-not $ready) { Add-Summary $ctx '- recheck: no ready issue'; return @() }
    $since = @{}
    foreach ($i in $ready) {
        $labeled = Get-LabelTime ([int]$i.number)
        if ($null -eq $labeled) { Add-Summary $ctx "  - recheck: #$($i.number) has no $ReadyLabel label event; left out"; continue }
        $drafted = Get-DraftTime $ctx.DraftsRoot "recheck-$($i.number).md"
        $since[[int]$i.number] = if ($null -ne $drafted -and $drafted -gt $labeled) { $drafted } else { $labeled }
    }
    if (-not $since.Count) { return @() }
    $earliest = @(@($since.Values) | Sort-Object)[0]
    $prs = Invoke-GhJson @('pr', 'list', '--repo', $Repo, '--state', 'merged', '--search', "merged:>=$($earliest.ToString('yyyy-MM-dd'))",
        '--limit', "$ListLimit", '--json', 'number,title,mergedAt,files')
    Assert-NotCut $prs 'merged PRs'
    $chosen = @($ready | Where-Object { $since.ContainsKey([int]$_.number) } | Where-Object {
            $from = $since[[int]$_.number]
            @($prs | Where-Object { (ConvertTo-Utc $_.mergedAt) -gt $from }).Count -gt 0
        } | Sort-Object { ConvertTo-Utc $_.createdAt }, { [int]$_.number } | Select-Object -First $MaxIssues)
    if (-not $chosen) { Add-Summary $ctx '- recheck: no PR merged under a ready issue'; return @() }

    $outputs = [ordered]@{}
    $inputs = @()
    foreach ($c in $chosen) {
        $issue = Get-Issue ([int]$c.number)
        $from = $since[[int]$c.number]
        $merged = @($prs | Where-Object { (ConvertTo-Utc $_.mergedAt) -gt $from })
        $outputs["recheck-$($issue.number)"] = "recheck-$($issue.number).md"
        $inputs += (Format-Issue $issue) + "`nPRs merged since $($from.ToString('u')):`n`n$(Format-Prs $merged -Files)`n"
    }
    $markers = @($outputs.Keys | ForEach-Object { "<!-- factory-draft: $_ -->" }) -join "`n"
    $spec = New-SpecText 'recheck' @{ date = $ctx.Date; repo = $Repo; markers = $markers } ($inputs -join "`n")
    Invoke-DraftJob $ctx 'recheck' $spec $outputs
}

# The newest record in sdlc/retros/ on the checkout's origin/HEAD, and the last day of its window (its Window:
# line's end, else the date in its name); nothing when there is no record.
function Get-LastRetro($ctx) {
    $null = Invoke-Tool 'git' $Git @('-C', $ctx.Checkout, 'fetch', '--quiet', 'origin')
    $names = @((Invoke-Tool 'git' $Git @('-C', $ctx.Checkout, 'ls-tree', '--name-only', 'origin/HEAD', 'sdlc/retros/')).Out -split "`n" |
            ForEach-Object { $_.Trim() } | Where-Object { $_ -match '^sdlc/retros/\d{4}-\d{2}-\d{2}-retro\.md$' } | Sort-Object)
    if (-not $names) { return $null }
    $newest = $names[-1]
    $text = (Invoke-Tool 'git' $Git @('-C', $ctx.Checkout, 'show', "origin/HEAD:$newest")).Out
    $end = if ($text -match '(?m)^Window: (\d{4}-\d{2}-\d{2}) to (\d{4}-\d{2}-\d{2})') { $Matches[2] } else { (Split-Path $newest -Leaf).Substring(0, 10) }
    [pscustomobject]@{ Path = $newest; End = $end }
}

function Invoke-Retro($ctx) {
    $last = Get-LastRetro $ctx
    $file = if ($last) { "retro-after-$($last.End).md" } else { 'retro-first.md' }
    if ($null -ne (Get-DraftTime $ctx.DraftsRoot $file)) { Add-Summary $ctx "- retro: $file is already drafted"; return @() }
    # With no record, the latest 100 merged PRs are the agent's input; it proposes the window.
    $search = @('pr', 'list', '--repo', $Repo, '--state', 'merged', '--json', 'number,title,mergedAt,url')
    if ($last) { $search += @('--limit', "$ListLimit", '--search', "merged:>$($last.End)") } else { $search += @('--limit', '100') }
    $prs = @(Invoke-GhJson $search)
    if ($last) { Assert-NotCut $prs 'merged PRs' }
    if ($last -and $prs.Count -lt $RetroDue) { Add-Summary $ctx "- retro: not due ($($prs.Count) PRs merged since $($last.End))"; return @() }
    $window = if ($last) { "the day after $($last.End) (the window end of $($last.Path)) to $($ctx.Date)" } else { "no retro record exists yet; propose a window start, to $($ctx.Date)" }
    $spec = New-SpecText 'retro' @{ date = $ctx.Date; repo = $Repo; window = $window; count = "$($prs.Count)" } (Format-Prs $prs)
    Invoke-DraftJob $ctx 'retro' $spec ([ordered]@{ '*' = $file })
}

# ---- The run ----------------------------------------------------------------------------------------------------

function Add-Summary($ctx, [string]$line) {
    New-Item -ItemType Directory -Force $ctx.Today | Out-Null
    Add-Content -LiteralPath $ctx.Summary -Value $line
    Write-Host $line
}

# Whether this job already ran an agent today: at most one run per job per day (decision 269(f)).
function Test-RanToday($ctx, [string]$job) {
    (Test-Path -LiteralPath $ctx.Summary) -and @(Select-String -LiteralPath $ctx.Summary -Pattern "^- $job`: ran\b").Count -gt 0
}

$config = Read-SbxmConfig
$draftsRoot = Join-Path $config.BaseDir '.sbxm/factory/drafts'
$date = $Now.ToString('yyyy-MM-dd')
$ctx = [pscustomobject]@{
    Config     = $config
    Date       = $date
    DraftsRoot = $draftsRoot
    Today      = Join-Path $draftsRoot $date
    Summary    = Join-Path (Join-Path $draftsRoot $date) 'summary.md'
    Checkout   = Join-Path $config.BaseDir 'factory/checkout'
}

Add-Summary $ctx "`n## Run at $($Now.ToString('HH:mm:ss'))`n"
$skip = Get-SkipReason $config
if ($skip) { Add-Summary $ctx "- skipped: $skip"; exit 0 }
$stop = Get-StopReason $config
if ($stop) { Add-Summary $ctx "- stopped: $stop"; exit 0 }

if (-not (Test-Path -LiteralPath (Join-Path $ctx.Checkout '.git'))) {
    throw "no dedicated checkout at $($ctx.Checkout); make it once with: git clone https://github.com/<owner>/<repo>.git $($ctx.Checkout)"
}
if (-not $Repo) {
    $url = (Invoke-Tool 'git' $Git @('-C', $ctx.Checkout, 'remote', 'get-url', 'origin')).Out.Trim()
    if ($url -notmatch 'github\.com[:/]([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+?)(\.git)?/?$') { throw "the checkout's origin $url isn't a GitHub repo; pass -Repo owner/name" }
    $Repo = $Matches[1]
}
Copy-Item -LiteralPath (Join-Path $TemplateDir 'sbxm-task.toml') -Destination (Join-Path $ctx.Checkout 'sbxm-task.toml') -Force

$rows = @()
$failed = $false
foreach ($job in @('readiness', 'recheck', 'retro')) {
    if (Test-RanToday $ctx $job) { Add-Summary $ctx "- $job`: already ran today"; continue }
    try {
        $rows += @(switch ($job) {
                'readiness' { Invoke-Readiness $ctx }
                'recheck' { Invoke-Recheck $ctx }
                'retro' { Invoke-Retro $ctx }
            })
    }
    catch {
        $failed = $true
        Add-Summary $ctx "- $job`: failed: $_"
    }
}
if ($rows) {
    Add-Summary $ctx "`nLog lines for sdlc/batches/$date-factory.md (the last two columns are filled in when the draft is worked through):`n"
    Add-Summary $ctx '| Date | Draft | Job | Task | Cost | Time | Recommended answers accepted unchanged | Draft to ready-for-worker |'
    Add-Summary $ctx '|---|---|---|---|---|---|---|---|'
    foreach ($row in $rows) { Add-Summary $ctx $row }
}
if ($failed) { exit 1 }
