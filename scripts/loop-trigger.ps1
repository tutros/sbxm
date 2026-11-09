<#
.SYNOPSIS
The scheduled start of `sbxm task loop` (decision 310, issue 527): a Task Scheduler job at the user's logon runs
the loop with --wait, so no terminal hosts it, and starts it again after an error.

.DESCRIPTION
With -Register, registers (or replaces) the scheduled task `sbxm task loop` for the current user: at logon, only
while the user is logged on (Docker Desktop and sbx run in the user's session), one instance at a time, no time
limit. It runs this script with no switch. -Unregister removes it.

Without a switch, the script:
1. In the loop checkout (-Checkout), refuses to start when `git status --porcelain` shows local changes, saying
   why and overwriting nothing; otherwise runs `git fetch` and `git checkout --detach origin/main`.
2. Waits up to 10 minutes for `sbx ls` to succeed, checking every 30 seconds; still failing counts as an error.
3. Runs `<wrapper> task loop --wait` in the checkout, through the wrapper path given (-Sbxm), never a build.
4. After an exit that is neither 0 (a stop request) nor 3 (a stop that needs the user), waits 10 minutes and
   starts it again from step 2, at most 3 times in an hour: exit 0 or 3, or a 4th failure within the hour, leaves
   it down until the next logon or a start by hand.

Stop the running loop with `sbxm task loop --stop`; it is back at the next logon.

.EXAMPLE
./scripts/loop-trigger.ps1 -Register
./scripts/loop-trigger.ps1 -Unregister
#>
param(
    [switch]$Register,
    [switch]$Unregister,
    # The loop's own checkout of the repo, reset to origin/main at each start.
    [string]$Checkout = 'H:\sbxm-e2e\loop-checkout',
    # The wrapper that runs the known sbxm binary.
    [string]$Sbxm = 'H:\sbxm-e2e\sbxm.ps1',
    # The commands and the clock, parameters only so tests can stand in for them.
    [scriptblock]$Git = { git @args },
    [scriptblock]$Sbx = { sbx @args },
    [scriptblock]$Sleep = { param($seconds) Start-Sleep -Seconds $seconds },
    [scriptblock]$Now = { Get-Date }
)

$ErrorActionPreference = 'Stop'
# A tool's exit code is read here, not turned into an error by PowerShell.
$PSNativeCommandUseErrorActionPreference = $false
$taskName = 'sbxm task loop'

# The loop's stops that need the user (a lapsed sign-in, a missing secret, two parked tasks, the usage waits'
# total): never restarted.
$needsUser = 3
$restartsPerHour = 3
$restartAfter = 10 * 60
$sbxWait = 10 * 60
$sbxPoll = 30

if ($Register) {
    $pwsh = (Get-Command pwsh).Source
    $arguments = "-NoLogo -NoProfile -WindowStyle Hidden -File `"$PSCommandPath`" -Checkout `"$Checkout`" -Sbxm `"$Sbxm`""
    $action = New-ScheduledTaskAction -Execute $pwsh -Argument $arguments -WorkingDirectory $Checkout
    $user = "$env:USERDOMAIN\$env:USERNAME"
    $trigger = New-ScheduledTaskTrigger -AtLogOn -User $user
    # Interactive: runs only while the user is logged on, in the user's session.
    $principal = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive
    # A zero time limit is none; IgnoreNew: a second start while one runs does nothing.
    $settings = New-ScheduledTaskSettingsSet -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero)
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Principal $principal -Settings $settings -Force | Out-Null
    Write-Host "Registered '$taskName': at the logon of $user, in the checkout $Checkout."
    return
}

if ($Unregister) {
    if (-not (Get-ScheduledTask -TaskName $taskName -ErrorAction SilentlyContinue)) {
        Write-Host "No '$taskName' is registered; nothing to remove."
        return
    }
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false
    Write-Host "Removed '$taskName'; a loop running now keeps running until 'sbxm task loop --stop'."
    return
}

# Step 1: the checkout, on origin/main, with nothing of its own overwritten.
$changes = @(& $Git -C $Checkout status --porcelain)
if ($LASTEXITCODE -ne 0) {
    Write-Host "Not starting the loop: 'git status' failed in $Checkout (exit code $LASTEXITCODE)."
    exit 1
}
if ($changes | Where-Object { "$_".Trim() }) {
    Write-Host "Not starting the loop: the checkout $Checkout has local changes; commit, stash or remove them first:"
    $changes | ForEach-Object { Write-Host "  $_" }
    exit 1
}
foreach ($step in @('fetch'), @('checkout', '--detach', 'origin/main')) {
    & $Git -C $Checkout @step | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Not starting the loop: 'git $($step -join ' ')' failed in $Checkout (exit code $LASTEXITCODE)."
        exit 1
    }
}

# Step 2: true once `sbx ls` succeeds, false after 10 minutes of failures.
# Measured on the clock, so the time each `sbx ls` takes counts too; no sleep runs past the deadline.
function Wait-Sbx {
    $deadline = (& $Now).AddSeconds($sbxWait)
    while ($true) {
        & $Sbx ls *> $null
        if ($LASTEXITCODE -eq 0) { return $true }
        $left = [int][Math]::Ceiling(($deadline - (& $Now)).TotalSeconds)
        if ($left -le 0) { return $false }
        & $Sleep ([Math]::Min($sbxPoll, $left))
    }
}

# Steps 3 and 4: the loop, started again after an error under the restart rule.
$failures = @()
while ($true) {
    if (Wait-Sbx) {
        Write-Host 'Starting sbxm task loop --wait.'
        Push-Location -LiteralPath $Checkout
        try {
            $global:LASTEXITCODE = 0
            & $Sbxm task loop --wait | Out-Host
            $code = $LASTEXITCODE
        }
        finally { Pop-Location }
    }
    else {
        Write-Host "sbx ls still fails after $($sbxWait / 60) minutes; Docker Desktop or sbx is not up."
        $code = 1
    }
    if ($code -eq 0) {
        Write-Host 'The loop stopped on request; it starts again at the next logon.'
        exit 0
    }
    if ($code -eq $needsUser) {
        Write-Host 'The loop stopped on something that needs you (see its log); it starts again at the next logon.'
        exit $code
    }
    $failedAt = & $Now
    $failures = @($failures | Where-Object { ($failedAt - $_).TotalSeconds -lt 3600 }) + $failedAt
    if ($failures.Count -gt $restartsPerHour) {
        Write-Host "The loop failed (exit code $code) $($failures.Count) times within an hour; left down until the next logon or a start by hand."
        exit $code
    }
    Write-Host "The loop failed (exit code $code); starting it again in $($restartAfter / 60) minutes (restart $($failures.Count) of $restartsPerHour this hour)."
    & $Sleep $restartAfter
}
