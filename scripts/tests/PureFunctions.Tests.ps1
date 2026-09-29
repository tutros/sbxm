BeforeAll {
    Import-Module (Join-Path $PSScriptRoot '..\issue-workers.psm1') -Force
}

Describe 'Get-AgentCommand' {
    It 'runs claude headless on the prompt file' {
        Get-AgentCommand 'claude' '' '/p/prompt.md' |
            Should -Be 'claude -p "$(cat /p/prompt.md)" --output-format text'
    }

    It 'adds --model for claude when a model is given' {
        Get-AgentCommand 'claude' 'claude-opus-5-5' '/p/prompt.md' |
            Should -Be 'claude -p "$(cat /p/prompt.md)" --output-format text --model claude-opus-5-5'
    }

    It 'runs codex exec with high reasoning effort and no model by default' {
        Get-AgentCommand 'codex' '' '/p/prompt.md' |
            Should -Be 'codex exec -c ''model_reasoning_effort="high"'' "$(cat /p/prompt.md)"'
    }

    It 'adds -m for codex when a model is given' {
        Get-AgentCommand 'codex' 'gpt-5.6-sol' '/p/prompt.md' |
            Should -Be 'codex exec -m gpt-5.6-sol -c ''model_reasoning_effort="high"'' "$(cat /p/prompt.md)"'
    }
}

Describe 'Get-MustFixCount' {
    BeforeEach { $file = Join-Path $TestDrive 'review.md' }

    It 'reads the count from the first line' {
        Set-Content $file "Must-fix findings: 2`nrest"
        Get-MustFixCount $file | Should -Be 2
    }

    It 'reads zero' {
        Set-Content $file 'Must-fix findings: 0'
        Get-MustFixCount $file | Should -Be 0
    }

    It 'returns null when the line is missing' {
        Set-Content $file "Looks good`nMust-fix findings: 1"
        Get-MustFixCount $file | Should -BeNullOrEmpty
    }
}

Describe 'Get-IssueNumbers' {
    It 'lists the issue numbers on a field line' {
        Get-IssueNumbers "**Depends on:** #3, #12`n**Related:** #5" 'Depends on' | Should -Be @(3, 12)
    }

    It 'returns nothing when the field is missing' {
        Get-IssueNumbers '**Related:** #5' 'Depends on' | Should -BeNullOrEmpty
    }
}
