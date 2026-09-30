BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\file-review-issues.psm1') -Force
    $script:fixtures = Join-Path $PSScriptRoot 'fixtures'
    $script:sha = '2222222222222222222222222222222222222222'

    function Get-Golden([string]$name) {
        (Get-Content -Raw (Join-Path $script:fixtures $name)) -replace "`r`n", "`n"
    }
    function Get-Small { ConvertFrom-ReviewFile (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) }
}

Describe 'ConvertTo-WhereLinks' {
    It 'links path:a-b at the head sha' {
        ConvertTo-WhereLinks '`src/git.rs:50-72`' 'o/r' $script:sha |
            Should -Be "[``src/git.rs:50-72``](https://github.com/o/r/blob/$script:sha/src/git.rs#L50-L72)"
    }

    It 'links a single line, a bare path and a path outside backticks' {
        $out = ConvertTo-WhereLinks 'src/b.rs:5 and `README.md`' 'o/r' $script:sha
        $out | Should -Be "[src/b.rs:5](https://github.com/o/r/blob/$script:sha/src/b.rs#L5) and [``README.md``](https://github.com/o/r/blob/$script:sha/README.md)"
    }

    It 'leaves ordinary words alone' {
        ConvertTo-WhereLinks 'used by e.g. the parser' 'o/r' $script:sha | Should -Be 'used by e.g. the parser'
    }

    It 'warns and keeps the text when there is no sha' {
        $out = ConvertTo-WhereLinks '`src/git.rs:50-72`' 'o/r' $null 3>&1
        ($out | Where-Object { $_ -is [System.Management.Automation.WarningRecord] }).Message | Should -BeLike '*no commit sha*'
        ($out | Where-Object { $_ -is [string] }) | Should -Be '`src/git.rs:50-72`'
    }
}

Describe 'Resolve-HeadSha' {
    BeforeEach {
        Mock git { $global:LASTEXITCODE = 0; $script:sha } -ModuleName file-review-issues
    }

    It 'keeps a full sha without calling git' {
        Resolve-HeadSha $script:sha | Should -Be $script:sha
        Should -Invoke git -ModuleName file-review-issues -Times 0
    }

    It 'resolves a short sha with git rev-parse' {
        Resolve-HeadSha '2222222' | Should -Be $script:sha
        Should -Invoke git -ModuleName file-review-issues -Times 1 -ParameterFilter { $args -contains 'rev-parse' }
    }

    It 'returns nothing when there is no sha' {
        Resolve-HeadSha $null | Should -BeNullOrEmpty
    }
}

Describe 'New-IssueBody' {
    BeforeAll { $script:review = Get-Small }

    It 'renders the template parts in order and ends with the marker (golden)' {
        $body = New-IssueBody -Review $script:review -Finding $script:review.Findings[0] -Repo 'o/r' `
            -ReviewName 'review-small.md' -HeadSha $script:sha
        $body.TrimEnd() | Should -Be (Get-Golden 'golden-body-s1.md').TrimEnd()
    }

    It 'says none known for a missing Depends on and adds the reverse Related (golden)' {
        $body = New-IssueBody -Review $script:review -Finding $script:review.Findings[1] -Repo 'o/r' `
            -ReviewName 'review-small.md' -HeadSha $script:sha
        $body.TrimEnd() | Should -Be (Get-Golden 'golden-body-s2-linked.md').TrimEnd()
    }

    It 'uses Options and recommendation for a question and no criteria (golden)' {
        $body = New-IssueBody -Review $script:review -Finding $script:review.Findings[2] -Repo 'o/r' `
            -ReviewName 'review-small.md' -HeadSha $script:sha
        $body.TrimEnd() | Should -Be (Get-Golden 'golden-body-q1.md').TrimEnd()
    }

    It 'rewrites finding ids to issue numbers where they are known' {
        $body = New-IssueBody -Review $script:review -Finding $script:review.Findings[0] -Repo 'o/r' `
            -ReviewName 'review-small.md' -HeadSha $script:sha -IdMap @{ 'S-2' = 41 }
        $body | Should -BeLike '*Depends on:** #41 (the message must match)*'
    }
}
