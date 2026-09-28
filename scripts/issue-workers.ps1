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
./scripts/issue-workers.ps1 finish -Issue 1
./scripts/issue-workers.ps1 remove -Issue 1
#>
param(
    [Parameter(Position = 0, Mandatory)]
    [ValidateSet('start', 'status', 'review', 'finish', 'remove')]
    [string]$Action,
    [int[]]$Issue,
    [int]$Workers = 2,
    [string]$BaseDir = 'E:\sbxm-projects',
    [string]$SbxmProfile = 'sbxm-dev',
    [string]$Repo = 'tutros/sbxm',
    [string]$TimeLimit = '2h',
    [string]$ReviewHarness = 'claude',
    [string]$ReviewTimeLimit = '45m',
    [switch]$DryRun
)

$ErrorActionPreference = 'Stop'
$labelOrder = @{ 'must-fix' = 0; 'should-fix' = 1 }
# Only Claude's headless command is known; spike S5 covers the other harnesses (decision 84).
if ($ReviewHarness -ne 'claude') {
    throw "-ReviewHarness ${ReviewHarness}: only claude can review until spike S5 finds the other harnesses' headless commands; use -ReviewHarness claude"
}

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

# Writes <workspace>/<dir>/<name>, which runs Claude headless on <dir>/<prompt>, and returns its sandbox path.
# The time limit is enforced inside the sandbox: killing sbx exec on the host leaves the agent running.
function Write-RunScript([string]$workspace, [string]$dir, [string]$prompt, [string]$name, [string]$limit) {
    $sandboxWorkspace = ConvertTo-SandboxPath $workspace
    $run = @(
        '#!/usr/bin/env bash'
        "cd '$sandboxWorkspace' || exit 1"
        "timeout --kill-after=60s $limit claude -p `"`$(cat $dir/$prompt)`" --output-format text < /dev/null"
        'echo "agent exit code: $?"'
    ) -join "`n"
    [IO.File]::WriteAllText((Join-Path $workspace "$dir/$name"), "$run`n")
    "$sandboxWorkspace/$dir/$name"
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
    $runScript = Write-RunScript $workspace '.sbxm-issue' 'prompt.md' 'run.sh' $TimeLimit

    Invoke-Native 'sbxm new' { sbxm new $project --profile $SbxmProfile }

    $proc = Start-Process sbx -PassThru -WindowStyle Hidden `
        -ArgumentList 'exec', "sbxm-$project-claude", 'bash', $runScript `
        -RedirectStandardOutput (Join-Path $issueDir 'agent.log') `
        -RedirectStandardError (Join-Path $issueDir 'agent.err.log')
    Set-Content (Join-Path $issueDir 'pid') $proc.Id
    Write-Host "#${n}: agent started (log: $issueDir\agent.log)"
}

function Test-AgentRunning([int]$n) {
    $pidFile = Join-Path (Get-IssueDir $n) 'pid'
    (Test-Path $pidFile) -and [bool](Get-Process -Id (Get-Content $pidFile) -ErrorAction SilentlyContinue)
}

# fmt, clippy and tests on the host (Windows), since the agents only test on Linux. Throws on the first failure.
function Invoke-Gates([int]$n, [string]$log) {
    $workspace = Get-Workspace $n
    # Two jobs: with sandboxes running, a full parallel build ran the host out of memory (2026-09-28).
    $env:CARGO_BUILD_JOBS = '2'
    $gates = @(
        @('cargo fmt --check', @('fmt', '--check')),
        @('cargo clippy', @('clippy', '--all-targets', '--', '-D', 'warnings')),
        @('cargo test', @('test'))
    )
    foreach ($gate in $gates) {
        Write-Host "#${n}: $($gate[0])"
        Push-Location $workspace
        try { & cargo @($gate[1]) *>> $log }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw "#${n}: $($gate[0]) failed on the host (exit code $LASTEXITCODE); see $log" }
    }
}

function New-ReviewPrompt([int]$n, [bool]$again) {
    $rereview = if ($again) {
        @"

This is a re-review: the author had one round to fix the earlier findings, in .sbxm-review/previous-review.md.
Check each earlier finding is fixed, and review the new commits too.
"@
    }
    @"
You are reviewing the change on branch issue-$n of this repository ($Repo), made for GitHub issue #$n. The issue
is in .sbxm-review/issue.md; you have no GitHub access. Someone else wrote the change, and you can't change it:
don't edit tracked files, commit, push or file issues. You may build and run tests.

Follow the sdlc-code-review skill. Scope: the branch's commits (git log origin/main..HEAD, and
git diff origin/main...HEAD). Check the change against the issue's acceptance criteria, decisions.md and the
project conventions. The host has already run cargo fmt --check, clippy and cargo test on Windows: they pass.
$rereview
Write .sbxm-review/review.md. Its first line is exactly "Must-fix findings: <count>". Then list each finding with
its rank (must-fix, should-fix or nit), file:line, evidence (a command and its trimmed output, or the quoted code)
and the fix you suggest. Only this change's problems count; put problems it didn't cause under
"Outside this change". If there are no findings, say what you checked.
"@
}

function New-FixPrompt([int]$n) {
    @"
A reviewer checked your change for issue #$n on branch issue-$n. The review is in .sbxm-issue/review.md.

Fix every must-fix finding, and each should-fix finding that stays within the issue. Same rules as before:
the sdlc-implementation skill, test first, a commit after every green step (cargo fmt --check,
cargo clippy --all-targets -- -D warnings, cargo test). Don't rewrite existing commits, push or change remotes.
If you think a finding is wrong, don't change the code for it; say why.

Then add a "Review" section to .sbxm-issue/result.md: each finding as fixed (with the commit) or not (with the
reason).
"@
}

# Must-fix count from the reviewer's first line, or $null if the line is missing.
function Get-MustFixCount([string]$reviewFile) {
    $m = [regex]::Match((Get-Content -Raw $reviewFile), '(?m)^Must-fix findings: (\d+)')
    if ($m.Success) { [int]$m.Groups[1].Value } else { $null }
}

function Invoke-Reviewer([int]$n, [int]$round) {
    $issueDir = Get-IssueDir $n
    $reviewProject = "sbxm-review-$n"
    $reviewWorkspace = Join-Path $BaseDir $reviewProject
    $reviewDir = Join-Path $reviewWorkspace '.sbxm-review'
    if ($round -eq 1) {
        # Cloned from the worker's clone: the reviewer sees exactly the local commits, which aren't pushed yet.
        Invoke-Native 'git clone' {
            git clone -q -c core.autocrlf=false --branch "issue-$n" (Get-Workspace $n) $reviewWorkspace
        }
        Add-Content (Join-Path $reviewWorkspace '.git/info/exclude') '.sbxm-review/'
        New-Item -ItemType Directory $reviewDir | Out-Null
        Copy-Item (Join-Path $issueDir 'issue.md') $reviewDir
    }
    else {
        Invoke-Native 'git fetch' { git -C $reviewWorkspace fetch -q origin }
        Invoke-Native 'git reset' { git -C $reviewWorkspace reset -q --hard "origin/issue-$n" }
        Move-Item -Force (Join-Path $reviewDir 'review.md') (Join-Path $reviewDir 'previous-review.md')
    }
    Set-Content (Join-Path $reviewDir 'prompt.md') (New-ReviewPrompt $n ($round -gt 1))
    $runScript = Write-RunScript $reviewWorkspace '.sbxm-review' 'prompt.md' 'run.sh' $ReviewTimeLimit
    if ($round -eq 1) {
        Invoke-Native 'sbxm new' { sbxm new $reviewProject --profile $SbxmProfile --harness $ReviewHarness }
    }

    Write-Host "#${n}: review round $round (log: $issueDir\review-$round.log)"
    sbx exec "sbxm-$reviewProject-$ReviewHarness" bash $runScript *> (Join-Path $issueDir "review-$round.log")
    $review = Join-Path $reviewDir 'review.md'
    if (-not (Test-Path $review)) { throw "#${n}: the reviewer wrote no review.md; see $issueDir\review-$round.log" }
    Copy-Item $review (Join-Path $issueDir "review-$round.md")
    Copy-Item $review (Join-Path $issueDir 'review.md')
}

function Invoke-FixRound([int]$n) {
    $workspace = Get-Workspace $n
    $issueDir = Get-IssueDir $n
    Set-Content (Join-Path $issueDir 'fix-prompt.md') (New-FixPrompt $n)
    $runScript = Write-RunScript $workspace '.sbxm-issue' 'fix-prompt.md' 'fix.sh' $TimeLimit
    Write-Host "#${n}: fix round (log: $issueDir\fix.log)"
    # sbx exec starts the worker's sandbox if it's stopped.
    sbx exec "sbxm-$(Get-ProjectName $n)-claude" bash $runScript *> (Join-Path $issueDir 'fix.log')
}

function Invoke-Review([int]$n) {
    $workspace = Get-Workspace $n
    $issueDir = Get-IssueDir $n
    $reviewProject = "sbxm-review-$n"
    if (-not (Test-Path $workspace)) { throw "#${n}: no worker at $workspace" }
    if (Test-AgentRunning $n) { throw "#${n}: the agent is still running; wait until status says finished" }
    if (Test-Path (Join-Path $BaseDir $reviewProject)) {
        throw "#${n}: $BaseDir\$reviewProject is left from an earlier review; remove it with: sbxm rm $reviewProject --purge"
    }
    $log = Join-Path $issueDir 'gates.log'
    Remove-Item $log -ErrorAction SilentlyContinue
    try {
        foreach ($round in 1, 2) {
            Invoke-Gates $n $log
            Invoke-Reviewer $n $round
            $mustFix = Get-MustFixCount (Join-Path $issueDir 'review.md')
            if ($null -eq $mustFix) {
                Write-Warning "#${n}: review.md has no 'Must-fix findings:' line; no fix round. Read it before finish."
                break
            }
            Write-Host "#${n}: $mustFix must-fix finding(s)"
            if ($mustFix -eq 0 -or $round -eq 2) { break }
            Invoke-FixRound $n
        }
    }
    finally {
        if (Test-Path (Join-Path $BaseDir $reviewProject)) {
            Invoke-Native 'sbxm rm' { sbxm rm $reviewProject --purge --yes }
        }
    }
    Write-Host "#${n}: review done; read $issueDir\review.md, then run finish"
}

function Show-Status {
    $numbers = if ($Issue) { $Issue } else { Get-InProgress | Sort-Object }
    if (-not $numbers) { Write-Host "No workers in $BaseDir."; return }
    foreach ($n in $numbers) {
        $workspace = Get-Workspace $n
        $issueDir = Get-IssueDir $n
        if (-not (Test-Path $workspace)) { Write-Host "#${n}: no worker"; continue }
        $running = Test-AgentRunning $n
        $commits = @(git -C $workspace log --oneline "origin/main..issue-$n").Count
        $result = if (Test-Path (Join-Path $issueDir 'result.md')) { 'result.md written' } else { 'no result.md' }
        $review = if (Test-Path (Join-Path $issueDir 'review.md')) { 'reviewed' } else { 'not reviewed' }
        Write-Host ("#{0}: agent {1}, {2} commit(s), {3}, {4}" -f $n, ($running ? 'running' : 'finished'), $commits, $result, $review)
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
    $reviewFile = Join-Path (Get-IssueDir $n) 'review.md'
    $review = if (Test-Path $reviewFile) { Get-Content -Raw $reviewFile } else {
        Write-Warning "#${n}: not reviewed; run review -Issue $n first unless you've reviewed it yourself"
        '(not reviewed)'
    }
    $bodyFile = New-TemporaryFile
    Set-Content $bodyFile "Fixes #$n`n`n$result`n`n## Independent review`n`n$review"
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
    'review' {
        if (-not $Issue) { throw 'review needs -Issue' }
        foreach ($n in $Issue) { Invoke-Review $n }
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
