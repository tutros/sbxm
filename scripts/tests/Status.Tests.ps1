BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\issue-workers.psm1') -Force
}

Describe 'Show-Status' {
    BeforeEach {
        $script:base = Join-Path $TestDrive ([guid]::NewGuid())
        $script:ws = Join-Path $script:base 'sbxm-issue-7'
        New-Item -ItemType Directory (Join-Path $script:ws '.sbxm-issue') | Out-Null
        git -C $script:ws init -q -b main
        git -C $script:ws -c user.name=t -c user.email=t@t commit -q --allow-empty -m base
        git -C $script:ws update-ref refs/remotes/origin/main HEAD
        git -C $script:ws checkout -q -b issue-7
        Mock Write-Host {} -ModuleName issue-workers
        Set-WorkerConfig ([pscustomobject]@{ BaseDir = $script:base; Issue = $null })
    }

    It 'says when a finished worker has uncommitted changes' {
        Set-Content (Join-Path $script:ws 'src.rs') 'fn main() {}'

        Show-Status

        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '*uncommitted changes*' }
    }

    It 'does not mention uncommitted changes when the clone is clean' {
        Show-Status

        Should -Invoke Write-Host -ModuleName issue-workers -Times 0 -ParameterFilter { $Object -like '*uncommitted*' }
    }

    It 'says when a review is in progress' {
        New-Item -ItemType Directory (Join-Path $script:base 'sbxm-review-7') | Out-Null

        Show-Status

        Should -Invoke Write-Host -ModuleName issue-workers -ParameterFilter { $Object -like '*review in progress*' }
    }
}
