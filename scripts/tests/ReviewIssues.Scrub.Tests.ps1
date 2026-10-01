# Secret-shaped values are built here, so the repo holds no literal that looks like one.
$secretCases = @(
    @{ Name = 'a GitHub token'; Value = 'ghp_' + ('a' * 30) }
    @{ Name = 'an OAuth token'; Value = 'gho_' + ('b' * 30) }
    @{ Name = 'a server token'; Value = 'ghs_' + ('c' * 30) }
    @{ Name = 'a fine-grained token'; Value = 'github_pat_' + ('d' * 30) }
    @{ Name = 'an sk- key'; Value = 'sk-' + ('e' * 24) }
    @{ Name = 'an AWS key'; Value = 'AKIA' + ('F' * 16) }
    @{ Name = 'a bearer token'; Value = 'Bearer ' + ('g' * 30) }
    @{ Name = 'a private key'; Value = '-----BEGIN RSA PRIVATE KEY-----' }
    @{ Name = 'a password'; Value = 'password = hunter2hunter' }
    @{ Name = 'a token value'; Value = 'token = "abcd1234efgh"' }
)

BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\file-review-issues.psm1') -Force

    function New-ReviewWith([string]$line) {
        @"
# Review

Scope: ``aaaaaaa..bbbbbbb`` (1 commit)
Issues: pending (test)

## Must fix

### S-1 - Thing

**Where:** ``src/a.rs:1``
**What happens:** first line
$line
**Why it matters:** x
**Fix:** y
**Acceptance criteria:**
- [ ] z
"@
    }
}

Describe 'Find-Secrets' {
    It 'refuses <name>, naming the finding and line but not the value' -ForEach $secretCases {
        $text = New-ReviewWith "output: $Value"
        $finding = (ConvertFrom-ReviewFile $text).Findings[0]
        $found = @(Find-Secrets -Text $text -Finding $finding)
        $found.Count | Should -Be 1
        $found[0].Id | Should -Be 'S-1'
        $found[0].Line | Should -Be 12
        ($found | Out-String) | Should -Not -Match ([regex]::Escape($Value.Substring(6)))
    }

    It 'refuses the short assignment <text>' -ForEach @(
        @{ Text = 'token = a' }, @{ Text = 'token = ab' }, @{ Text = 'token = abc' },
        @{ Text = 'password = x' }, @{ Text = 'password=xy' }, @{ Text = 'secret = xyz' }, @{ Text = 'api_token = abc' },
        @{ Text = 'token = "a"' }, @{ Text = "password = 'ab'" }, @{ Text = 'token = "abc"' }
    ) {
        $text = New-ReviewWith "output: $Text"
        $found = @(Find-Secrets -Text $text -Finding (ConvertFrom-ReviewFile $text).Findings[0])
        $found.Count | Should -Be 1
        $found[0].Kind | Should -Be 'a password or token assignment'
    }

    It 'lets the empty assignment <text> and a comparison through' -ForEach @(
        @{ Text = 'token =' }, @{ Text = 'password = ' }, @{ Text = 'token = ""' }, @{ Text = 'if token == other then' },
        @{ Text = 'the token is missing' }, @{ Text = 'tokens = 5 of them' }
    ) {
        $text = New-ReviewWith "output: $Text"
        @(Find-Secrets -Text $text -Finding (ConvertFrom-ReviewFile $text).Findings[0]) | Should -BeNullOrEmpty
    }

    It 'finds nothing in the Codex review' {
        $text = Get-Content -Raw (Join-Path $PSScriptRoot 'fixtures\review-m2a-codex.md')
        foreach ($finding in (ConvertFrom-ReviewFile $text).Findings) { @(Find-Secrets -Text $text -Finding $finding) | Should -BeNullOrEmpty }
    }

    It 'lets prose about tokens and passwords through' {
        $text = New-ReviewWith 'A token is required; the password field is empty.'
        @(Find-Secrets -Text $text -Finding (ConvertFrom-ReviewFile $text).Findings[0]) | Should -BeNullOrEmpty
    }
}

Describe 'Protect-Text' {
    It 'replaces a personal Windows path with ~ and warns' {
        $out = Protect-Text 'see C:\Users\james\proj\x.rs' -Id 'S-1' 3>&1
        ($out | Where-Object { $_ -is [string] }) | Should -Be 'see ~\proj\x.rs'
        ($out | Where-Object { $_ -is [System.Management.Automation.WarningRecord] }).Message | Should -BeLike 'S-1:*personal path*'
    }

    It 'replaces a personal Linux path' {
        Protect-Text 'in /home/james/src/a.rs' -Id 'S-1' 3>$null | Should -Be 'in ~/src/a.rs'
    }

    It 'removes a whole Windows profile name that contains a space' {
        Protect-Text 'see C:\Users\Jane Doe\project\x.rs' -Id 'S-1' 3>$null | Should -Be 'see ~\project\x.rs'
    }

    It 'removes a whole Linux profile name that contains a space' {
        Protect-Text 'in /home/Jane Doe/project/x.rs' -Id 'S-1' 3>$null | Should -Be 'in ~/project/x.rs'
    }

    It 'removes a profile name of four words, however many spaces it has' {
        Protect-Text 'C:\Users\Mary Jane Watson Parker\repo' -Id 'S-1' 3>$null | Should -Be '~\repo'
        Protect-Text 'at /home/Mary Jane Watson Parker/repo/x' -Id 'S-1' 3>$null | Should -Be 'at ~/repo/x'
        Protect-Text 'in /Users/Mary Jane Watson Parker Smith/repo' -Id 'S-1' 3>$null | Should -Be 'in ~/repo'
    }

    It 'stops a spaced profile name at a quote, a backtick or a newline' {
        Protect-Text 'a "C:\Users\Jane Doe" b' -Id 'S-1' 3>$null | Should -Be 'a "~" b'
        Protect-Text "x /home/Jane Doe``y/z" -Id 'S-1' 3>$null | Should -Be "x ~``y/z"
        Protect-Text "C:\Users\james`nsee D:\x\y" -Id 'S-1' 3>$null | Should -Be "~`nsee D:\x\y"
    }

    It 'does not swallow ordinary prose after a path with no separator' {
        Protect-Text 'C:\Users\james is the folder' -Id 'S-1' 3>$null | Should -Be '~ is the folder'
        Protect-Text 'under /home/james and then /etc/x' -Id 'S-1' 3>$null | Should -Be 'under ~ and then /etc/x'
    }

    It 'keeps personal paths with -KeepPaths' {
        Protect-Text 'see C:\Users\james\x' -Id 'S-1' -KeepPaths | Should -Be 'see C:\Users\james\x'
    }

    It 'leaves project paths alone' {
        Protect-Text 'see E:\sbxm-projects\x' -Id 'S-1' | Should -Be 'see E:\sbxm-projects\x'
    }

    It 'warns about an e-mail address without changing it' {
        $out = Protect-Text 'ask a@b.example' -Id 'S-1' 3>&1
        ($out | Where-Object { $_ -is [string] }) | Should -Be 'ask a@b.example'
        ($out | Where-Object { $_ -is [System.Management.Automation.WarningRecord] }).Message | Should -BeLike 'S-1:*e-mail*'
    }
}

Describe 'Protect-Finding' {
    It 'protects the title, every field and the criteria' {
        $text = (New-ReviewWith 'path C:\Users\james\x') -replace '- \[ \] z', '- [ ] check C:\Users\james\y'
        $finding = (ConvertFrom-ReviewFile $text).Findings[0]
        $safe = Protect-Finding $finding 3>$null
        $safe.Fields['what happens'] | Should -Be "first line`npath ~\x"
        $safe.Criteria | Should -Be @('check ~\y')
    }
}
