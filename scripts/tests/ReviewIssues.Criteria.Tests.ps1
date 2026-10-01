BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\file-review-issues.psm1') -Force
    $script:codex = ConvertFrom-ReviewFile (Get-Content -Raw (Join-Path $PSScriptRoot 'fixtures\review-m2a-codex.md'))
    $script:small = ConvertFrom-ReviewFile (Get-Content -Raw (Join-Path $PSScriptRoot 'fixtures\review-small.md'))
    $script:standard = @(
        'A test covering it fails before the fix and passes after (name it, or say which file it goes in)'
        '`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` pass'
        'Docs updated where behavior users see changed (`README.md`), or "no user-visible change"'
    )
}

Describe 'Get-AcceptanceCriteria' {
    It 'refuses a finding with none, with the exact message' {
        $result = Get-AcceptanceCriteria $script:codex.Findings[0]
        $result.Error | Should -Be 'M2A-1 has no acceptance criteria; add them to the review file, or rerun with -StandardCriteria to file it with only the standard ones'
        $result.Items | Should -BeNullOrEmpty
    }

    It 'with -StandardCriteria gives the placeholder line, then the standard checklist' {
        $result = Get-AcceptanceCriteria $script:codex.Findings[0] -StandardCriteria
        $result.Error | Should -BeNullOrEmpty
        $result.Items[0] | Should -Be '<the specific check is missing from the review: add one before working this issue>'
        $result.Items[1..3] | Should -Be $script:standard
    }

    It 'keeps a finding''s own criteria and appends only the standard ones it lacks' {
        $finding = $script:small.Findings[0]
        (Get-AcceptanceCriteria $finding).Items | Should -Be $finding.Criteria   # it already has all three

        $own = $script:small.Findings[0].PSObject.Copy()
        $own.Criteria = @('it works', 'A test covering it fails before the fix and passes after (name it)')
        $items = (Get-AcceptanceCriteria $own).Items
        $items[0..1] | Should -Be $own.Criteria
        $items[2..3] | Should -Be $script:standard[1..2]
    }

    It 'gives a question none' {
        $result = Get-AcceptanceCriteria $script:codex.Findings[3] -StandardCriteria
        $result.Error | Should -BeNullOrEmpty
        $result.Items | Should -BeNullOrEmpty
    }
}

Describe 'New-IssueBody criteria' {
    It 'renders the standard criteria for a finding that has none when asked' {
        $finding = $script:codex.Findings[1]
        $body = New-IssueBody -Review $script:codex -Finding $finding -Repo 'o/r' -ReviewName 'x.md' `
            -HeadSha ('a' * 40) -StandardCriteria
        $body | Should -Match '(?s)Acceptance criteria:\*\*\n- \[ \] <the specific check is missing'
        $body | Should -Match '- \[ \] `cargo fmt --check`'
    }

    It 'throws the criteria message when there are none and the switch is off' {
        { New-IssueBody -Review $script:codex -Finding $script:codex.Findings[1] -Repo 'o/r' -ReviewName 'x.md' -HeadSha ('a' * 40) } |
            Should -Throw 'M2A-2 has no acceptance criteria*'
    }
}
