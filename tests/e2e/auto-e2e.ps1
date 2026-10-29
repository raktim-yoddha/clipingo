# E2E: Automatic workflow, horizontal format, Library listing and removal.
#   Create (Automatic, Horizontal, 1 clip) → Generate clips → Jobs: Done → 16:9 files on disk → Library shows the run →
#   remove it (the run folder goes, the exported clip stays).
# Usage: powershell -File tests/e2e/auto-e2e.ps1 -Source <file> [-Exe <Clipingo.exe>]
param(
    [Parameter(Mandatory)][string]$Source,
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'
if (-not $Exe) {
    $root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))
    $Exe = Get-ChildItem (Join-Path $root 'app\Clipingo\bin') -Recurse -Filter Clipingo.exe | Select-Object -First 1 -ExpandProperty FullName
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
function Texts { @($script:win.FindAll($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::Text))) | ForEach-Object { $_.Current.Name } | Where-Object { $_ }) }
function Enabled($name) { Wait-Until { $e = Find $name; if ($e -and $e.Current.IsEnabled) { $e } } 60 $name }
function Invoke($name) { (Enabled $name).GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); Start-Sleep -Milliseconds 400 }
function Pick($name) {
    $t = (Enabled $name).GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern)
    if ($t.Current.ToggleState -ne 'On') { $t.Toggle() }
}
function Select-Nav($name) { (Enabled $name).GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select(); Start-Sleep 1 }

$proc = Start-Process $Exe -PassThru
$script:win = Wait-Until { $A::RootElement.FindFirst($T::Children, (New-Object $P($A::ProcessIdProperty, [int]$proc.Id))) } 30 'main window'
$started = Get-Date
$before = @(Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Clipingo\runs') -Directory).Count

Pick 'Automatic'
(Enabled 'Video link or file path').GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($Source)
Invoke 'Load video'
Invoke 'Next: Format'
Pick 'Horizontal'
Invoke 'Next: Clips'
Pick 'English'
(Enabled 'Let Clipingo decide how many clips').GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern) | ForEach-Object { if ($_.Current.ToggleState -eq 'On') { $_.Toggle() } }
(Enabled 'Maximum clips').GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern).SetValue(1)
Invoke 'Next: Captions'
Invoke 'Next: Review'
Invoke 'Generate clips'
Invoke 'View job'
$null = Wait-Until { (Texts) -contains 'Done' } 1800 'automatic job'
"jobs:    " + ((Texts | Where-Object { $_ -match 'exported|dead air' }) -join ' || ')

$runDir = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Clipingo\runs') -Directory | Sort-Object Name -Descending | Select-Object -First 1
if (@(Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Clipingo\runs') -Directory).Count -ne $before + 1) { throw 'no new run' }
$run = Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$videos = @(Get-ChildItem $run.clips_dir -Filter *_16x9.mp4)
if ($videos.Count -ne 1) { throw "expected 1 horizontal clip, found $($videos.Count) in $($run.clips_dir)" }
$wh = (& ffprobe -v error -select_streams v:0 -show_entries stream=width,height -of csv=p=0 $videos[0].FullName)
if ($wh -ne '1920,1080') { throw "horizontal clip is $wh" }
"files:   $($videos[0].Name) $wh"

Select-Nav 'Library'
$null = Wait-Until { Find "Edit clips of $($run.source.title)" } 20 'run in Library'
'library: run listed'
# Remove it again (exported clips stay).
Invoke "Delete $($run.source.title)"
$dlg = Wait-Until { $script:win.FindFirst($T::Descendants, (New-Object System.Windows.Automation.AndCondition((New-Object $P($A::NameProperty, 'Remove')), (New-Object $P($A::ControlTypeProperty, $CT::Button))))) } 10 'remove confirmation'
$dlg.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
$null = Wait-Until { -not (Test-Path $runDir.FullName) } 15 'run folder removed'
if (-not (Test-Path $videos[0].FullName)) { throw 'exported clip was deleted with the run' }
'library: run removed, exported clip kept'

$log = Join-Path $env:LOCALAPPDATA 'Clipingo\logs\app.log'
if ((Test-Path $log) -and (Get-Item $log).LastWriteTime -gt $started) { throw "unhandled app errors:`n$(Get-Content $log -Tail 30 | Out-String)" }
$proc.CloseMainWindow() | Out-Null
'auto e2e: ok'
