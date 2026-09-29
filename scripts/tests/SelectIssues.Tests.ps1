BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\issue-workers.psm1') -Force

    function New-Issue([int]$Number, [string[]]$Labels = @('should-fix'), [string]$Body = '') {
        [pscustomobject]@{
            number = $Number
            title  = "issue $Number"
            labels = @($Labels | ForEach-Object { [pscustomobject]@{ name = $_ } })
            body   = $Body
        }
    }
}

Describe 'Select-Issues' {
    BeforeEach {
        $global:LASTEXITCODE = 0
        $script:base = Join-Path $TestDrive ([guid]::NewGuid())
        New-Item -ItemType Directory $script:base | Out-Null
        $script:issues = @()
        Mock gh { $script:issues | ConvertTo-Json -Depth 5 -AsArray } -ModuleName issue-workers
        Mock Write-Host {} -ModuleName issue-workers
        Set-WorkerConfig ([pscustomobject]@{
                BaseDir = $script:base; SbxmProfile = 'sbxm-dev'; Repo = 'o/r'; TimeLimit = '2h'
                ReviewHarness = 'codex'; ReviewModel = ''; ReviewTimeLimit = '45m'; Workers = 2; Issue = $null
            })
    }

    It 'picks must-fix issues before should-fix ones, up to the worker count' {
        $script:issues = @(
            (New-Issue 1 @('should-fix')), (New-Issue 2 @('must-fix')), (New-Issue 3 @('should-fix')))

        @(Select-Issues).Number | Should -Be @(2, 1)
    }

    It 'picks an issue with no labels after the labelled ones (rank 9), without error' {
        $script:issues = @((New-Issue 1 @()), (New-Issue 2 @('should-fix')))

        @(Select-Issues).Number | Should -Be @(2, 1)
    }

    It 'skips a question and says why' {
        $script:issues = @((New-Issue 1 @('question', 'must-fix')), (New-Issue 2))

        @(Select-Issues).Number | Should -Be @(2)
        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '#1: a question*' }
    }

    It 'skips an issue whose Depends on issue is still open' {
        $script:issues = @((New-Issue 1), (New-Issue 2 -Body '**Depends on:** #1'))

        @(Select-Issues).Number | Should -Be @(1)
        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '#2: blocked by open #1*' }
    }

    It 'takes an issue whose Depends on issue is closed' {
        $script:issues = @((New-Issue 2 -Body '**Depends on:** #1'))

        @(Select-Issues).Number | Should -Be @(2)
    }

    It 'skips an issue that already has a worker' {
        New-Item -ItemType Directory (Join-Path $script:base 'sbxm-issue-1') | Out-Null
        $script:issues = @((New-Issue 1), (New-Issue 2))

        @(Select-Issues).Number | Should -Be @(2)
        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '#1: already has a worker*' }
    }

    It 'skips an issue related to one that has a worker (written on the worker''s issue)' {
        New-Item -ItemType Directory (Join-Path $script:base 'sbxm-issue-1') | Out-Null
        $script:issues = @((New-Issue 1 -Body '**Related:** #2'), (New-Issue 2))

        @(Select-Issues).Number | Should -BeNullOrEmpty
        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '#2: related to #1*' }
    }

    It 'skips an issue related to one that has a worker (written on its own issue)' {
        New-Item -ItemType Directory (Join-Path $script:base 'sbxm-issue-1') | Out-Null
        $script:issues = @((New-Issue 1), (New-Issue 2 -Body '**Related:** #1'))

        @(Select-Issues).Number | Should -BeNullOrEmpty
    }

    It 'does not start two related issues in the same run' {
        $script:issues = @((New-Issue 1 -Body '**Related:** #2'), (New-Issue 2))

        @(Select-Issues).Number | Should -Be @(1)
    }

    It 'warns about a named issue that is not open' {
        Mock Write-Warning {} -ModuleName issue-workers
        $script:issues = @((New-Issue 1))
        Set-WorkerConfig ([pscustomobject]@{
                BaseDir = $script:base; SbxmProfile = 'sbxm-dev'; Repo = 'o/r'; TimeLimit = '2h'
                ReviewHarness = 'codex'; ReviewModel = ''; ReviewTimeLimit = '45m'; Workers = 2; Issue = @(1, 9)
            })

        @(Select-Issues).Number | Should -Be @(1)
        Should -Invoke Write-Warning -ModuleName issue-workers -ParameterFilter { $Message -like '#9 isn''t an open issue*' }
    }

    It 'stops when gh fails' {
        Mock gh { $global:LASTEXITCODE = 1 } -ModuleName issue-workers

        { Select-Issues } | Should -Throw 'gh issue list failed'
    }
}
