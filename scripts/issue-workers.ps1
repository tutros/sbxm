<#
.SYNOPSIS
Hands open GitHub issues to parallel Claude Code agents, one sbxm sandbox per issue (decisions 80-82).

.DESCRIPTION
start   Picks issues (or takes -Issue), clones the repo into <BaseDir>\sbxm-issue-<n> on branch issue-<n>,
        creates the sandbox with sbxm, and starts a headless agent in it. The host picks, so no two workers
        get the same issue: blocked issues (open "Depends on"), questions, and issues "Related" to one
        already in progress are skipped.
status  Shows each worker: agent running or finished, commits on its branch, and whether result.md exists.
finish  Pushes branch issue-<n> and opens a PR that fixes the issue. Run it after reading result.md.
remove  Removes the sandbox and the clone (sbxm rm --purge asks you to confirm).

The sandboxes have no GitHub access: the agent reads the issue from .sbxm-issue/issue.md, commits locally,
and writes .sbxm-issue/result.md. Needs PowerShell 7, git, gh (logged in) and sbxm on PATH.

.EXAMPLE
./scripts/issue-workers.ps1 start -Workers 2 -DryRun
./scripts/issue-workers.ps1 start -Workers 2
./scripts/issue-workers.ps1 start -Issue 1,2
./scripts/issue-workers.ps1 status
./scripts/issue-workers.ps1 finish -Issue 1
./scripts/issue-workers.ps1 remove -Issue 1
#>
param(
    [Parameter(Position = 0, Mandatory)]
    [ValidateSet('start', 'status', 'finish', 'remove')]
    [string]$Action,
    [int[]]$Issue,
    [int]$Workers = 2,
    [string]$BaseDir = 'E:\sbxm-projects',
    [string]$SbxmProfile = 'sbxm-dev',
    [string]$Repo = 'tutros/sbxm',
    [string]$TimeLimit = '2h',
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$labelOrder = @{ 'must-fix' = 0; 'should-fix' = 1 }

function Invoke-Native {
    param([string]$What, [scriptblock]$Command)
    & $Command
    if ($LASTEXITCODE -ne 0) { throw "$What failed (exit code $LASTEXITCODE)" }
}

function Get-ProjectName([int]$n) { "sbxm-issue-$n" }
function Get-Workspace([int]$n) { Join-Path $BaseDir (Get-ProjectName $n) }
function Get-IssueDir([int]$n) { Join-Path (Get-Workspace $n) '.sbxm-issue' }

# sbx mounts E:\a\b at /e/a/b inside the sandbox.
function ConvertTo-SandboxPath([string]$path) {
    $full = [IO.Path]::GetFullPath($path)
    '/' + $full.Substring(0, 1).ToLower() + $full.Substring(2).Replace('\', '/')
}

function Get-IssueNumbers([string]$body, [string]$field) {
    $line = [regex]::Match($body, "(?m)^\*\*$field\:\*\*(.*)$")
    if (-not $line.Success) { return @() }
    [regex]::Matches($line.Groups[1].Value, '#(\d+)') | ForEach-Object { [int]$_.Groups[1].Value }
}

function Get-InProgress {
    Get-ChildItem -Directory $BaseDir -Filter 'sbxm-issue-*' -ErrorAction SilentlyContinue |
        ForEach-Object { [int]($_.Name -replace '^sbxm-issue-', '') }
}

function Select-Issues {
    $open = gh issue list --repo $Repo --state open --limit 200 --json number,title,labels,body | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'gh issue list failed' }
    $openNumbers = $open.number
    $taken = [Collections.Generic.List[int]]::new()
    Get-InProgress | ForEach-Object { $taken.Add($_) }
    # "Related" is often written on one of the two issues only, so look both ways.
    $relatedOf = @{}
    foreach ($o in $open) { $relatedOf[$o.number] = @(Get-IssueNumbers $o.body 'Related') }

    $candidates = $open | Where-Object { -not $Issue -or $Issue -contains $_.number } | ForEach-Object {
        $labels = $_.labels.name
        $rank = ($labels | ForEach-Object { $labelOrder[$_] } | Where-Object { $null -ne $_ } | Measure-Object -Minimum).Minimum
        [pscustomobject]@{
            Number     = $_.number
            Title      = $_.title
            IsQuestion = $labels -contains 'question'
            Rank       = $rank ?? 9
            DependsOn  = @(Get-IssueNumbers $_.body 'Depends on' | Sort-Object -Unique)
        }
    } | Sort-Object Rank, Number

    foreach ($n in $Issue) {
        if ($openNumbers -notcontains $n) { Write-Warning "#$n isn't an open issue; skipped" }
    }

    $picked = @()
    foreach ($c in $candidates) {
        if (-not $Issue -and $picked.Count -ge $Workers) { break }
        $blockers = $c.DependsOn | Where-Object { $openNumbers -contains $_ }
        $clash = $taken | Where-Object { $relatedOf[$c.Number] -contains $_ -or $relatedOf[$_] -contains $c.Number }
        if ($taken -contains $c.Number) { Write-Host "#$($c.Number): already has a worker; skipped" }
        elseif ($c.IsQuestion) { Write-Host "#$($c.Number): a question, needs your answer first; skipped" }
        elseif ($blockers) { Write-Host "#$($c.Number): blocked by open #$($blockers -join ', #'); skipped" }
        elseif ($clash) { Write-Host "#$($c.Number): related to #$($clash -join ', #'), which has a worker; skipped" }
        else {
            $picked += $c
            $taken.Add($c.Number)
        }
    }
    $picked
}

function New-Prompt([int]$n) {
    @"
You are working on GitHub issue #$n of this repository ($Repo). You have no GitHub access: the issue is in
.sbxm-issue/issue.md. You are on branch issue-$n.

Follow the sdlc-implementation skill: test first, smallest steps, and a commit after every green step
(cargo fmt --check, cargo clippy --all-targets -- -D warnings, cargo test). Work until every acceptance
criterion is met or you are blocked. Stay within this issue; don't push or change git remotes. Criteria you
can't check here (real sbx tests, anything needing GitHub) stay unticked, with the reason.

The last commit message must contain "Fixes #$n".

When you stop, write .sbxm-issue/result.md: each acceptance criterion as done or not done with its evidence
(the command and its trimmed output, or the test name), then anything blocked or left for the user.
"@
}

function Start-Worker($item) {
    $n = $item.Number
    $project = Get-ProjectName $n
    $workspace = Get-Workspace $n
    $issueDir = Get-IssueDir $n
    Write-Host "`n#${n}: $($item.Title)"

    Invoke-Native 'git clone' { git clone -q -c core.autocrlf=false "https://github.com/$Repo" $workspace }
    Invoke-Native 'git switch' { git -C $workspace switch -q -c "issue-$n" }
    # The sandbox doesn't see the host's global git config, so commits need a repo-level identity.
    foreach ($key in 'user.name', 'user.email') {
        $value = git config --global $key
        if ($value) { Invoke-Native "git config $key" { git -C $workspace config $key $value } }
    }
    Add-Content (Join-Path $workspace '.git/info/exclude') '.sbxm-issue/'
    New-Item -ItemType Directory $issueDir | Out-Null

    gh issue view $n --repo $Repo | Set-Content (Join-Path $issueDir 'issue.md')
    if ($LASTEXITCODE -ne 0) { throw "gh issue view $n failed" }
    Set-Content (Join-Path $issueDir 'prompt.md') (New-Prompt $n)
    $sandboxWorkspace = ConvertTo-SandboxPath $workspace
    # The time limit is enforced inside the sandbox: killing sbx exec on the host leaves the agent running.
    $run = @(
        '#!/usr/bin/env bash'
        "cd '$sandboxWorkspace' || exit 1"
        "timeout --kill-after=60s $TimeLimit claude -p `"`$(cat .sbxm-issue/prompt.md)`" --output-format text < /dev/null"
        'echo "agent exit code: $?"'
    ) -join "`n"
    [IO.File]::WriteAllText((Join-Path $issueDir 'run.sh'), "$run`n")

    Invoke-Native 'sbxm new' { sbxm new $project --profile $SbxmProfile }

    $proc = Start-Process sbx -PassThru -WindowStyle Hidden `
        -ArgumentList 'exec', "sbxm-$project-claude", 'bash', "$sandboxWorkspace/.sbxm-issue/run.sh" `
        -RedirectStandardOutput (Join-Path $issueDir 'agent.log') `
        -RedirectStandardError (Join-Path $issueDir 'agent.err.log')
    Set-Content (Join-Path $issueDir 'pid') $proc.Id
    Write-Host "#${n}: agent started (log: $issueDir\agent.log)"
}

function Show-Status {
    $numbers = if ($Issue) { $Issue } else { Get-InProgress | Sort-Object }
    if (-not $numbers) { Write-Host "No workers in $BaseDir."; return }
    foreach ($n in $numbers) {
        $workspace = Get-Workspace $n
        $issueDir = Get-IssueDir $n
        if (-not (Test-Path $workspace)) { Write-Host "#${n}: no worker"; continue }
        $pidFile = Join-Path $issueDir 'pid'
        $running = (Test-Path $pidFile) -and (Get-Process -Id (Get-Content $pidFile) -ErrorAction SilentlyContinue)
        $commits = @(git -C $workspace log --oneline "origin/main..issue-$n").Count
        $result = if (Test-Path (Join-Path $issueDir 'result.md')) { 'result.md written' } else { 'no result.md' }
        Write-Host ("#{0}: agent {1}, {2} commit(s), {3}" -f $n, ($running ? 'running' : 'finished'), $commits, $result)
    }
}

function Complete-Worker([int]$n) {
    $workspace = Get-Workspace $n
    if (-not (Test-Path $workspace)) { throw "#${n}: no worker at $workspace" }
    $commits = @(git -C $workspace log --oneline "origin/main..issue-$n").Count
    if ($commits -eq 0) { throw "#${n}: branch issue-$n has no commits" }
    $title = gh issue view $n --repo $Repo --json title --jq .title
    $resultFile = Join-Path (Get-IssueDir $n) 'result.md'
    $result = if (Test-Path $resultFile) { Get-Content -Raw $resultFile } else { '(the agent wrote no result.md)' }
    $bodyFile = New-TemporaryFile
    Set-Content $bodyFile "Fixes #$n`n`n$result"
    try {
        Invoke-Native 'git push' { git -C $workspace push -u origin "issue-$n" }
        Invoke-Native 'gh pr create' { gh pr create --repo $Repo --head "issue-$n" --title $title --body-file $bodyFile }
    }
    finally { Remove-Item $bodyFile }
}

switch ($Action) {
    'start' {
        foreach ($item in @(Select-Issues)) {
            if ($DryRun) { Write-Host "would start #$($item.Number): $($item.Title)" } else { Start-Worker $item }
        }
        if ($DryRun) { return }
        Write-Host "`nCheck progress with: ./scripts/issue-workers.ps1 status"
    }
    'status' { Show-Status }
    'finish' {
        if (-not $Issue) { throw 'finish needs -Issue' }
        foreach ($n in $Issue) { Complete-Worker $n }
    }
    'remove' {
        if (-not $Issue) { throw 'remove needs -Issue' }
        foreach ($n in $Issue) { Invoke-Native 'sbxm rm' { sbxm rm (Get-ProjectName $n) --purge } }
    }
}
