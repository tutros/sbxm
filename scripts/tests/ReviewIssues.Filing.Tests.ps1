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
                    $script:fake.Created++
                    if ($script:fake.FailCreateAt -eq $script:fake.Created) { $global:LASTEXITCODE = 1; return }
                    $number = $script:fake.Next++
                    $script:fake.Issues.Add(@{ number = $number; title = (& $get '--title'); body = (Get-Content -Raw (& $get '--body-file')); label = (& $get '--label') })
                    return "https://github.com/o/r/issues/$number"
                }
                'issue edit' {
                    $script:fake.Writes.Add("edit $($a[2])")
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
        if (-not (Test-Path $path)) { Copy-Item (Join-Path $script:fixtures $Fixture) $path }
        $all = Invoke-ReviewFiling -Review $path @Arguments 6>&1 3>&1
        [pscustomobject]@{
            Code = @($all | Where-Object { $_ -is [int] })[-1]
            Text = (($all | Where-Object { $_ -isnot [int] } | ForEach-Object { if ($_ -is [System.Management.Automation.InformationRecord]) { "$($_.MessageData)" } else { "$_" } }) -join "`n")
            Path = $path
        }
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
