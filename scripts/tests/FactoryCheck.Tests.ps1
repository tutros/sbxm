BeforeAll {
    $script:check = Join-Path $PSScriptRoot '..\factory-check.ps1'
    $script:templates = Join-Path $PSScriptRoot '..\factory'

    # A stand-in command: logs each call's arguments as one line to $log, and answers from $answers with the
    # first pattern (in order) that matches the joined arguments. An answer is text, @{ out = ...; code = ... },
    # or a scriptblock given the arguments that returns one of those. With $envLog, each call also logs the
    # SBXM_CONFIG_DIR it sees there, as `<arguments> => <value>`.
    function New-Tool([string]$log, [System.Collections.IDictionary]$answers, [string]$envLog) {
        {
            $line = $args -join ' '
            Add-Content -LiteralPath $log -Value $line
            if ($envLog) { Add-Content -LiteralPath $envLog -Value "$line => $env:SBXM_CONFIG_DIR" }
            foreach ($pattern in $answers.Keys) {
                if ($line -match $pattern) {
                    $answer = $answers[$pattern]
                    if ($answer -is [scriptblock]) { $answer = & $answer @args }
                    if ($answer -is [hashtable]) { $global:LASTEXITCODE = $answer.code; return $answer.out }
                    $global:LASTEXITCODE = 0
                    return $answer
                }
            }
            throw "unexpected call: $line"
        }.GetNewClosure()
    }

    function Get-Log([string]$log) { if (Test-Path -LiteralPath $log) { @(Get-Content -LiteralPath $log) } else { @() } }

    # What `sbxm task start --spec` leaves behind: the task folder with task.json and a repo.git whose task
    # branch has draft.md (one section per marker line the spec asks for).
    function New-FakeTask([string]$base, [string]$specPath, [string]$draft) {
        $name = [IO.Path]::GetFileNameWithoutExtension($specPath)
        $id = "spec-$name-abc123"
        $dir = Join-Path $base ".sbxm/tasks/$id"
        New-Item -ItemType Directory -Force $dir | Out-Null
        $markers = @(Get-Content -LiteralPath $specPath | Where-Object { $_ -match '^<!-- factory-draft: [a-z]+-\d+ -->$' })
        if (-not $draft) { $draft = if ($markers) { ($markers | ForEach-Object { "$_`nDraft for $_" }) -join "`n" } else { 'The retro draft' } }
        $repoGit = Join-Path $dir 'repo.git'
        git init --quiet --bare $repoGit
        $blob = $draft | git --git-dir $repoGit hash-object -w --stdin
        $tree = "100644 blob $blob`tdraft.md" | git --git-dir $repoGit mktree
        $commit = git --git-dir $repoGit -c user.name=t -c user.email=t@example.com commit-tree $tree -m draft
        git --git-dir $repoGit update-ref "refs/heads/$id" $commit
        @{ title = (Split-Path $specPath -Leaf); branch = $id; worker = @{ run = @{ usage = @{ cost_usd = 0.5 }; duration_s = 125 } } } |
            ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $dir 'task.json')
    }

    function ConvertTo-IssueJson([object[]]$issues) { ConvertTo-Json -InputObject @($issues) -Depth 5 }

    function New-ListedIssue([int]$number, [string]$created, [string[]]$labels = @(), [string]$updated = $created) {
        @{ number = $number; title = "Issue $number"; labels = @($labels | ForEach-Object { @{ name = $_ } }); createdAt = $created; updatedAt = $updated }
    }

    function New-Prs([int]$count, [string]$merged = '2026-11-05T12:00:00Z') {
        ConvertTo-Json -InputObject @(1..$count | Where-Object { $count -gt 0 } | ForEach-Object {
                @{ number = 500 + $_; title = "PR $_"; mergedAt = $merged; url = "https://example.com/$_"; files = @(@{ path = 'src/x.rs' }) }
            }) -Depth 5
    }

    function Invoke-Check([hashtable]$extra = @{}) {
        $params = @{
            ConfigDir   = $script:cfg
            Repo        = 'owner/repo'
            TemplateDir = $script:templates
            Now         = [datetime]'2026-11-06T07:00:00'
            Sbxm        = New-Tool $script:sbxmLog $script:sbxmAnswers $script:sbxmEnvLog
            Sbx         = New-Tool $script:sbxLog $script:sbxAnswers
            Gh          = New-Tool $script:ghLog $script:ghAnswers
            Git         = New-Tool $script:gitLog $script:gitAnswers
            FreeSpace   = $script:freeSpace
            ProcessName = $script:processName
        }
        foreach ($key in $extra.Keys) { $params[$key] = $extra[$key] }
        & $script:check @params 6>$null
    }

    function Get-Starts([string]$job) { @(Get-Log $script:sbxmLog | Where-Object { $_ -match "^task start --spec .*factory-$job-" }) }
}

Describe 'factory-check.ps1' {
    BeforeEach {
        $root = Join-Path $TestDrive ([guid]::NewGuid())
        $script:base = Join-Path $root 'base'
        $script:cfg = Join-Path $root 'config'
        $state = Join-Path $root 'sbx-state'
        New-Item -ItemType Directory $script:cfg, $state, (Join-Path $script:base 'factory/checkout/.git') | Out-Null
        Set-Content -LiteralPath (Join-Path $script:cfg 'config.toml') -Value @(
            "base_dir = '$($script:base)'"
            "min_free_disk = '1g'   # sbxm's own format"
            "sandbox_state_dir = '$state'"
            ''
            '[resources]'
            'cpus = 2'
        )
        $script:drafts = Join-Path $script:base '.sbxm/factory/drafts'
        $script:today = Join-Path $script:drafts '2026-11-06'
        $script:specs = Join-Path $script:base '.sbxm/factory/specs'

        $script:sbxmLog = Join-Path $root 'sbxm.log'
        $script:sbxmEnvLog = Join-Path $root 'sbxm-env.log'
        $script:sbxLog = Join-Path $root 'sbx.log'
        $script:ghLog = Join-Path $root 'gh.log'
        $script:gitLog = Join-Path $root 'git.log'
        # A closure doesn't see this file's functions, so it is handed the one it calls.
        $fakeBase = $script:base
        $makeTask = ${function:New-FakeTask}
        $script:sbxmAnswers = [ordered]@{
            '^config profiles-dir$' = 'profiles'
            '^task start --spec '   = { & $makeTask $fakeBase $args[3]; 'started' }.GetNewClosure()
            '^task rm --spec '      = 'removed'
        }
        $script:sbxAnswers = [ordered]@{ '^ls --json$' = '{"sandboxes": []}' }
        $script:ghAnswers = [ordered]@{
            '^issue list .*--label ready-for-worker' = '[]'
            '^issue list '                           = '[]'
            '^issue view (\d+) '                     = {
                $n = [int]$args[2]
                ConvertTo-Json -Depth 5 -InputObject @{ number = $n; title = "Issue $n"; body = "Body of $n"; labels = @(); createdAt = '2026-10-01T00:00:00Z'; updatedAt = '2026-10-01T00:00:00Z'; comments = @() }
            }
            '^api repos/owner/repo/issues/\d+/timeline' = '[]'
            '^pr list .*files'                       = '[]'
            '^pr list '                              = (New-Prs 0)
        }
        $script:gitAnswers = [ordered]@{
            ' fetch '                      = ''
            ' ls-tree '                    = 'sdlc/retros/2026-10-30-retro.md' + "`n" + 'sdlc/retros/2026-11-04-retro.md'
            ' show origin/HEAD:sdlc/retros/2026-11-04-retro.md$' = "# Retro`n`nWindow: 2026-10-31 to 2026-11-04`n"
        }
        $script:freeSpace = { param($path) [decimal]100GB }
        $script:processName = { param($id) $null }
    }

    Context 'readiness candidates' {
        It 'runs no agent when no issue needs a draft' {
            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'readiness: no issue needs a draft'
        }

        It 'drafts all 5 candidates in one agent run and removes the task after the drafts are written' {
            $issues = 1..5 | ForEach-Object { New-ListedIssue $_ "2026-10-0$($_)T00:00:00Z" }
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson $issues
            $todayDir = $script:today
            $seen = Join-Path $TestDrive "seen-$([guid]::NewGuid())"
            $script:sbxmAnswers['^task rm --spec '] = { Set-Content -LiteralPath $seen (Test-Path -LiteralPath (Join-Path $todayDir 'issue-5.md')); 'removed' }.GetNewClosure()

            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 1
            foreach ($n in 1..5) { Get-Content -LiteralPath (Join-Path $script:today "issue-$n.md") -Raw | Should -Match "Draft for <!-- factory-draft: issue-$n -->" }
            @(Get-Log $script:sbxmLog | Where-Object { $_ -match '^task rm --spec .*factory-readiness-20261106-070000\.md --yes --force$' }) | Should -HaveCount 1
            Get-Content -LiteralPath $seen | Should -Be 'True'
        }

        It 'drafts 5 of 6 candidates, oldest by creation date first' {
            # Numbers run against the dates, so the order can only come from createdAt.
            $issues = 1..6 | ForEach-Object { New-ListedIssue (70 - 10 * $_) "2026-10-0$($_)T00:00:00Z" }
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson $issues

            Invoke-Check

            foreach ($n in 60, 50, 40, 30, 20) { Test-Path -LiteralPath (Join-Path $script:today "issue-$n.md") | Should -BeTrue }
            Test-Path -LiteralPath (Join-Path $script:today 'issue-10.md') | Should -BeFalse
            @(Get-Log $script:ghLog | Where-Object { $_ -match '^issue view ' }) | Should -HaveCount 5
        }

        It 'never drafts an issue labeled waiting-on-trigger, follow-up or ready-for-worker, or the backlog #405' {
            $issues = @(
                New-ListedIssue 1 '2026-10-01T00:00:00Z' @('waiting-on-trigger')
                New-ListedIssue 2 '2026-10-02T00:00:00Z' @('follow-up', 'size:S')
                New-ListedIssue 3 '2026-10-03T00:00:00Z' @('ready-for-worker')
                New-ListedIssue 405 '2026-10-04T00:00:00Z' @('documentation')
            )
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson $issues

            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 0
        }

        It 'skips an issue whose draft is newer than its updatedAt, and drafts it again once it is older' {
            $old = Join-Path $script:drafts '2026-11-01'
            New-Item -ItemType Directory $old | Out-Null
            Set-Content -LiteralPath (Join-Path $old 'issue-7.md') 'old draft'
            (Get-Item -LiteralPath (Join-Path $old 'issue-7.md')).LastWriteTimeUtc = [datetime]::new(2026, 11, 2, 0, 0, 0, 'Utc')
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 7 '2026-10-01T00:00:00Z' @() '2026-11-01T00:00:00Z')

            Invoke-Check
            Get-Starts 'readiness' | Should -HaveCount 0

            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 7 '2026-10-01T00:00:00Z' @() '2026-11-03T00:00:00Z')
            Invoke-Check -extra @{ Now = [datetime]'2026-11-07T07:00:00' }
            Get-Starts 'readiness' | Should -HaveCount 1
            Test-Path -LiteralPath (Join-Path $script:drafts '2026-11-07/issue-7.md') | Should -BeTrue
        }

        It 'passes an issue body at the largest size gh returns (65,536 characters) into the spec whole' {
            $body = ('x' * 65530) + '```END'
            $body.Length | Should -Be 65536
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 9 '2026-10-01T00:00:00Z')
            $script:ghAnswers['^issue view (\d+) '] = {
                ConvertTo-Json -Depth 5 -InputObject @{ number = 9; title = 'Big'; body = $body; labels = @(); createdAt = '2026-10-01T00:00:00Z'; updatedAt = '2026-10-01T00:00:00Z'; comments = @() }
            }.GetNewClosure()

            Invoke-Check

            $spec = [IO.File]::ReadAllText((Join-Path $script:specs 'factory-readiness-20261106-070000.md'))
            # The body holds a run of 3 backticks, so its fence is 4 long.
            $fence = '````'
            $spec.Contains("$fence text`n$body`n$fence") | Should -BeTrue
            Test-Path -LiteralPath (Join-Path $script:today 'issue-9.md') | Should -BeTrue
        }

        It 'runs each job at most once a day' {
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 1 '2026-10-01T00:00:00Z' @() '2026-11-06T08:00:00Z')

            Invoke-Check
            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 1
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'readiness: already ran today'
        }

        It 'writes a log line per draft with the cost and the time' {
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 1 '2026-10-01T00:00:00Z')

            Invoke-Check

            $summary = Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw
            $summary | Should -Match '\| 2026-11-06 \| issue-1\.md \| readiness \| spec-factory-readiness-20261106-070000-abc123 \| \$0\.50 \| 2m 5s \| \| \|'
        }
    }

    Context 'retro due' {
        It 'drafts no retro for 9 merged PRs since the last record' {
            $script:ghAnswers['^pr list '] = New-Prs 9

            Invoke-Check

            Get-Starts 'retro' | Should -HaveCount 0
            @(Get-Log $script:ghLog | Where-Object { $_ -match '^pr list .*--search merged:>2026-11-04' }) | Should -HaveCount 1
        }

        It 'drafts a retro for <count> merged PRs since the last record' -ForEach @(@{ count = 10 }, @{ count = 11 }) {
            $script:ghAnswers['^pr list '] = New-Prs $count

            Invoke-Check

            Get-Starts 'retro' | Should -HaveCount 1
            Get-Content -LiteralPath (Join-Path $script:today 'retro-after-2026-11-04.md') -Raw | Should -Match 'The retro draft'
        }

        It 'drafts a retro when there is no retro record at all' {
            $script:gitAnswers[' ls-tree '] = ''
            $script:ghAnswers['^pr list '] = New-Prs 1

            Invoke-Check

            Get-Starts 'retro' | Should -HaveCount 1
            Test-Path -LiteralPath (Join-Path $script:today 'retro-first.md') | Should -BeTrue
        }

        It 'drafts a retro for one record only once' {
            $script:ghAnswers['^pr list '] = New-Prs 10

            Invoke-Check
            Invoke-Check -extra @{ Now = [datetime]'2026-11-07T07:00:00' }

            Get-Starts 'retro' | Should -HaveCount 1
        }
    }

    Context 'recheck of ready issues' {
        BeforeEach {
            $script:ghAnswers['^issue list .*--label ready-for-worker'] = ConvertTo-IssueJson @(New-ListedIssue 77 '2026-10-01T00:00:00Z' @('ready-for-worker'))
            $script:ghAnswers['^api repos/owner/repo/issues/\d+/timeline'] = '[{"event": "labeled", "label": {"name": "size:M"}, "created_at": "2026-11-04T00:00:00Z"}, {"event": "labeled", "label": {"name": "ready-for-worker"}, "created_at": "2026-11-03T00:00:00Z"}]'
        }

        It 'drafts a recheck for a ready issue with a PR merged since its label' {
            $script:ghAnswers['^pr list .*files'] = New-Prs 1 '2026-11-03T12:00:00Z'

            Invoke-Check

            Get-Starts 'recheck' | Should -HaveCount 1
            Get-Content -LiteralPath (Join-Path $script:today 'recheck-77.md') -Raw | Should -Match 'factory-draft: recheck-77'
            $spec = [IO.File]::ReadAllText((Join-Path $script:specs 'factory-recheck-20261106-070000.md'))
            $spec | Should -Match '#501 PR 1 .*: src/x\.rs'
        }

        It 'drafts no recheck when every PR merged before the label' {
            $script:ghAnswers['^pr list .*files'] = New-Prs 1 '2026-11-02T12:00:00Z'

            Invoke-Check

            Get-Starts 'recheck' | Should -HaveCount 0
        }
    }

    Context 'guards' {
        BeforeEach {
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 1 '2026-10-01T00:00:00Z')
        }

        It 'runs with free disk exactly at min_free_disk' {
            $script:freeSpace = { param($path) [decimal]1GB }

            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 1
        }

        It 'stops one byte under min_free_disk, before any GitHub read' {
            $script:freeSpace = { param($path) [decimal]1GB - 1 }

            Invoke-Check

            Get-Log $script:ghLog | Should -HaveCount 0
            Get-Starts 'readiness' | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'stopped: only 1073741823 bytes free'
        }

        It 'runs, and says so in the summary, when the sandbox state folder is unknown' {
            $config = Join-Path $script:cfg 'config.toml'
            Set-Content -LiteralPath $config -Value @(Get-Content -LiteralPath $config | Where-Object { $_ -notmatch '^sandbox_state_dir' })
            $script:freeSpace = { param($path) throw 'not read' }

            Invoke-Check -extra @{ DefaultStateDir = '' }

            Get-Starts 'readiness' | Should -HaveCount 1
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'sandbox drive not checked'
        }

        It 'skips the run while an sbxm- sandbox is running' {
            $script:sbxAnswers['^ls --json$'] = '{"sandboxes": [{"name": "other", "status": "running"}, {"name": "sbxm-task-issue-1-claude", "status": "running"}]}'

            Invoke-Check

            Get-Log $script:ghLog | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'skipped: sandbox sbxm-task-issue-1-claude is running'
        }

        It 'runs while an sbxm- sandbox is only stopped' {
            $script:sbxAnswers['^ls --json$'] = '{"sandboxes": [{"name": "sbxm-task-issue-1-claude", "status": "stopped"}]}'

            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 1
        }

        It 'runs with a loop lock whose process is <case>' -ForEach @(
            @{ case = 'gone'; name = $null }
            @{ case = 'a live non-sbxm process'; name = 'pwsh' }
        ) {
            New-Item -ItemType Directory -Force (Join-Path $script:base '.sbxm') | Out-Null
            Set-Content -LiteralPath (Join-Path $script:base '.sbxm/task-loop.lock') -Value '{"pid":4242,"started_at":"2026-11-06T06:00:00Z"}'
            $answer = $name
            $script:processName = { param($id) if ($id -eq 4242) { $answer } }.GetNewClosure()

            Invoke-Check

            Get-Starts 'readiness' | Should -HaveCount 1
        }

        It 'skips the run while a live sbxm process holds the loop lock' {
            New-Item -ItemType Directory -Force (Join-Path $script:base '.sbxm') | Out-Null
            Set-Content -LiteralPath (Join-Path $script:base '.sbxm/task-loop.lock') -Value '{"pid":4242,"started_at":"2026-11-06T06:00:00Z"}'
            $script:processName = { param($id) if ($id -eq 4242) { 'sbxm' } }

            Invoke-Check

            Get-Log $script:ghLog | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'skipped: a task loop is running \(process 4242\)'
        }

        It 'skips the run, with a log line, when the loop lock cannot be read' {
            New-Item -ItemType Directory -Force (Join-Path $script:base '.sbxm') | Out-Null
            Set-Content -LiteralPath (Join-Path $script:base '.sbxm/task-loop.lock') -Value 'not json'

            Invoke-Check

            Get-Log $script:ghLog | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match "skipped: the task loop lock .* can't be read"
        }

        It 'stops before anything when sbxm cannot read its config' {
            $script:sbxmAnswers['^config profiles-dir$'] = @{ out = 'error: unknown field'; code = 1 }

            { Invoke-Check } | Should -Throw "*sbxm can't read its config*"

            Get-Log $script:sbxLog | Should -HaveCount 0
            Test-Path -LiteralPath $script:drafts | Should -BeFalse
        }
    }

    Context 'the selected config folder (review M-1)' {
        BeforeEach {
            $script:savedConfigDir = $env:SBXM_CONFIG_DIR
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 1 '2026-10-01T00:00:00Z')
        }
        AfterEach { $env:SBXM_CONFIG_DIR = $script:savedConfigDir }

        It 'hands -ConfigDir to every sbxm call when SBXM_CONFIG_DIR is <case>, and restores it' -ForEach @(
            @{ case = 'unset'; incoming = $null }
            @{ case = 'another folder'; incoming = 'elsewhere-config' }
        ) {
            $env:SBXM_CONFIG_DIR = $incoming

            Invoke-Check

            $calls = @(Get-Log $script:sbxmEnvLog)
            @($calls | Where-Object { $_ -match '^task start ' }) | Should -HaveCount 1
            @($calls | Where-Object { $_ -match '^task rm ' }) | Should -HaveCount 1
            foreach ($call in $calls) { $call | Should -BeLike "* => $($script:cfg)" }
            Test-Path -LiteralPath (Join-Path $script:today 'issue-1.md') | Should -BeTrue
            $env:SBXM_CONFIG_DIR | Should -Be $incoming
        }

        It 'restores SBXM_CONFIG_DIR when an sbxm call fails' {
            $env:SBXM_CONFIG_DIR = 'elsewhere-config'
            $script:sbxmAnswers['^task start --spec '] = { throw 'injected start failure' }

            Invoke-Check

            $LASTEXITCODE | Should -Be 1
            @(Get-Log $script:sbxmEnvLog | Where-Object { $_ -match '^task start ' }) | Should -BeLike "* => $($script:cfg)"
            $env:SBXM_CONFIG_DIR | Should -Be 'elsewhere-config'
        }
    }

    Context 'measurements written at once (review M-2)' {
        It 'has the draft''s cost and time in the summary before its task is removed and before the next job' {
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 1 '2026-10-01T00:00:00Z')
            # The next job, recheck, fails at its first GitHub read.
            $script:ghAnswers['^issue list .*--label ready-for-worker'] = @{ out = 'gh: rate limited'; code = 1 }
            $summary = Join-Path $script:today 'summary.md'
            $atRm = Join-Path $TestDrive "at-rm-$([guid]::NewGuid())"
            $script:sbxmAnswers['^task rm --spec '] = { Copy-Item -LiteralPath $summary -Destination $atRm; 'removed' }.GetNewClosure()

            Invoke-Check

            $row = '| 2026-11-06 | issue-1.md | readiness | spec-factory-readiness-20261106-070000-abc123 | $0.50 | 2m 5s | | |'
            @(Get-Content -LiteralPath $atRm) | Should -Contain $row
            @(Get-Content -LiteralPath $summary | Where-Object { $_ -eq $row }) | Should -HaveCount 1
            Get-Content -LiteralPath $summary -Raw | Should -Match 'recheck: failed'
        }
    }

    Context 'the dedicated checkout' {
        It 'writes the checkout''s sbxm-task.toml from the repo''s copy' {
            Invoke-Check

            Get-Content -LiteralPath (Join-Path $script:base 'factory/checkout/sbxm-task.toml') -Raw |
                Should -Be (Get-Content -LiteralPath (Join-Path $script:templates 'sbxm-task.toml') -Raw)
        }

        It 'keeps the task, and writes no draft, when its branch has no draft.md' {
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson @(New-ListedIssue 1 '2026-10-01T00:00:00Z')
            $fakeBase = $script:base
            $makeTask = ${function:New-FakeTask}
            $script:sbxmAnswers['^task start --spec '] = {
                & $makeTask $fakeBase $args[3]
                $task = @(Get-ChildItem (Join-Path $fakeBase '.sbxm/tasks') -Directory)[0]
                $repoGit = Join-Path $task.FullName 'repo.git'
                # The branch points at a commit of the empty tree, which git always knows.
                $empty = git --git-dir $repoGit -c user.name=t -c user.email=t@example.com commit-tree 4b825dc642cb6eb9a060e54bf8d69288fbee4904 -m empty
                git --git-dir $repoGit update-ref "refs/heads/$($task.Name)" $empty
                'started'
            }.GetNewClosure()

            Invoke-Check

            $LASTEXITCODE | Should -Be 1
            Test-Path -LiteralPath (Join-Path $script:today 'issue-1.md') | Should -BeFalse
            @(Get-Log $script:sbxmLog | Where-Object { $_ -match '^task rm ' }) | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'has no draft\.md; it is kept'
        }

        It 'writes the drafts it has and keeps the task when draft.md lacks a section' {
            $issues = 1..2 | ForEach-Object { New-ListedIssue $_ "2026-10-0$($_)T00:00:00Z" }
            $script:ghAnswers['^issue list '] = ConvertTo-IssueJson $issues
            $fakeBase = $script:base
            $makeTask = ${function:New-FakeTask}
            $script:sbxmAnswers['^task start --spec '] = {
                & $makeTask $fakeBase $args[3] "<!-- factory-draft: issue-1 -->`nOnly the first"
                'started'
            }.GetNewClosure()

            Invoke-Check

            $LASTEXITCODE | Should -Be 1
            Get-Content -LiteralPath (Join-Path $script:today 'issue-1.md') -Raw | Should -Match 'Only the first'
            Test-Path -LiteralPath (Join-Path $script:today 'issue-2.md') | Should -BeFalse
            @(Get-Log $script:sbxmLog | Where-Object { $_ -match '^task rm ' }) | Should -HaveCount 0
            Get-Content -LiteralPath (Join-Path $script:today 'summary.md') -Raw | Should -Match 'no section for issue-2\.md; the task is kept'
        }
    }
}
