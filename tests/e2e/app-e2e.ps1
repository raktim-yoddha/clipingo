# E2E: drive the real WinUI app through UI Automation:
#   paste a source → Load → pick language → Find clips → untick the last candidate → Render selected.
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
# Waits for the element: popups (ComboBox) briefly hide the rest of the tree while they close.
function Invoke($name) { (Wait-Until { Find $name } 15 $name).GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke() }

$proc = Start-Process $Exe -PassThru
$script:win = Wait-Until { $A::RootElement.FindFirst($T::Children, (New-Object $P($A::ProcessIdProperty, [int]$proc.Id))) } 30 'main window'
$sw = [Diagnostics.Stopwatch]::StartNew()

(Wait-Until { Find 'Video link or file path' } 15 'source box').GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Source)
Invoke 'Load video'
$null = Wait-Until { $f = Find 'Find clips'; if ($f -and $f.Current.IsEnabled) { $f } } 60 'video card'
"loaded:  " + ((Texts) -join ' | ').Substring(0, [Math]::Min(200, ((Texts) -join ' | ').Length))

$combo = Find 'Spoken language'
$combo.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand()
(Wait-Until { Find $Language } 10 "language $Language").GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()

Invoke 'Find clips'
$null = Wait-Until { (Texts) -contains 'Moments' } 1800 'candidates'
$texts = Texts
"found:   " + ($texts | Where-Object { $_ -match '^\d+ moments' })
$boxes = @($win.FindAll($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::CheckBox))))
"candidates: " + (($boxes | ForEach-Object { $_.Current.Name }) -join ' || ')
if ($boxes.Count -gt 1) { $boxes[-1].GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle() }   # render all but the last

Invoke 'Render selected clips'
$null = Wait-Until { (Texts) -contains 'Rendered' } 1800 'rendered clips'
$after = Texts
$i = [array]::IndexOf($after, 'Rendered')
"rendered: " + (($after[($i + 2)..($after.Count - 1)]) -join ' || ')
"total time: $([int]$sw.Elapsed.TotalSeconds) s"
