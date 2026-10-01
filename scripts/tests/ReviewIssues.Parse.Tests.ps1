BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\file-review-issues.psm1') -Force
    $script:codex = Get-Content -Raw (Join-Path $PSScriptRoot 'fixtures\review-m2a-codex.md')
    $script:dash = [string][char]0x2014

    function New-Review([string]$Heading, [string]$Section = 'Must fix') {
        @"
# Review

Scope: ``aaaaaaa..bbbbbbb`` (2 commits)
Issues: pending (no access)

## $Section

### $Heading

**Where:** ``src/a.rs:1-2``
**What happens:** it breaks
**Why it matters:** decision 1
**Fix:** fix it
**Acceptance criteria:**
- [ ] it works
"@
    }
}

Describe 'ConvertFrom-ReviewFile on the Codex review' {
    BeforeAll { $script:review = ConvertFrom-ReviewFile $script:codex }

    It 'finds exactly four findings with ids, labels and titles' {
        $found = $script:review.Findings | ForEach-Object { "$($_.Id)|$($_.Label)" }
        $found | Should -Be @('M2A-1|must-fix', 'M2A-2|must-fix', 'M2A-3|should-fix', 'M2A-Q1|question')
        $script:review.Findings[0].Title | Should -Be 'Inherited Git config variables can execute a host command while diffing a contestant workspace'
        $script:review.Findings[1].Title | Should -Be '`[eval.cosine]` is accepted but silently does nothing'
    }

    It 'reads the fields that are in the file, under canonical names' {
        $m1 = $script:review.Findings[0]
        $m1.Fields['where'].StartsWith('`src/git.rs:50-72`, used by') | Should -BeTrue
        $m1.Fields['what happens'] | Should -BeLike "*GIT_CONFIG_COUNT*"
        $m1.Fields['why it matters'] | Should -BeLike 'decision 127*'
        $m1.Fields['fix'] | Should -BeLike 'construct the Git child*'
        $m1.Fields.Contains('depends on') | Should -BeFalse
    }

    It 'maps Smallest fix and Recommendation to fix' {
        $script:review.Findings[2].Fields['fix'] | Should -BeLike 'before merge*'
        $script:review.Findings[3].Fields['fix'] | Should -BeLike 'reject Antigravity*'
    }

    It 'reports the Summary, Verification and Nit text as not filed' {
        $script:review.NotFiled.Sections | Should -Be 2
        $script:review.NotFiled.Nits | Should -Be 1
    }

    It 'reads the Scope head sha and the Issues line' {
        $script:review.HeadSha | Should -Be '82659a26614c58da671641ffa6e62264cb9038ce'
        $script:review.IssuesLine | Should -BeLike 'Issues: pending.*'
    }

    It 'has no missing fields except the acceptance criteria' {
        foreach ($finding in $script:review.Findings) { $finding.Missing | Should -BeNullOrEmpty }
    }
}

Describe 'ConvertFrom-ReviewFile on the canonical format' {
    It 'splits id from title on a hyphen, a colon and an em dash' {
        foreach ($heading in @('C-1 - Title here', 'C-1: Title here', "C-1 $script:dash Title here")) {
            $review = ConvertFrom-ReviewFile (New-Review $heading)
            $review.Findings.Count | Should -Be 1
            $review.Findings[0].Id | Should -Be 'C-1'
            $review.Findings[0].Title | Should -Be 'Title here'
        }
    }

    It 'gives the same structure as the Codex variant' {
        $codexStyle = ConvertFrom-ReviewFile (New-Review "C-1 $script:dash Title here" 'Must fix')
        $canonical = ConvertFrom-ReviewFile ((New-Review 'C-1 - Title here' 'Must fix') -replace '\*\*Fix:\*\*', '**Smallest fix:**')
        $canonical.Findings[0].Fields.Keys | Should -Be $codexStyle.Findings[0].Fields.Keys
        $canonical.Findings[0].Criteria | Should -Be @('it works')
    }

    It 'maps section headings to labels, ignoring case and colons' {
        $labels = @{ 'Must fix' = 'must-fix'; 'Should fix:' = 'should-fix'; 'QUESTION' = 'question'; 'Questions' = 'question' }
        foreach ($section in $labels.Keys) {
            (ConvertFrom-ReviewFile (New-Review 'C-1 - T' $section)).Findings[0].Label | Should -Be $labels[$section]
        }
    }

    It 'matches field names case-insensitively' {
        $text = (New-Review 'C-1 - T') -replace '\*\*What happens:\*\*', '**WHAT HAPPENS:**'
        (ConvertFrom-ReviewFile $text).Findings[0].Missing | Should -BeNullOrEmpty
    }

    It 'names the finding and the field when a required one is missing' {
        $text = (New-Review 'C-1 - T') -replace '(?m)^\*\*Why it matters:\*\*.*\r?\n', ''
        $finding = (ConvertFrom-ReviewFile $text).Findings[0]
        $finding.Missing | Should -Be @('why it matters')
    }

    It 'needs Fix for a must-fix and a Recommendation or Fix for a question' {
        $text = (New-Review 'C-1 - T' 'Questions') -replace '(?m)^\*\*Fix:\*\*.*\r?\n', ''
        (ConvertFrom-ReviewFile $text).Findings[0].Missing | Should -Be @('fix')
    }

    It 'accepts a field value below the label line' {
        $text = (New-Review 'C-1 - T') -replace '\*\*What happens:\*\* it breaks', "**What happens:**`n`nit breaks`nin two lines"
        (ConvertFrom-ReviewFile $text).Findings[0].Fields['what happens'] | Should -Be "it breaks`nin two lines"
    }

    It 'reads CRLF files the same way' {
        $crlf = (New-Review 'C-1 - T') -replace "`n", "`r`n"
        (ConvertFrom-ReviewFile $crlf).Findings[0].Fields['fix'] | Should -Be 'fix it'
    }
}
