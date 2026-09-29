# The functions behind issue-workers.ps1 (decision 91). The script sets the settings once with
# Set-WorkerConfig; tests set what they need. The bodies read them as module variables.
$ErrorActionPreference = 'Stop'
$labelOrder = @{ 'must-fix' = 0; 'should-fix' = 1 }

# The settings the functions read (the script's parameters, in one object).
function Set-WorkerConfig {
    param([Parameter(Mandatory)][pscustomobject]$Config)
    foreach ($name in 'BaseDir', 'SbxmProfile', 'Repo', 'TimeLimit', 'ReviewHarness', 'ReviewModel',
        'ReviewTimeLimit', 'Workers', 'Issue') {
        Set-Variable -Scope Script -Name $name -Value $Config.$name
    }
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

# The headless command for a harness, reading the prompt from a file (decision 85).
function Get-AgentCommand([string]$harness, [string]$model, [string]$promptFile) {
    $prompt = "`"`$(cat $promptFile)`""
    switch ($harness) {
        'claude' { "claude -p $prompt --output-format text" + ($model ? " --model $model" : '') }
        # Codex's default reasoning effort is low, too light for a review.
        'codex' { "codex exec" + ($model ? " -m $model" : '') + " -c 'model_reasoning_effort=`"high`"' $prompt" }
    }
}

# Writes <workspace>/<dir>/<name>, which runs an agent headless on <dir>/<prompt> (Claude unless $harness says
# otherwise), and returns its sandbox path.
# The time limit is enforced inside the sandbox: killing sbx exec on the host leaves the agent running.
function Write-RunScript([string]$workspace, [string]$dir, [string]$prompt, [string]$name, [string]$limit,
    [string]$harness = 'claude', [string]$model) {
    $sandboxWorkspace = ConvertTo-SandboxPath $workspace
    $run = @(
        '#!/usr/bin/env bash'
        "cd '$sandboxWorkspace' || exit 1"
        "timeout --kill-after=60s $limit $(Get-AgentCommand $harness $model "$dir/$prompt") < /dev/null"
        # Exits with the agent's status (124: timed out), so the host can tell a failed run from a finished one.
        'status=$?'
        'echo "agent exit code: $status"'
        'exit $status'
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
function Invoke-Gates([string]$label, [string]$workspace, [string]$log) {
    # Two jobs: with sandboxes running, a full parallel build ran the host out of memory (2026-09-28).
    $env:CARGO_BUILD_JOBS = '2'
    $gates = @(
        @('cargo fmt --check', @('fmt', '--check')),
        @('cargo clippy', @('clippy', '--all-targets', '--', '-D', 'warnings')),
        @('cargo test', @('test'))
    )
    foreach ($gate in $gates) {
        Write-Host "${label}: $($gate[0])"
        Push-Location $workspace
        try { & cargo @($gate[1]) *>> $log }
        finally { Pop-Location }
        if ($LASTEXITCODE -ne 0) { throw "${label}: $($gate[0]) failed on the host (exit code $LASTEXITCODE); see $log" }
    }
}

# $subject says what's reviewed and where its description (.sbxm-review/context.md) comes from.
function New-ReviewPrompt([string]$subject, [bool]$again) {
    $rereview = if ($again) {
        @"

This is a re-review: the author had one round to fix the earlier findings, in .sbxm-review/previous-review.md.
Check each earlier finding is fixed, and review the new commits too.
"@
    }
    @"
You are reviewing $subject of this repository ($Repo). Its description is in .sbxm-review/context.md; you have no
GitHub access. Someone else wrote the change, and you can't change it: don't edit tracked files, commit, push or
file issues. You may build and run tests.

Follow the sdlc-code-review skill (.claude/skills/sdlc-code-review/SKILL.md). Scope: the branch's commits
(git log origin/main..HEAD, and git diff origin/main...HEAD). Check the change against the acceptance criteria of
the issues in context.md (if any), decisions.md and the project conventions. origin/main is current and may be
newer than the branch's base: read decisions there (git show origin/main:decisions.md). The host has already run
cargo fmt --check, clippy and cargo test on Windows: they pass.
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

# Must-fix count from the reviewer's first line, or $null if that line isn't "Must-fix findings: <count>".
function Get-MustFixCount([string]$reviewFile) {
    $m = [regex]::Match((Get-Content -TotalCount 1 $reviewFile), '^Must-fix findings: (\d+)\s*$')
    if ($m.Success) { [int]$m.Groups[1].Value } else { $null }
}

# Clones $source (a path or URL) at $branch into the review project's workspace, with $context as context.md.
function Initialize-ReviewWorkspace([string]$reviewProject, [string]$source, [string]$branch, [string[]]$context) {
    $reviewWorkspace = Join-Path $BaseDir $reviewProject
    Invoke-Native 'git clone' { git clone -q -c core.autocrlf=false --branch $branch $source $reviewWorkspace }
    Add-Content (Join-Path $reviewWorkspace '.git/info/exclude') '.sbxm-review/'
    $reviewDir = Join-Path $reviewWorkspace '.sbxm-review'
    New-Item -ItemType Directory $reviewDir | Out-Null
    Set-Content (Join-Path $reviewDir 'context.md') $context
}

# Runs the reviewer on the review project's workspace (created on round 1), copies review.md, headed with the
# reviewer's harness and model, to $outDir as review.md and review-<round>.md, and returns its must-fix count.
# A failed or timed-out reviewer, or a review.md without its count line, throws: nothing is used or posted.
function Invoke-Reviewer([string]$label, [string]$reviewProject, [string]$subject, [int]$round, [string]$outDir) {
    $reviewWorkspace = Join-Path $BaseDir $reviewProject
    $reviewDir = Join-Path $reviewWorkspace '.sbxm-review'
    # A review.md left from an earlier run must never stand in for this one.
    Remove-Item (Join-Path $outDir 'review.md') -ErrorAction SilentlyContinue
    if ($round -gt 1) {
        Move-Item -Force (Join-Path $reviewDir 'review.md') (Join-Path $reviewDir 'previous-review.md')
    }
    Set-Content (Join-Path $reviewDir 'prompt.md') (New-ReviewPrompt $subject ($round -gt 1))
    $runScript = Write-RunScript $reviewWorkspace '.sbxm-review' 'prompt.md' 'run.sh' $ReviewTimeLimit `
        $ReviewHarness $ReviewModel
    if ($round -eq 1) {
        Invoke-Native 'sbxm new' { sbxm new $reviewProject --profile $SbxmProfile --harness $ReviewHarness } | Out-Host
    }

    $log = Join-Path $outDir "review-$round.log"
    Write-Host "${label}: review round $round (log: $log)"
    sbx exec "sbxm-$reviewProject-$ReviewHarness" bash $runScript *> $log
    $status = $LASTEXITCODE
    Save-Transcripts $label $reviewProject $outDir
    if ($status -ne 0) {
        throw "${label}: the reviewer exited with $status (124: it hit -ReviewTimeLimit), so its review isn't used; see $log"
    }
    $review = Join-Path $reviewDir 'review.md'
    if (-not (Test-Path $review)) { throw "${label}: the reviewer wrote no review.md; see $log" }
    $mustFix = Get-MustFixCount $review
    $reviewer = "Reviewer: $ReviewHarness ($($ReviewModel ? $ReviewModel : 'default model'))"
    $text = "$reviewer`n`n" + (Get-Content -Raw $review)
    Set-Content (Join-Path $outDir "review-$round.md") $text
    if ($null -eq $mustFix) {
        throw "${label}: review.md doesn't start with 'Must-fix findings: <count>', so it isn't used; see $outDir\review-$round.md"
    }
    Set-Content (Join-Path $outDir 'review.md') $text
    $mustFix
}

# Copies the reviewer's session transcripts to $outDir\transcripts, since the sandbox (and its home) is removed
# after the review. They go through the mounted workspace; a failed copy only warns.
function Save-Transcripts([string]$label, [string]$reviewProject, [string]$outDir) {
    $sessions = @{ claude = '~/.claude/projects'; codex = '~/.codex/sessions' }[$ReviewHarness]
    $reviewDir = Join-Path (Join-Path $BaseDir $reviewProject) '.sbxm-review'
    $copy = "rm -rf '$(ConvertTo-SandboxPath $reviewDir)/transcripts' && cp -r $sessions/. '$(ConvertTo-SandboxPath $reviewDir)/transcripts'"
    sbx exec "sbxm-$reviewProject-$ReviewHarness" bash -c $copy | Out-Host
    $source = Join-Path $reviewDir 'transcripts'
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path $source)) {
        Write-Warning "${label}: couldn't copy the reviewer's transcripts from $sessions"
        return
    }
    $target = Join-Path $outDir 'transcripts'
    Remove-Item -Recurse -Force $target -ErrorAction SilentlyContinue
    Copy-Item -Recurse $source $target
}

# The reviewer's provider secret must be stored before any host code runs. sbxm checks only the profile's
# secrets.services, and sbxm-dev names anthropic, which the Claude workers need.
function Assert-ReviewSecret {
    $service = @{ claude = 'anthropic'; codex = 'openai' }[$ReviewHarness]
    $secrets = (sbx secret ls --json | ConvertFrom-Json).secrets
    if ($LASTEXITCODE -ne 0) { throw 'sbx secret ls failed' }
    if (-not ($secrets | Where-Object { $_.scope -eq 'global' -and $_.type -eq 'service' -and $_.name -eq $service })) {
        throw "the $ReviewHarness reviewer needs the '$service' secret, which sbx doesn't have; add it with: sbx secret set $service"
    }
}

function Remove-ReviewProject([string]$reviewProject) {
    if (Test-Path (Join-Path $BaseDir $reviewProject)) {
        Invoke-Native 'sbxm rm' { sbxm rm $reviewProject --purge --yes }
    }
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
    Assert-ReviewSecret
    $log = Join-Path $issueDir 'gates.log'
    Remove-Item $log -ErrorAction SilentlyContinue
    $subject = "the change on branch issue-$n, made for GitHub issue #$n (the issue is context.md)"
    try {
        foreach ($round in 1, 2) {
            Invoke-Gates "#$n" $workspace $log
            if ($round -eq 1) {
                # Cloned from the worker's clone: the reviewer sees exactly the local commits, which aren't pushed yet.
                $issueText = Get-Content (Join-Path $issueDir 'issue.md')
                Initialize-ReviewWorkspace $reviewProject $workspace "issue-$n" $issueText
                # The worker's clone has main as of its start; the review compares against today's.
                Invoke-Native 'git fetch main' {
                    git -C (Join-Path $BaseDir $reviewProject) fetch -q "https://github.com/$Repo" '+main:refs/remotes/origin/main'
                }
            }
            else {
                $reviewWorkspace = Join-Path $BaseDir $reviewProject
                # Only the branch: fetching all of origin would reset origin/main to the worker's old main.
                Invoke-Native 'git fetch' { git -C $reviewWorkspace fetch -q origin "issue-$n" }
                Invoke-Native 'git reset' { git -C $reviewWorkspace reset -q --hard "origin/issue-$n" }
            }
            $mustFix = Invoke-Reviewer "#$n" $reviewProject $subject $round $issueDir
            Write-Host "#${n}: $mustFix must-fix finding(s)"
            if ($mustFix -eq 0 -or $round -eq 2) { break }
            Invoke-FixRound $n
        }
    }
    finally { Remove-ReviewProject $reviewProject }
    Write-Host "#${n}: review done; read $issueDir\review.md, then run finish"
}

# Reviews a PR's branch and posts the review as a PR comment (decision 87). No fix round: the author fixes and
# runs it again.
function Invoke-PrReview([int]$pr) {
    $label = "PR #$pr"
    $reviewProject = "sbxm-review-pr-$pr"
    $outDir = Join-Path $BaseDir "sbxm-pr-$pr-review"
    $info = gh pr view $pr --repo $Repo --json headRefName,isCrossRepository,state,title,body,closingIssuesReferences |
        ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "gh pr view $pr failed" }
    # The host gates run the PR's code (cargo test, build scripts) on this machine.
    if ($info.isCrossRepository) { throw "${label} comes from a fork; its code would run on this host, so it isn't reviewed here" }
    if ($info.state -ne 'OPEN') { throw "${label} is $($info.state.ToLower()), not open" }
    if (Test-Path (Join-Path $BaseDir $reviewProject)) {
        throw "${label}: $BaseDir\$reviewProject is left from an earlier review; remove it with: sbxm rm $reviewProject --purge"
    }
    Assert-ReviewSecret
    # The linked issues' acceptance criteria are what the reviewer checks against, so a failed lookup stops here.
    $context = @("# PR #${pr}: $($info.title)", '', $info.body)
    foreach ($ref in $info.closingIssuesReferences) {
        $issueText = gh issue view $ref.number --repo $Repo
        if ($LASTEXITCODE -ne 0) { throw "${label}: gh issue view $($ref.number) (an issue the PR closes) failed" }
        $context += @('', '---', '') + $issueText
    }
    New-Item -ItemType Directory -Force $outDir | Out-Null
    $log = Join-Path $outDir 'gates.log'
    Remove-Item $log -ErrorAction SilentlyContinue

    $subject = "pull request #$pr (branch $($info.headRefName))"
    try {
        Initialize-ReviewWorkspace $reviewProject "https://github.com/$Repo" $info.headRefName $context
        Invoke-Gates $label (Join-Path $BaseDir $reviewProject) $log
        $mustFix = Invoke-Reviewer $label $reviewProject $subject 1 $outDir
    }
    finally { Remove-ReviewProject $reviewProject }

    Write-Host "${label}: $mustFix must-fix finding(s)"
    $bodyFile = New-TemporaryFile
    Set-Content $bodyFile ("## Independent review (decision 87)`n`nHost checks (fmt, clippy, cargo test) passed on Windows.`n`n" +
        (Get-Content -Raw (Join-Path $outDir 'review.md')))
    try { Invoke-Native 'gh pr comment' { gh pr comment $pr --repo $Repo --body-file $bodyFile } }
    finally { Remove-Item $bodyFile }
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
