# E2E: update check. A fake GitHub release feed (v9.9.0) must show the update bar at start-up and in
# Settings › Updates; with the real feed, Check now must report a result (latest, or why GitHub said no).
# Usage: powershell -File platforms/windows/tests/e2e/update-e2e.ps1 [-Exe <Clipingo.exe>]
param([string]$Exe = '')
$ErrorActionPreference = 'Stop'
if (-not $Exe) {
    $root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))))
    $Exe = Get-ChildItem (Join-Path $root 'platforms\windows\Clipingo\bin') -Recurse -Filter Clipingo.exe | Select-Object -First 1 -ExpandProperty FullName
}
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]; $T = [System.Windows.Automation.TreeScope]
$P = [System.Windows.Automation.PropertyCondition]; $CT = [System.Windows.Automation.ControlType]
function Wait-Until([scriptblock]$cond, [int]$seconds, [string]$what) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) { $r = & $cond; if ($r) { return $r }; Start-Sleep -Milliseconds 500 }
    throw "timed out after $seconds s waiting for $what. On screen: $((Texts) -join ' | ')"
}
function Find($name) { $script:win.FindFirst($T::Descendants, (New-Object $P($A::NameProperty, $name))) }
function Texts { if ($script:win) { @($script:win.FindAll($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::Text))) | ForEach-Object { $_.Current.Name } | Where-Object { $_ }) } }
function Start-App {
    $script:proc = Start-Process $Exe -PassThru
    $script:win = Wait-Until { $A::RootElement.FindFirst($T::Children, (New-Object $P($A::ProcessIdProperty, [int]$script:proc.Id))) } 30 'main window'
}
function Stop-App { $script:proc.Kill(); $script:proc.WaitForExit(); $script:win = $null; Get-Process clipd -ErrorAction SilentlyContinue | Stop-Process -Force }

$dir = Join-Path $env:TEMP 'clipingo-e2e'
New-Item -ItemType Directory -Force $dir | Out-Null
$feed = Join-Path $dir 'release.json'
@{ tag_name = 'v9.9.0'; html_url = 'https://example.invalid/release'; body = 'Test release';
   assets = @(@{ name = 'Clipingo-Setup-9.9.0.exe'; browser_download_url = (Join-Path $dir 'not-run.exe'); digest = 'sha256:00' }) } |
    ConvertTo-Json -Depth 4 | Set-Content $feed -Encoding UTF8

# 1. Fake newer release: the bar appears on its own at start-up.
$env:CLIPINGO_UPDATE_FEED = $feed
Start-App
$null = Wait-Until { (Texts) -match 'Clipingo 9\.9\.0 is available' } 20 'update bar'
'startup: update bar shows 9.9.0'
(Wait-Until { Find 'Settings' } 10 'Settings').GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
(Wait-Until { $b = Find 'Check for updates'; if ($b -and $b.Current.IsEnabled) { $b } } 20 'Check now').GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
$null = Wait-Until { (Texts) -match 'Clipingo 9\.9\.0 is available' -and (Find 'Install update') } 20 'Update now button'
'settings: Check now finds 9.9.0 and offers Update now'
Stop-App

# 2. Real GitHub feed: a definite answer either way (never stuck on "Checking…").
Remove-Item Env:CLIPINGO_UPDATE_FEED
Start-App
(Wait-Until { Find 'Settings' } 10 'Settings').GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
(Wait-Until { $b = Find 'Check for updates'; if ($b -and $b.Current.IsEnabled) { $b } } 20 'Check now').GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
$answer = Wait-Until { (Texts) | Where-Object { $_ -match '^You have the latest|^Could not check|is available' } | Select-Object -First 1 } 30 'GitHub answer'
"github:  $answer"
Stop-App
