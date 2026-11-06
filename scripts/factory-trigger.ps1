<#
.SYNOPSIS
The Windows trigger of the factory check (decision 269(b), issue 327): a Task Scheduler job daily at 07:00, run at
the next chance if missed, and a toast when a run ends. Only this file is Windows-specific; the logic is in
factory-check.ps1, so the trigger can move to cron or a cloud schedule later.

.DESCRIPTION
With -Register, registers (or replaces) the scheduled task `sbxm factory check` for the current user, which runs
this script without -Register. Without it, runs factory-check.ps1 and shows a toast with the end of that day's
summary.md (the guard's skip, the drafts written, or the failure). Remove the job with
Unregister-ScheduledTask -TaskName 'sbxm factory check'.

.EXAMPLE
./scripts/factory-trigger.ps1 -Register
#>
param(
    [switch]$Register,
    [string]$At = '07:00'
)

$ErrorActionPreference = 'Stop'
$taskName = 'sbxm factory check'
$check = Join-Path $PSScriptRoot 'factory-check.ps1'

if ($Register) {
    $pwsh = (Get-Command pwsh).Source
    $action = New-ScheduledTaskAction -Execute $pwsh -Argument "-NoLogo -NoProfile -File `"$PSCommandPath`"" -WorkingDirectory $PSScriptRoot
    $trigger = New-ScheduledTaskTrigger -Daily -At $At
    # -StartWhenAvailable: a run missed while the machine was off or asleep starts at the next chance.
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -MultipleInstances IgnoreNew -ExecutionTimeLimit (New-TimeSpan -Hours 4)
    Register-ScheduledTask -TaskName $taskName -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
    Write-Host "Registered '$taskName': daily at $At, run at the next chance if missed."
    return
}

try {
    $global:LASTEXITCODE = 0
    $output = @(& $check *>&1 | ForEach-Object { "$_" })
    $code = $LASTEXITCODE
}
catch {
    # A stop before the summary (sbxm's config, no checkout) still gets its toast.
    $output = @("$_")
    $code = 1
}
$lines = @($output | Where-Object { $_.Trim() } | Select-Object -Last 4)
$title = if ($code -eq 0) { 'sbxm factory check' } else { "sbxm factory check failed (exit code $code)" }

# A toast needs the WinRT types, which only Windows PowerShell 5.1 loads; the text goes in as XML-escaped strings.
$escape = { param($text) [Security.SecurityElement]::Escape($text) }
$body = ($lines | ForEach-Object { "<text>$(& $escape $_)</text>" } | Select-Object -First 2) -join ''
$xml = "<toast><visual><binding template=`"ToastGeneric`"><text>$(& $escape $title)</text>$body</binding></visual></toast>"
$toast = @"
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
`$doc = New-Object Windows.Data.Xml.Dom.XmlDocument
`$doc.LoadXml([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('$([Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($xml)))')))
`$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier(`$app).Show([Windows.UI.Notifications.ToastNotification]::new(`$doc))
"@
powershell.exe -NoLogo -NoProfile -Command $toast
exit $code
