BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\issue-workers.psm1') -Force
    $script:entry = Join-Path $PSScriptRoot '..\issue-workers.ps1'
}

Describe '-GatesOnly validation (the script itself)' {
    It 'is refused with any action other than review' {
        { & $script:entry status -GatesOnly } | Should -Throw '-GatesOnly works only with review*'
    }

    It 'is refused with -Pr' {
        { & $script:entry review -Pr 3 -GatesOnly } | Should -Throw '-GatesOnly checks a worker clone, not a PR*'
    }

    It 'needs -Issue' {
        { & $script:entry review -GatesOnly } | Should -Throw 'review needs -Issue*'
    }
}

Describe 'Invoke-GatesOnly' {
    BeforeEach {
        $script:base = Join-Path $TestDrive ([guid]::NewGuid())
        $script:ws = Join-Path $script:base 'sbxm-issue-7'
        New-Item -ItemType Directory (Join-Path $script:ws '.sbxm-issue') | Out-Null
        Set-WorkerConfig ([pscustomobject]@{ BaseDir = $script:base; Issue = $null })
        Mock Invoke-Gates {} -ModuleName issue-workers
        Mock Test-AgentRunning { $false } -ModuleName issue-workers
        Mock Assert-ReviewSecret {} -ModuleName issue-workers
        Mock Invoke-Reviewer { 0 } -ModuleName issue-workers
        Mock Initialize-ReviewWorkspace {} -ModuleName issue-workers
        Mock sbxm {} -ModuleName issue-workers
        Mock Write-Host {} -ModuleName issue-workers
    }

    It 'runs the gates on the worker clone and logs them next to the issue files' {
        Invoke-GatesOnly 7

        Should -Invoke Invoke-Gates -ModuleName issue-workers -Times 1 -Exactly -ParameterFilter {
            $workspace -eq $script:ws -and $log -eq (Join-Path $script:ws '.sbxm-issue\gates.log')
        }
    }

    It 'starts no reviewer and no sandbox, and needs no review secret' {
        Invoke-GatesOnly 7

        Should -Invoke Invoke-Reviewer -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke Initialize-ReviewWorkspace -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke Assert-ReviewSecret -ModuleName issue-workers -Times 0 -Exactly
        Should -Invoke sbxm -ModuleName issue-workers -Times 0 -Exactly
    }

    It 'starts each run with a fresh log' {
        $log = Join-Path $script:ws '.sbxm-issue\gates.log'
        Set-Content $log 'old run'
        Mock Invoke-Gates { Test-Path $log | Should -BeFalse } -ModuleName issue-workers

        Invoke-GatesOnly 7
    }

    It 'says the gates passed' {
        Invoke-GatesOnly 7

        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '#7: gates passed*' }
    }

    It 'lets a failing gate through, so the command fails' {
        Mock Invoke-Gates { throw '#7: cargo test failed on the host' } -ModuleName issue-workers

        { Invoke-GatesOnly 7 } | Should -Throw '*cargo test failed*'
    }

    It 'refuses when there is no worker' {
        { Invoke-GatesOnly 8 } | Should -Throw '#8: no worker at*'
        Should -Invoke Invoke-Gates -ModuleName issue-workers -Times 0 -Exactly
    }

    It 'refuses while the agent is still running' {
        Mock Test-AgentRunning { $true } -ModuleName issue-workers

        { Invoke-GatesOnly 7 } | Should -Throw '#7: the agent is still running*'
        Should -Invoke Invoke-Gates -ModuleName issue-workers -Times 0 -Exactly
    }
}
