# E2E: drive the real WinUI app through UI Automation, Review & edit workflow:
#   Video (workflow, source, Load) → Format → Clips (language) → Captions → Review → Find candidates →
#   Jobs: untick the last candidate → Export selected → Done.
# Usage: powershell -File tests/e2e/app-e2e.ps1 -Source <file-or-youtube-url> [-Language English] [-Exe <Clipingo.exe>]
param(
    [Parameter(Mandatory)][string]$Source,
    [string]$Language = 'Auto',
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'
if (-not $Exe) {   # Windows PowerShell 5.1 has no $PSScriptRoot in param defaults
    $root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
    $Exe = Get-ChildItem (Join-Path $root 'app\Clipingo\bin') -Recurse -Filter Clipingo.exe | Select-Object -First 1 -ExpandProperty FullName
}
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$A = [System.Windows.Automation.AutomationElement]; $T = [System.Windows.Automation.TreeScope]
$P = [System.Windows.Automation.PropertyCondition]; $CT = [System.Windows.Automation.ControlType]

function Wait-Until([scriptblock]$cond, [int]$seconds, [string]$what) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) { $r = & $cond; if ($r) { return $r }; Start-Sleep -Milliseconds 500 }
    throw "timed out after $seconds s waiting for $what"
}
function Find($name) { $script:win.FindFirst($T::Descendants, (New-Object $P($A::NameProperty, $name))) }
function Texts { @($script:win.FindAll($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::Text))) | ForEach-Object { $_.Current.Name } | Where-Object { $_ }) }
function Enabled($name) { Wait-Until { $e = Find $name; if ($e -and $e.Current.IsEnabled) { $e } } 60 $name }
function Invoke($name) { (Enabled $name).GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); Start-Sleep -Milliseconds 400 }
# Tiles are toggle buttons; selecting one that is already on is a no-op.
function Pick($name) {
    $t = (Enabled $name).GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
    if ($t.Current.ToggleState -ne 'On') { $t.Toggle() }
}

$proc = Start-Process $Exe -PassThru
$script:win = Wait-Until { $A::RootElement.FindFirst($T::Children, (New-Object $P($A::ProcessIdProperty, [int]$proc.Id))) } 30 'main window'
$sw = [Diagnostics.Stopwatch]::StartNew()

Pick 'Review & edit'
(Enabled 'Video link or file path').GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Source)
Invoke 'Load video'
Invoke 'Next: Format'
"format:  " + (@('Vertical', 'Smart', 'Cut dead air and filler words', '1×') | ForEach-Object { if (Find $_) { $_ } }) -join ', '
Invoke 'Next: Clips'
Pick $Language
Invoke 'Next: Captions'
"captions: " + (@('Pop', 'Glow', 'Paper') | ForEach-Object { if (Find "Caption style $_") { $_ } }) -join ', '
Invoke 'Next: Review'
"review:  " + ((Texts | Where-Object { $_ -match '^(Workflow|Format|Language|Clips|Captions)$' }) -join ', ')
Invoke 'Find candidates'
Invoke 'View job'

$null = Wait-Until { (Texts) -contains 'Pick your clips' } 1800 'candidates'
$boxes = @($win.FindAll($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::CheckBox))))
"candidates: " + (($boxes | ForEach-Object { $_.Current.Name }) -join ' || ')
if ($boxes.Count -gt 1) { $boxes[-1].GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle() }   # export all but the last

Invoke 'Render selected clips'
$null = Wait-Until { (Texts) -contains 'Done' } 1800 'exported clips'
"result:  " + ((Texts | Where-Object { $_ -match 'exported|dead air' }) -join ' || ')
"total time: $([int]$sw.Elapsed.TotalSeconds) s"
