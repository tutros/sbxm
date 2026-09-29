BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\issue-workers.psm1') -Force
    $script:entry = Join-Path $PSScriptRoot '..\issue-workers.ps1'

    function New-PrInfo([hashtable]$Override = @{}) {
        $info = @{
            headRefName = 'issue-5'; isCrossRepository = $false; state = 'OPEN'; title = 'Fix five'
            body = 'Fixes #5'; closingIssuesReferences = @(@{ number = 5 })
        }
        foreach ($key in $Override.Keys) { $info[$key] = $Override[$key] }
        $info | ConvertTo-Json -Depth 5
    }
}

Describe '-Pr validation (the script itself)' {
    It 'is refused with -Issue' {
        { & $script:entry review -Issue 1 -Pr 3 } | Should -Throw '-Pr and -Issue both name what to review*'
    }

    It 'is refused with any action other than review' {
        { & $script:entry status -Pr 3 } | Should -Throw '-Pr works only with review; use: review -Pr 3'
    }

    It 'is refused when zero' {
        { & $script:entry review -Pr 0 } | Should -Throw "-Pr 0 isn't a PR number; give a positive number"
    }

    It 'is refused when negative' {
        { & $script:entry review -Pr -4 } | Should -Throw "-Pr -4 isn't a PR number; give a positive number"
    }
}

Describe 'Invoke-PrReview stops before doing any work' {
    BeforeEach {
        $global:LASTEXITCODE = 0
        $script:base = Join-Path $TestDrive ([guid]::NewGuid())
        New-Item -ItemType Directory $script:base | Out-Null
        $script:prJson = New-PrInfo
        $script:issueLookupFails = $false
        $script:secrets = '{"secrets":[{"scope":"global","type":"service","name":"openai"}]}'
        Set-WorkerConfig ([pscustomobject]@{
                BaseDir = $script:base; SbxmProfile = 'sbxm-dev'; Repo = 'o/r'; TimeLimit = '2h'
                ReviewHarness = 'codex'; ReviewModel = ''; ReviewTimeLimit = '45m'; Workers = 2; Issue = $null
            })
        Mock gh {
            $global:LASTEXITCODE = 0
            if ($args[0] -eq 'pr' -and $args[1] -eq 'view') { return $script:prJson }
            if ($args[0] -eq 'issue' -and $args[1] -eq 'view') {
                if ($script:issueLookupFails) { $global:LASTEXITCODE = 1; return }
                return 'issue text'
            }
        } -ModuleName issue-workers
        Mock sbx { $global:LASTEXITCODE = 0; $script:secrets } -ModuleName issue-workers
        Mock sbxm { $global:LASTEXITCODE = 0 } -ModuleName issue-workers
        Mock git { $global:LASTEXITCODE = 0 } -ModuleName issue-workers
        Mock cargo { $global:LASTEXITCODE = 0 } -ModuleName issue-workers
        Mock Initialize-ReviewWorkspace {} -ModuleName issue-workers
        Mock Invoke-Gates {} -ModuleName issue-workers
        Mock Invoke-Reviewer { 0 } -ModuleName issue-workers
        Mock Remove-ReviewProject {} -ModuleName issue-workers
        Mock Write-Host {} -ModuleName issue-workers
    }

    AfterEach {
        # Nothing may have been cloned, checked, created in a sandbox or commented on.
        Should -Invoke Initialize-ReviewWorkspace -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke Invoke-Gates -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke Invoke-Reviewer -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke sbxm -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke cargo -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke git -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke gh -ModuleName issue-workers -Times 0 -Exactly -ParameterFilter { $args[1] -eq 'comment' }
    }

    It 'refuses a fork PR' {
        $script:prJson = New-PrInfo @{ isCrossRepository = $true }

        { Invoke-PrReview 5 } | Should -Throw '*comes from a fork*'
    }

    It 'refuses a closed PR' {
        $script:prJson = New-PrInfo @{ state = 'CLOSED' }

        { Invoke-PrReview 5 } | Should -Throw 'PR #5 is closed, not open'
    }

    It 'stops when the PR cannot be read' {
        Mock gh { $global:LASTEXITCODE = 1 } -ModuleName issue-workers

        { Invoke-PrReview 5 } | Should -Throw 'gh pr view 5 failed'
    }

    It 'stops when a linked issue cannot be read' {
        $script:issueLookupFails = $true

        { Invoke-PrReview 5 } | Should -Throw '*gh issue view 5 (an issue the PR closes) failed'
    }

    It 'stops when the reviewer secret is missing' {
        $script:secrets = '{"secrets":[]}'

        { Invoke-PrReview 5 } | Should -Throw "*needs the 'openai' secret*sbx secret set openai"
    }

    It 'stops when a review folder is left from an earlier run' {
        New-Item -ItemType Directory (Join-Path $script:base 'sbxm-review-pr-5') | Out-Null

        { Invoke-PrReview 5 } | Should -Throw '*is left from an earlier review*'
    }
}
