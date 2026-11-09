BeforeAll {
    $script:trigger = Join-Path $PSScriptRoot '..\loop-trigger.ps1'

    # A stand-in for the sbxm wrapper: each run logs `loop <arguments>` to $log, takes the first line of $codes
    # (`<exit code>` or `<exit code>@<seconds it runs>`), advances the test clock by those seconds, and exits
    # with the code.
    function New-FakeSbxm([string]$dir, [string[]]$codes) {
        $codesFile = Join-Path $dir 'codes.txt'
        Set-Content -LiteralPath $codesFile -Value $codes
        $fake = Join-Path $dir 'sbxm.ps1'
        Set-Content -LiteralPath $fake -Value @"
`$codes = @(Get-Content -LiteralPath '$codesFile')
Set-Content -LiteralPath '$codesFile' -Value @(`$codes | Select-Object -Skip 1)
Add-Content -LiteralPath '$(Join-Path $dir 'calls.txt')' -Value "loop `$args in `$((Get-Location).Path)"
`$code, `$runs = "`$(`$codes[0])" -split '@'
if (`$runs) { `$global:fakeClock = `$global:fakeClock.AddSeconds([int]`$runs) }
exit [int]`$code
"@
        $fake
    }

    # A stand-in command logging each call as `<name> <arguments>` to calls.txt in $dir, answering with the
    # exit code (and output) the scriptblock $answer gives for the joined arguments.
    function New-Tool([string]$dir, [string]$name, [scriptblock]$answer) {
        $log = Join-Path $dir 'calls.txt'
        {
            $line = $args -join ' '
            Add-Content -LiteralPath $log -Value "$name $line"
            $result = & $answer $line
            $global:LASTEXITCODE = $result.code
            $result.out
        }.GetNewClosure()
    }

    function Get-Calls([string]$dir) {
        $log = Join-Path $dir 'calls.txt'
        if (Test-Path -LiteralPath $log) { @(Get-Content -LiteralPath $log) } else { @() }
    }

    # Runs the trigger with stand-ins: a clean checkout, sbx answering $sbx, the loop exiting with $codes in
    # turn. The clock starts at 08:00 and moves on each sleep (also logged as `sleep <seconds>`).
    function Invoke-Trigger([string[]]$codes, [scriptblock]$sbx = { @{ code = 0 } }, [scriptblock]$git = { @{ code = 0 } }) {
        $dir = Join-Path $TestDrive ([guid]::NewGuid())
        $checkout = Join-Path $dir 'checkout'
        New-Item -ItemType Directory -Force $checkout | Out-Null
        $global:fakeClock = [datetime]'2026-11-09T08:00:00'
        $log = Join-Path $dir 'calls.txt'
        $params = @{
            Checkout = $checkout
            Sbxm     = New-FakeSbxm $dir $codes
            Git      = New-Tool $dir 'git' $git
            Sbx      = New-Tool $dir 'sbx' $sbx
            Sleep    = { param($seconds) Add-Content -LiteralPath $log -Value "sleep $seconds"; $global:fakeClock = $global:fakeClock.AddSeconds($seconds) }.GetNewClosure()
            Now      = { $global:fakeClock }
        }
        $global:LASTEXITCODE = 0
        $output = & $script:trigger @params *>&1 | Out-String
        @{ code = $LASTEXITCODE; output = $output; calls = Get-Calls $dir; checkout = $checkout }
    }

    function Get-Loops($run) { @($run.calls | Where-Object { $_ -like 'loop *' }) }
}

Describe 'loop-trigger.ps1: the restart rule' {
    It 'restarts after each of three failures within the hour, and not after the fourth' {
        $run = Invoke-Trigger @('1', '1', '1', '1', '0')
        $run.code | Should -Be 1
        (Get-Loops $run).Count | Should -Be 4
        @($run.calls | Where-Object { $_ -eq 'sleep 600' }).Count | Should -Be 3
        $run.output | Should -Match 'left down'
    }

    It 'does not restart after a 0 that follows a failure' {
        $run = Invoke-Trigger @('1', '0', '1')
        $run.code | Should -Be 0
        (Get-Loops $run).Count | Should -Be 2
    }

    It 'does not restart after exit code 3' {
        $run = Invoke-Trigger @('3', '1')
        $run.code | Should -Be 3
        (Get-Loops $run).Count | Should -Be 1
        @($run.calls | Where-Object { $_ -like 'sleep *' }).Count | Should -Be 0
    }

    It 'restarts each failure when they are more than an hour apart' {
        $run = Invoke-Trigger @('1@3700', '1@3700', '1@3700', '1@3700', '1@3700', '0')
        $run.code | Should -Be 0
        (Get-Loops $run).Count | Should -Be 6
    }

    It 'runs the loop with --wait in the checkout' {
        $run = Invoke-Trigger @('0')
        (Get-Loops $run) | Should -Be @("loop task loop --wait in $($run.checkout)")
    }
}

Describe 'loop-trigger.ps1: the start' {
    It 'refuses a checkout with local changes, with no fetch, checkout or loop' {
        $git = { param($line) if ($line -like '*status --porcelain') { @{ code = 0; out = ' M src/main.rs' } } else { @{ code = 0 } } }
        $run = Invoke-Trigger @('0') -git $git
        $run.code | Should -Be 1
        $run.output | Should -Match 'local changes'
        $run.output | Should -Match 'src/main.rs'
        @($run.calls | Where-Object { $_ -like 'git *' }) | Should -Be @("git -C $($run.checkout) status --porcelain")
        (Get-Loops $run).Count | Should -Be 0
    }

    It 'fetches and checks out origin/main detached before the loop' {
        $run = Invoke-Trigger @('0')
        @($run.calls | Where-Object { $_ -like 'git *' }) | Should -Be @(
            "git -C $($run.checkout) status --porcelain"
            "git -C $($run.checkout) fetch"
            "git -C $($run.checkout) checkout --detach origin/main"
        )
    }

    It 'counts sbx ls failing for 10 minutes as an error under the restart rule' {
        # Down for the first 21 checks (0 s to 600 s, 30 s apart), then up.
        $script:tries = 0
        $sbx = { $script:tries++; @{ code = $(if ($script:tries -gt 21) { 0 } else { 1 }) } }
        $run = Invoke-Trigger @('0') -sbx $sbx
        $run.code | Should -Be 0
        $run.output | Should -Match 'sbx ls still fails'
        $run.output | Should -Match 'restart 1 of 3'
        @($run.calls | Where-Object { $_ -eq 'sleep 30' }).Count | Should -Be 20
        @($run.calls | Where-Object { $_ -eq 'sleep 600' }).Count | Should -Be 1
        @($run.calls | Where-Object { $_ -eq 'sbx ls' }).Count | Should -Be 22
        (Get-Loops $run).Count | Should -Be 1
    }

    It 'counts the time sbx ls itself takes toward the 10 minutes' {
        # Each check takes 60 s; checks 1 to 7 fail (the 7th ends at 600 s with the 30 s sleeps), the 8th would pass.
        $script:tries = 0
        $sbx = {
            $script:tries++
            $global:fakeClock = $global:fakeClock.AddSeconds(60)
            @{ code = $(if ($script:tries -ge 8) { 0 } else { 1 }) }
        }
        $run = Invoke-Trigger @('0') -sbx $sbx
        $run.output | Should -Match 'sbx ls still fails after 10 minutes'
        # The first attempt stops at its 7th check; the restart's first check (the 8th) passes.
        @($run.calls | Where-Object { $_ -eq 'sbx ls' }).Count | Should -Be 8
        @($run.calls | Where-Object { $_ -eq 'sleep 600' }).Count | Should -Be 1
        (Get-Loops $run).Count | Should -Be 1
    }

    It 'never sleeps past the 10 minutes' {
        # Each check takes 25 s and each cycle 55 s: the 11th check ends at 575 s, 25 s before the deadline, so the
        # last sleep is 25 s, and the 12th check (ending at 625 s) times the attempt out. Then sbx is up.
        $script:tries = 0
        $sbx = {
            $script:tries++
            $global:fakeClock = $global:fakeClock.AddSeconds(25)
            @{ code = $(if ($script:tries -gt 12) { 0 } else { 1 }) }
        }
        $run = Invoke-Trigger @('0') -sbx $sbx
        $run.output | Should -Match 'sbx ls still fails after 10 minutes'
        @($run.calls | Where-Object { $_ -eq 'sleep 30' }).Count | Should -Be 10
        $run.calls | Should -Contain 'sleep 25'
        @($run.calls | Where-Object { $_ -eq 'sbx ls' }).Count | Should -Be 13
    }

    It 'starts the loop once sbx ls succeeds on the 3rd try' {
        $script:tries = 0
        $sbx = { $script:tries++; @{ code = $(if ($script:tries -ge 3) { 0 } else { 1 }) } }
        $run = Invoke-Trigger @('0') -sbx $sbx
        $run.code | Should -Be 0
        (Get-Loops $run).Count | Should -Be 1
        @($run.calls | Where-Object { $_ -eq 'sbx ls' }).Count | Should -Be 3
        @($run.calls | Where-Object { $_ -eq 'sleep 30' }).Count | Should -Be 2
    }
}

Describe 'loop-trigger.ps1: -Register and -Unregister' {
    BeforeAll {
        # Stand-ins for the ScheduledTasks cmdlets: functions are found before cmdlets, so the real scheduler
        # is never touched. The registered jobs are kept by name.
        $global:scheduled = @{}
        function global:New-ScheduledTaskAction { param($Execute, $Argument, $WorkingDirectory) @{ Execute = $Execute; Argument = $Argument; WorkingDirectory = $WorkingDirectory } }
        function global:New-ScheduledTaskTrigger { param([switch]$AtLogOn, $User) @{ AtLogOn = [bool]$AtLogOn; User = $User } }
        function global:New-ScheduledTaskPrincipal { param($UserId, $LogonType) @{ UserId = $UserId; LogonType = $LogonType } }
        function global:New-ScheduledTaskSettingsSet { param($MultipleInstances, $ExecutionTimeLimit) @{ MultipleInstances = $MultipleInstances; ExecutionTimeLimit = $ExecutionTimeLimit } }
        function global:Register-ScheduledTask {
            param($TaskName, $Action, $Trigger, $Principal, $Settings, [switch]$Force)
            if ($global:scheduled.ContainsKey($TaskName) -and -not $Force) { throw "$TaskName exists" }
            $global:scheduled[$TaskName] = @{ Action = $Action; Trigger = $Trigger; Principal = $Principal; Settings = $Settings }
        }
        function global:Get-ScheduledTask { param($TaskName, $ErrorAction) $global:scheduled[$TaskName] }
        function global:Unregister-ScheduledTask { param($TaskName, [switch]$Confirm) $global:scheduled.Remove($TaskName) }
    }

    AfterAll {
        foreach ($name in 'New-ScheduledTaskAction', 'New-ScheduledTaskTrigger', 'New-ScheduledTaskPrincipal',
            'New-ScheduledTaskSettingsSet', 'Register-ScheduledTask', 'Get-ScheduledTask', 'Unregister-ScheduledTask') {
            Remove-Item -LiteralPath "function:global:$name" -ErrorAction SilentlyContinue
        }
        Remove-Variable -Name scheduled -Scope Global -ErrorAction SilentlyContinue
    }

    BeforeEach { $global:scheduled.Clear() }

    It 'registers the job at logon, while logged on, one instance, no time limit; twice replaces it' {
        & $script:trigger -Register -Checkout 'H:\loop' -Sbxm 'H:\sbxm.ps1' *>&1 | Out-Null
        & $script:trigger -Register -Checkout 'H:\loop' -Sbxm 'H:\sbxm.ps1' *>&1 | Out-Null
        $global:scheduled.Keys | Should -Be @('sbxm task loop')
        $job = $global:scheduled['sbxm task loop']
        $job.Trigger.AtLogOn | Should -BeTrue
        $job.Principal.LogonType | Should -Be 'Interactive'
        "$($job.Settings.MultipleInstances)" | Should -Be 'IgnoreNew'
        $job.Settings.ExecutionTimeLimit | Should -Be ([TimeSpan]::Zero)
        $job.Action.Argument | Should -Match ([regex]::Escape('-Checkout "H:\loop" -Sbxm "H:\sbxm.ps1"'))
        $job.Action.WorkingDirectory | Should -Be 'H:\loop'
    }

    It '-Unregister removes the job' {
        & $script:trigger -Register *>&1 | Out-Null
        $output = & $script:trigger -Unregister *>&1 | Out-String
        $global:scheduled.Count | Should -Be 0
        $output | Should -Match 'Removed'
    }

    It '-Unregister with none registered says so and exits 0' {
        $global:LASTEXITCODE = 0
        $output = & $script:trigger -Unregister *>&1 | Out-String
        $LASTEXITCODE | Should -Be 0
        $output | Should -Match 'No .sbxm task loop. is registered'
    }
}
