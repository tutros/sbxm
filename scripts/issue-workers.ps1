<#
.SYNOPSIS
Hands open GitHub issues to parallel Claude Code agents, one sbxm sandbox per issue (decisions 80-82).

.DESCRIPTION
start   Picks issues (or takes -Issue), clones the repo into <BaseDir>\sbxm-issue-<n> on branch issue-<n>,
        creates the sandbox with sbxm, and starts a headless agent in it. The host picks, so no two workers
        get the same issue: blocked issues (open "Depends on"), questions, and issues "Related" to one
        already in progress are skipped.
status  Shows each worker: agent running or finished, commits on its branch, and whether result.md and
        review.md exist.
review  After the agent finishes (decision 84): runs fmt, clippy and tests on the host, then a fresh reviewer in
        its own sandbox (sbxm-review-<n>, on its own clone) writes review.md. If it has must-fix findings, the
        worker gets one fix round and the review runs once more. Blocks until done; removes the reviewer after.
        With -Pr <n> it reviews an open PR's branch instead and posts the review as a PR comment, with no fix
        round (decisions 86, 87). PRs from forks are refused, since the host checks run their code.
        With -Issue and -GatesOnly it runs only the host checks on the worker's clone (log: gates.log), with no
        reviewer, sandbox or secret: the quick check after any change to the branch.
finish  Pushes branch issue-<n> and opens a PR that fixes the issue, with result.md and review.md in its body.
remove  Removes the sandbox and the clone (sbxm rm --purge asks you to confirm).

The sandboxes have no GitHub access: the agent reads the issue from .sbxm-issue/issue.md, commits locally,
and writes .sbxm-issue/result.md. Needs PowerShell 7, git, gh (logged in), cargo and sbxm on PATH.

.EXAMPLE
./scripts/issue-workers.ps1 start -Workers 2 -DryRun
./scripts/issue-workers.ps1 start -Workers 2
./scripts/issue-workers.ps1 start -Issue 1,2
./scripts/issue-workers.ps1 status
./scripts/issue-workers.ps1 review -Issue 1
./scripts/issue-workers.ps1 review -Issue 1 -GatesOnly
./scripts/issue-workers.ps1 review -Pr 12
./scripts/issue-workers.ps1 finish -Issue 1
./scripts/issue-workers.ps1 remove -Issue 1
#>
param(
    [Parameter(Position = 0, Mandatory)]
    [ValidateSet('start', 'status', 'review', 'finish', 'remove')]
    [string]$Action,
    [int[]]$Issue,
    [int]$Pr,
    [int]$Workers = 2,
    [string]$BaseDir = 'E:\sbxm-projects',
    [string]$SbxmProfile = 'sbxm-dev',
    [string]$Repo = 'tutros/sbxm',
    [string]$TimeLimit = '2h',
    # Only Claude's and Codex's headless commands are known; spike S5 covers Gemini and Pi (decision 85).
    [ValidateSet('claude', 'codex')]
    [string]$ReviewHarness = 'codex',
    # Default: gpt-5.6-sol for Codex, Claude Code's own default for Claude.
    [string]$ReviewModel,
    [string]$ReviewTimeLimit = '45m',
    [switch]$GatesOnly,
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
if (-not $ReviewModel -and $ReviewHarness -eq 'codex') { $ReviewModel = 'gpt-5.6-sol' }
# -Pr is a review target only, and an explicit one: never ignored, never mixed with -Issue.
if ($PSBoundParameters.ContainsKey('Pr')) {
    if ($Action -ne 'review') { throw "-Pr works only with review; use: review -Pr $Pr" }
    if ($Issue) { throw '-Pr and -Issue both name what to review; give one of them' }
    if ($Pr -lt 1) { throw "-Pr $Pr isn't a PR number; give a positive number" }
}
if ($GatesOnly) {
    if ($Action -ne 'review') { throw '-GatesOnly works only with review; use: review -Issue <n> -GatesOnly' }
    if ($PSBoundParameters.ContainsKey('Pr')) { throw '-GatesOnly checks a worker clone, not a PR; use: review -Issue <n> -GatesOnly' }
}

Import-Module (Join-Path $PSScriptRoot 'issue-workers.psm1') -Force
Set-WorkerConfig ([pscustomobject]@{
        BaseDir = $BaseDir; SbxmProfile = $SbxmProfile; Repo = $Repo; TimeLimit = $TimeLimit
        ReviewHarness = $ReviewHarness; ReviewModel = $ReviewModel; ReviewTimeLimit = $ReviewTimeLimit
        Workers = $Workers; Issue = $Issue
    })

switch ($Action) {
    'start' {
        foreach ($item in @(Select-Issues)) {
            if ($DryRun) { Write-Host "would start #$($item.Number): $($item.Title)" } else { Start-Worker $item }
        }
        if ($DryRun) { return }
        Write-Host "`nCheck progress with: ./scripts/issue-workers.ps1 status"
    }
    'status' { Show-Status }
    'review' {
        if ($PSBoundParameters.ContainsKey('Pr')) { Invoke-PrReview $Pr; return }
        if (-not $Issue) { throw 'review needs -Issue or -Pr' }
        foreach ($n in $Issue) { if ($GatesOnly) { Invoke-GatesOnly $n } else { Invoke-Review $n } }
    }
    'finish' {
        if (-not $Issue) { throw 'finish needs -Issue' }
        foreach ($n in $Issue) { Complete-Worker $n }
    }
    'remove' {
        if (-not $Issue) { throw 'remove needs -Issue' }
        foreach ($n in $Issue) { Invoke-Native 'sbxm rm' { sbxm rm (Get-ProjectName $n) --purge } }
    }
}
