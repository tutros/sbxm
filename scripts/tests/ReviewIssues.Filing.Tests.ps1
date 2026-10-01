BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\file-review-issues.psm1') -Force
    $script:fixtures = Join-Path $PSScriptRoot 'fixtures'

    # A fake GitHub behind the mocked gh and git: what the script reads and what it writes.
    function Initialize-Fake {
        $global:LASTEXITCODE = 0
        $script:fake = @{
            Remote = 'https://github.com/o/r.git'; LoggedIn = $true; Scopes = "'repo', 'read:org'"
            Labels = @('must-fix', 'should-fix', 'question'); Issues = [System.Collections.Generic.List[object]]::new()
            Next = 40; Writes = [System.Collections.Generic.List[string]]::new(); FailCreateAt = 0; Created = 0
            BodyFiles = [System.Collections.Generic.List[string]]::new()
        }
        Mock git {
            $global:LASTEXITCODE = 0
            if ($args[0] -eq 'remote') { return $script:fake.Remote }
            if ($args[0] -eq 'rev-parse') { return ('2' * 40) }
        } -ModuleName file-review-issues
        Mock gh {
            $global:LASTEXITCODE = 0
            $a = $args
            $get = { param($name) $i = [array]::IndexOf($a, $name); if ($i -ge 0) { $a[$i + 1] } }
            switch ("$($a[0]) $($a[1])") {
                'repo view' { return '{"nameWithOwner":"o/r"}' }
                'auth status' {
                    if (-not $script:fake.LoggedIn) { $global:LASTEXITCODE = 1; return 'You are not logged in' }
                    return "Logged in to github.com`n  - Token scopes: $($script:fake.Scopes)"
                }
                'label list' { return (($script:fake.Labels | ForEach-Object { @{ name = $_ } }) | ConvertTo-Json -AsArray) }
                'issue list' { return (@($script:fake.Issues) | ConvertTo-Json -Depth 4 -AsArray) }
                'issue create' {
                    $script:fake.Writes.Add('create')
                    $script:fake.BodyFiles.Add((& $get '--body-file'))
                    $script:fake.Created++
                    if ($script:fake.FailCreateAt -eq $script:fake.Created) { $global:LASTEXITCODE = 1; return }
                    $number = $script:fake.Next++
                    $script:fake.Issues.Add(@{ number = $number; title = (& $get '--title'); body = (Get-Content -Raw (& $get '--body-file')); label = (& $get '--label') })
                    return "https://github.com/o/r/issues/$number"
                }
                'issue edit' {
                    $script:fake.Writes.Add("edit $($a[2])")
                    $script:fake.BodyFiles.Add((& $get '--body-file'))
                    $issue = $script:fake.Issues | Where-Object { $_.number -eq [int]$a[2] }
                    $issue.body = Get-Content -Raw (& $get '--body-file')
                    return "https://github.com/o/r/issues/$($a[2])"
                }
            }
        } -ModuleName file-review-issues
    }

    # Runs the filing and returns the exit code and everything it printed.
    function Invoke-Filing {
        param([string]$Fixture = 'review-small.md', [hashtable]$Arguments = @{})
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        $path = Join-Path $dir $Fixture
        $source = Join-Path $script:fixtures $Fixture
        if (Test-Path $source) { Copy-Item $source $path -Force }   # a fresh copy; other names are files the test made
        $all = Invoke-ReviewFiling -Review $path @Arguments 6>&1 3>&1
        [pscustomobject]@{
            Code = @($all | Where-Object { $_ -is [int] })[-1]
            Text = (($all | Where-Object { $_ -isnot [int] } | ForEach-Object { if ($_ -is [System.Management.Automation.InformationRecord]) { "$($_.MessageData)" } else { "$_" } }) -join "`n")
            Path = $path
        }
    }
}

Describe 'Duplicate finding ids are refused before any GitHub call' {
    BeforeEach { Initialize-Fake }

    It 'names the id and both review lines, and calls gh not at all' {
        $text = (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) -replace '### S-2 - Message is wrong', '### S-1 - Message is wrong'
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        Set-Content (Join-Path $dir 'review-dup.md') $text
        $run = Invoke-Filing -Fixture 'review-dup.md'
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*refused: S-1 is used by more than one finding*at lines 8 and 21*'
        $run.Text | Should -BeLike '*make finding ids unique*'
        $script:fake.Writes | Should -BeNullOrEmpty
        Should -Invoke gh -ModuleName file-review-issues -Times 0
    }
}

Describe 'The review file needs exactly one Issues: line' {
    BeforeEach { Initialize-Fake }

    It 'refuses a file with no Issues: line, before any gh call' {
        $text = (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) -replace '(?m)^Issues:.*\r?\n', ''
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        Set-Content (Join-Path $dir 'review-noissues.md') $text
        $run = Invoke-Filing -Fixture 'review-noissues.md'
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*has no*Issues:*line*keep exactly one*'
        Should -Invoke gh -ModuleName file-review-issues -Times 0
    }

    It 'refuses a file with two Issues: lines, before any gh call' {
        $text = (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) -replace 'Issues: pending \(test\)', "Issues: pending (test)`nIssues: pending (again)"
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        Set-Content (Join-Path $dir 'review-dupissues.md') $text
        $run = Invoke-Filing -Fixture 'review-dupissues.md'
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*has 2*Issues:*line*keep exactly one*'
        Should -Invoke gh -ModuleName file-review-issues -Times 0
    }

    It 'checks the file can be updated before the first gh issue create' {
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        $path = Join-Path $dir 'review-readonly.md'
        Copy-Item (Join-Path $script:fixtures 'review-small.md') $path
        Set-ItemProperty -Path $path -Name IsReadOnly -Value $true
        try {
            $all = Invoke-ReviewFiling -Review $path -Create 6>&1 3>&1
            $code = @($all | Where-Object { $_ -is [int] })[-1]
            $code | Should -Be 1
            $script:fake.Writes | Should -BeNullOrEmpty
        }
        finally { Set-ItemProperty -Path $path -Name IsReadOnly -Value $false }
    }
}

Describe 'A no-SHA Scope line and the review file name get the same protection as a finding''s fields' {
    # Secret-shaped values are built here, so the repo holds no literal that looks like one (see
    # ReviewIssues.Scrub.Tests.ps1, which has the same list for Find-Secrets).
    BeforeAll {
        $script:secretCases = @(
            @{ Name = 'a GitHub token'; Value = 'ghp_' + ('a' * 30) }
            @{ Name = 'an sk- key'; Value = 'sk-' + ('e' * 24) }
            @{ Name = 'an AWS key'; Value = 'AKIA' + ('F' * 16) }
            @{ Name = 'a bearer token'; Value = 'Bearer ' + ('g' * 30) }
            @{ Name = 'a private key'; Value = '-----BEGIN RSA PRIVATE KEY-----' }
            @{ Name = 'a password or token assignment'; Value = 'token = "abcd1234efgh"' }
        )

        function New-ScopeReview([string]$ScopeLine) {
            (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) -replace '(?m)^Scope:.*$', $ScopeLine
        }
        function Add-ReviewText([string]$Name, [string]$Text) {
            $dir = Join-Path $TestDrive 'reviews'
            New-Item -ItemType Directory $dir -Force | Out-Null
            Set-Content (Join-Path $dir $Name) $Text
        }
    }
    BeforeEach { Initialize-Fake }

    It 'replaces a personal path in a no-SHA Scope with ~ and warns, like any other field' {
        Add-ReviewText 'review-scope-path.md' (New-ScopeReview 'Scope: reviewed by hand, notes in /home/alice/project')
        $run = Invoke-Filing -Fixture 'review-scope-path.md'
        $run.Code | Should -Be 0
        # -BeLike would treat the backticks as its escape character.
        $run.Text | Should -Match ([regex]::Escape('reviewed commits `reviewed by hand, notes in ~/project`'))
        $run.Text | Should -BeLike '*replaced 1 personal path*'
        $run.Text | Should -Not -BeLike '*/home/alice*'
    }

    It 'keeps the path with -KeepPaths' {
        Add-ReviewText 'review-scope-path-keep.md' (New-ScopeReview 'Scope: reviewed by hand, notes in /home/alice/project')
        $run = Invoke-Filing -Fixture 'review-scope-path-keep.md' -Arguments @{ KeepPaths = $true }
        $run.Text | Should -Match ([regex]::Escape('reviewed commits `reviewed by hand, notes in /home/alice/project`'))
    }

    It 'warns about an e-mail address in a no-SHA Scope without changing it' {
        Add-ReviewText 'review-scope-email.md' (New-ScopeReview 'Scope: reviewed by alice@example.com')
        $run = Invoke-Filing -Fixture 'review-scope-email.md'
        $run.Code | Should -Be 0
        $run.Text | Should -Match ([regex]::Escape('reviewed commits `reviewed by alice@example.com`'))
        $run.Text | Should -BeLike '*e-mail address*'
    }

    It 'refuses <name> in a no-SHA Scope, naming it but not the value, before any gh write' -ForEach $secretCases {
        Add-ReviewText 'review-scope-secret.md' (New-ScopeReview "Scope: reviewed by hand, output: $Value")
        $run = Invoke-Filing -Fixture 'review-scope-secret.md'
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike "*Scope line 3 looks like $Name*"
        $run.Text | Should -Not -Match ([regex]::Escape($Value.Substring(6)))
        $script:fake.Writes | Should -BeNullOrEmpty
    }

    It 'refuses a review file name that looks like a secret, before any gh call' {
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        $name = 'sk-' + ('e' * 24) + '.md'
        Copy-Item (Join-Path $script:fixtures 'review-small.md') (Join-Path $dir $name) -Force
        $run = Invoke-Filing -Fixture $name
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*review file name looks like an sk- key*'
        $script:fake.Writes | Should -BeNullOrEmpty
    }
}

Describe 'Access checks stop before any write' {
    BeforeEach { Initialize-Fake }

    It 'refuses with no GitHub remote and no -Repo' {
        $script:fake.Remote = 'E:\local\path'
        $run = Invoke-Filing
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*origin isn''t a GitHub repo*pass -Repo*'
        $script:fake.Writes | Should -BeNullOrEmpty
    }

    It 'takes -Repo instead of the remote' {
        $script:fake.Remote = 'E:\local\path'
        (Invoke-Filing -Arguments @{ Repo = 'o/r' }).Code | Should -Be 0
    }

    It 'refuses when gh is not logged in' {
        $script:fake.LoggedIn = $false
        $run = Invoke-Filing
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*gh auth login*'
        $script:fake.Writes | Should -BeNullOrEmpty
    }

    It 'refuses a token without the repo scope' {
        $script:fake.Scopes = "'read:org'"
        $run = Invoke-Filing
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*gh auth refresh -s repo*'
    }

    It 'refuses when a needed label is missing, and creates none' {
        $script:fake.Labels = @('must-fix', 'should-fix')
        $run = Invoke-Filing
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike "*label 'question'*"
        $script:fake.Writes | Should -BeNullOrEmpty
        Should -Invoke gh -ModuleName file-review-issues -Times 0 -ParameterFilter { $args[0] -eq 'label' -and $args[1] -eq 'create' }
    }

    It 'does not need a label no selected finding uses' {
        $script:fake.Labels = @('must-fix')
        (Invoke-Filing -Arguments @{ Only = @('S-1', 'S-2') }).Code | Should -Be 0
    }

    It 'says the review stays pending when access fails' {
        $script:fake.LoggedIn = $false
        (Invoke-Filing).Text | Should -BeLike '*Issues: pending*'
    }
}

Describe 'Dry run' {
    BeforeEach { Initialize-Fake }

    It 'prints every issue and the Issues line, and calls no gh write command' {
        $run = Invoke-Filing
        $run.Code | Should -Be 0
        $run.Text | Should -BeLike '*[[]must-fix] S-1: Thing breaks*'
        $run.Text | Should -BeLike '*[[]must-fix] S-2: Message is wrong*'
        $run.Text | Should -BeLike '*[[]question] S-Q1: Keep the thing?*'
        $run.Text | Should -BeLike '*decision 7*'
        $run.Text | Should -BeLike '*review-finding: review-small.md#S-1*'
        $run.Text | Should -BeLike '*Issues: S-1 #?, S-2 #?, S-Q1 #?*'
        $script:fake.Writes | Should -BeNullOrEmpty
        Should -Invoke gh -ModuleName file-review-issues -Times 0 -ParameterFilter { $args[0] -eq 'issue' -and $args[1] -in 'create', 'edit', 'comment' }
    }

    It 'leaves the review file untouched' {
        $run = Invoke-Filing
        (Get-Content -Raw $run.Path) | Should -Be (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md'))
    }

    It 'files only the findings named by -Only' {
        $run = Invoke-Filing -Arguments @{ Only = @('S-2') }
        $run.Text | Should -BeLike '*S-2: Message is wrong*'
        $run.Text | Should -Not -BeLike '*S-1: Thing breaks*'
    }

    It 'refuses an unknown id in -Only' {
        $run = Invoke-Filing -Arguments @{ Only = @('S-9') }
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*S-9*'
    }

    It 'refuses findings with no acceptance criteria unless -StandardCriteria is given' {
        $run = Invoke-Filing -Fixture 'review-m2a-codex.md'
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*M2A-1 has no acceptance criteria; add them to the review file, or rerun with -StandardCriteria*'
        $run.Text | Should -BeLike '*nothing would be filed*'
    }

    It 'prints all four Codex findings and what is not filed with -StandardCriteria' {
        $run = Invoke-Filing -Fixture 'review-m2a-codex.md' -Arguments @{ StandardCriteria = $true }
        $run.Code | Should -Be 0
        foreach ($id in 'M2A-1', 'M2A-2', 'M2A-3', 'M2A-Q1') { $run.Text | Should -BeLike "*$id*" }
        $run.Text | Should -BeLike '*not filed: 2 sections, 1 nit*'
    }

    It 'refuses a finding that holds a secret, naming the id and line' {
        $text = (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) -replace 'It breaks\.', ('It breaks with ghp_' + ('a' * 30))
        $dir = Join-Path $TestDrive 'reviews'
        New-Item -ItemType Directory $dir -Force | Out-Null
        Set-Content (Join-Path $dir 'review-secret.md') $text
        $run = Invoke-Filing -Fixture 'review-secret.md'
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*S-1*line 11*a GitHub token*'
        $run.Text | Should -Not -BeLike '*aaaaaaaa*'
    }

    It 'fails on a review file that does not exist' {
        $all = Invoke-ReviewFiling -Review (Join-Path $TestDrive 'nope.md') 6>&1
        @($all | Where-Object { $_ -is [int] })[-1] | Should -Be 1
    }
}

Describe 'Create' {
    BeforeEach { Initialize-Fake }

    It 'creates issues in dependency order with the label and title' {
        $run = Invoke-Filing -Arguments @{ Create = $true }
        $run.Code | Should -Be 0
        @($script:fake.Issues | ForEach-Object { $_.title }) | Should -Be @('S-2: Message is wrong', 'S-1: Thing breaks', 'S-Q1: Keep the thing?')
        @($script:fake.Issues | ForEach-Object { $_.label }) | Should -Be @('must-fix', 'must-fix', 'question')
        @($script:fake.Issues | ForEach-Object { $_.number }) | Should -Be @(40, 41, 42)
    }

    It 'writes Depends on as #n in the dependent and the reverse Related in the dependency' {
        Invoke-Filing -Arguments @{ Create = $true } | Out-Null
        $s1 = $script:fake.Issues | Where-Object { $_.number -eq 41 }
        $s2 = $script:fake.Issues | Where-Object { $_.number -eq 40 }
        $s1.body | Should -BeLike '*Depends on:** #40 (the message must match)*'
        $s2.body | Should -BeLike '*Related:** #41 depends on this one*'
        @($script:fake.Writes) | Should -Be @('create', 'create', 'create', 'edit 40')
    }

    It 'prints a table with id, label, number, url and what happened' {
        $run = Invoke-Filing -Arguments @{ Create = $true }
        $run.Text | Should -BeLike '*S-1*must-fix*41*https://github.com/o/r/issues/41*created*'
    }

    It 'removes its temp body files' {
        Invoke-Filing -Arguments @{ Create = $true } | Out-Null
        $script:fake.BodyFiles.Count | Should -Be 4   # three creates and one edit
        foreach ($file in $script:fake.BodyFiles) { Test-Path $file | Should -BeFalse }
    }

    It 'a second run creates nothing and reuses the numbers' {
        Invoke-Filing -Arguments @{ Create = $true } | Out-Null
        $script:fake.Writes.Clear()
        $run = Invoke-Filing -Arguments @{ Create = $true }
        $run.Code | Should -Be 0
        $script:fake.Writes | Should -BeNullOrEmpty
        $script:fake.Issues.Count | Should -Be 3
        $run.Text | Should -BeLike '*S-1*41*skipped (exists)*'
    }

    It 'reuses the number of a finding filed earlier in links' {
        $script:fake.Issues.Add(@{ number = 99; title = 'S-2: old'; body = "x`n<!-- review-finding: review-small.md#S-2 -->`n" })
        Invoke-Filing -Arguments @{ Create = $true } | Out-Null
        $s1 = $script:fake.Issues | Where-Object { $_.title -eq 'S-1: Thing breaks' }
        $s1.body | Should -BeLike '*Depends on:** #99 (the message must match)*'
        $script:fake.Issues.Count | Should -Be 3   # the old one, S-1 and S-Q1
    }

    It 'shows skipped findings in the dry run too' {
        $script:fake.Issues.Add(@{ number = 99; title = 'S-2: old'; body = "x`n<!-- review-finding: review-small.md#S-2 -->`n" })
        (Invoke-Filing).Text | Should -BeLike '*S-2*skipped (exists #99)*'
    }

    It 'resumes after an interruption with only the findings that are missing' {
        $script:fake.FailCreateAt = 3
        $first = Invoke-Filing -Fixture 'review-m2a-codex.md' -Arguments @{ Create = $true; StandardCriteria = $true }
        $first.Code | Should -Be 2
        $script:fake.Issues.Count | Should -Be 2
        $script:fake.FailCreateAt = 0
        $second = Invoke-Filing -Fixture 'review-m2a-codex.md' -Arguments @{ Create = $true; StandardCriteria = $true }
        $second.Code | Should -Be 0
        $script:fake.Issues.Count | Should -Be 4
        @($script:fake.Issues | ForEach-Object { $_.title } | Sort-Object -Unique).Count | Should -Be 4
    }

    It 'stops with an error when the issue list is as long as the limit' {
        1..1000 | ForEach-Object { $script:fake.Issues.Add(@{ number = $_; title = "t$_"; body = '' }) }
        $run = Invoke-Filing -Arguments @{ Create = $true }
        $run.Code | Should -Be 1
        $run.Text | Should -BeLike '*1000*'
        $script:fake.Writes | Should -BeNullOrEmpty
    }

    It 'files only the findings named by -Only' {
        $run = Invoke-Filing -Arguments @{ Create = $true; Only = @('S-1') }
        $run.Code | Should -Be 0
        @($script:fake.Issues | ForEach-Object { $_.title }) | Should -Be @('S-1: Thing breaks')
    }

    It 'files nothing when any finding is refused' {
        $run = Invoke-Filing -Fixture 'review-m2a-codex.md' -Arguments @{ Create = $true }
        $run.Code | Should -Be 1
        $script:fake.Writes | Should -BeNullOrEmpty
        $run.Text | Should -BeLike '*nothing was filed*'
    }
}

Describe 'Write-back of the Issues line' {
    BeforeEach { Initialize-Fake }

    BeforeAll {
        function Get-FixtureText { (Get-Content -Raw (Join-Path $script:fixtures 'review-small.md')) -replace "`r`n", "`n" }
        function Add-ReviewFile([string]$Name, [byte[]]$Bytes) {
            $dir = Join-Path $TestDrive 'reviews'
            New-Item -ItemType Directory $dir -Force | Out-Null
            [IO.File]::WriteAllBytes((Join-Path $dir $Name), $Bytes)
        }
    }

    It 'replaces the Issues line and changes nothing else (byte for byte)' {
        $original = Get-FixtureText
        Add-ReviewFile 'r-lf.md' ([Text.Encoding]::UTF8.GetBytes($original))
        $run = Invoke-Filing -Fixture 'r-lf.md' -Arguments @{ Create = $true }
        $expected = $original.Replace('Issues: pending (test)', 'Issues: S-1 #41, S-2 #40, S-Q1 #42')
        [IO.File]::ReadAllBytes($run.Path) | Should -Be ([Text.Encoding]::UTF8.GetBytes($expected))
    }

    It 'keeps CRLF line endings' {
        $original = (Get-FixtureText) -replace "`n", "`r`n"
        Add-ReviewFile 'r-crlf.md' ([Text.Encoding]::UTF8.GetBytes($original))
        $run = Invoke-Filing -Fixture 'r-crlf.md' -Arguments @{ Create = $true }
        $expected = $original.Replace('Issues: pending (test)', 'Issues: S-1 #41, S-2 #40, S-Q1 #42')
        [IO.File]::ReadAllBytes($run.Path) | Should -Be ([Text.Encoding]::UTF8.GetBytes($expected))
    }

    It 'keeps a byte order mark' {
        $original = Get-FixtureText
        $bytes = [byte[]](0xEF, 0xBB, 0xBF) + [Text.Encoding]::UTF8.GetBytes($original)
        Add-ReviewFile 'r-bom.md' $bytes
        $run = Invoke-Filing -Fixture 'r-bom.md' -Arguments @{ Create = $true }
        $after = [IO.File]::ReadAllBytes($run.Path)
        $after[0..2] | Should -Be @(0xEF, 0xBB, 0xBF)
        [Text.Encoding]::UTF8.GetString($after, 3, $after.Length - 3) | Should -BeLike '*Issues: S-1 #41, S-2 #40, S-Q1 #42*'
    }

    It 'leaves findings that were not filed as pending' {
        $run = Invoke-Filing -Arguments @{ Create = $true; Only = @('S-1') }
        (Get-Content $run.Path | Where-Object { $_ -like 'Issues:*' }) | Should -Be 'Issues: S-1 #40, S-2 pending, S-Q1 pending'
    }

    It 'writes the numbers that exist after an interrupted run' {
        $script:fake.FailCreateAt = 2
        $run = Invoke-Filing -Arguments @{ Create = $true }
        $run.Code | Should -Be 2
        (Get-Content $run.Path | Where-Object { $_ -like 'Issues:*' }) | Should -Be 'Issues: S-1 pending, S-2 #40, S-Q1 pending'
    }

    It 'rewrites the Codex review''s long Issues line to the short form' {
        $run = Invoke-Filing -Fixture 'review-m2a-codex.md' -Arguments @{ Create = $true; StandardCriteria = $true }
        (Get-Content $run.Path | Where-Object { $_ -like 'Issues:*' }).Count | Should -Be 1
        (Get-Content $run.Path | Where-Object { $_ -like 'Issues:*' }) | Should -BeLike 'Issues: M2A-1 #*, M2A-2 #*, M2A-3 #*, M2A-Q1 #*'
    }

    It 'does not touch the file on a dry run' {
        $run = Invoke-Filing
        [IO.File]::ReadAllBytes($run.Path) | Should -Be ([IO.File]::ReadAllBytes((Join-Path $script:fixtures 'review-small.md')))
    }
}
