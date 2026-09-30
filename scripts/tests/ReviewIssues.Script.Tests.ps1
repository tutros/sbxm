BeforeAll {
    $script:entry = Join-Path $PSScriptRoot '..\file-review-issues.ps1'
}

Describe 'file-review-issues.ps1' {
    It 'exists next to its module' {
        Test-Path $script:entry | Should -BeTrue
        Test-Path (Join-Path $PSScriptRoot '..\file-review-issues.psm1') | Should -BeTrue
    }

    It 'takes the parameters the spec lists' {
        $names = (Get-Command $script:entry).Parameters.Keys
        foreach ($name in 'Review', 'Repo', 'Create', 'StandardCriteria', 'KeepPaths', 'Only') { $names | Should -Contain $name }
        (Get-Command $script:entry).Parameters['Create'].SwitchParameter | Should -BeTrue
    }

    It 'documents every parameter in its comment-based help' {
        $help = Get-Help $script:entry -Full
        $documented = $help.parameters.parameter | Where-Object { $_.description } | ForEach-Object { $_.name }
        foreach ($name in 'Review', 'Repo', 'Create', 'StandardCriteria', 'KeepPaths', 'Only') { $documented | Should -Contain $name }
        $help.Synopsis | Should -Not -BeNullOrEmpty
        ($help.examples | Out-String) | Should -BeLike '*-Create*'
    }

    It 'says dry run by default and exit codes in the help' {
        $text = Get-Help $script:entry -Full | Out-String
        $text | Should -BeLike '*dry run*'
        $text | Should -BeLike '*Exit codes*'
    }

    It 'exits 1 for a review file that does not exist' {
        $output = pwsh -NoProfile -File $script:entry -Review (Join-Path $TestDrive 'nope.md') 2>&1 | Out-String
        $LASTEXITCODE | Should -Be 1
        $output | Should -BeLike '*not found*'
    }
}
