# E2E: drive the real WinUI app through UI Automation, Review & edit workflow:
#   Video (workflow, source, Load) → Format → Clips (language) → Captions → Review → Find candidates →
#   Jobs: export the moments → files on disk → Open folder in Explorer →
#   Editor: trim, retitle, fix a caption word → Export clip → files updated → Show in folder.
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
    throw "timed out after $seconds s waiting for $what. On screen: $((Texts) -join ' | ')"
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
$started = Get-Date

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
# Export all but the last, keeping at least two: per-clip work (face detection) must survive a second clip.
if ($boxes.Count -gt 2) { $boxes[-1].GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle() }

Invoke 'Render selected clips'
$null = Wait-Until { (Texts) -contains 'Done' } 1800 'exported clips'
"result:  " + ((Texts | Where-Object { $_ -match 'exported|dead air' }) -join ' || ')

# ── The files are really there, outside the app-data folder, and complete ──
$runDir = Get-ChildItem (Join-Path $env:LOCALAPPDATA 'Clipingo\runs') -Directory | Sort-Object Name -Descending | Select-Object -First 1
$run = Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $run.clips_dir -or $run.clips_dir -like "$env:LOCALAPPDATA*") { throw "clips must be exported outside AppData, got '$($run.clips_dir)'" }
$videos = @(Get-ChildItem $run.clips_dir -Filter *.mp4)
if ($videos.Count -lt 2) { throw "expected 2 exported videos in $($run.clips_dir), found $($videos.Count)" }
foreach ($v in $videos) {
    foreach ($ext in '.srt', '.xml') { if (-not (Test-Path ([IO.Path]::ChangeExtension($v.FullName, $ext)))) { throw "missing $ext for $($v.Name)" } }
    $d = [double](& ffprobe -v error -show_entries format=duration -of csv=p=0 $v.FullName)
    if ($d -lt 5) { throw "$($v.Name) is $d s long" }
    [xml](Get-Content ([IO.Path]::ChangeExtension($v.FullName, '.xml')) -Raw) | Out-Null   # Premiere XML must parse
}
if (@(Get-ChildItem $run.clips_dir -Filter *.ass).Count -gt 0) { throw 'leftover .ass files in the clips folder' }
"files:   $($videos.Count) videos + .srt + .xml in $($run.clips_dir)"

# Open folder must show the folder in File Explorer, not "Location is not available".
function Explorer-Shows($folder) {
    Wait-Until {
        $wins = @($A::RootElement.FindAll($T::Children, (New-Object $P($A::ClassNameProperty, 'CabinetWClass'))))
        foreach ($w in $wins) {
            $all = @($w.FindAll($T::Descendants, [System.Windows.Automation.Condition]::TrueCondition) | ForEach-Object { $_.Current.Name })
            if ($all -match 'is unavailable') { throw "Explorer: $($all -match 'is unavailable' | Select-Object -First 1)" }
            if ($all -contains (Split-Path $folder -Leaf) -or ($all -match [regex]::Escape((Split-Path $folder -Leaf)))) {
                $w.GetCurrentPattern([System.Windows.Automation.WindowPattern]::Pattern).Close(); return $true
            }
        }
    } 20 "File Explorer showing $folder"
}
Invoke 'Open output folder'
$null = Explorer-Shows $run.clips_dir
'open folder: Explorer shows the clips folder'

# ── Editor: trim, retitle, fix a caption word, export again ──
$rank = 1
$clip = $run.candidates | Where-Object rank -eq $rank
$editButton = Wait-Until { $script:win.FindFirst($T::Descendants, (New-Object $P($A::NameProperty, "Edit $($clip.title)"))) } 30 'Edit button'
$editButton.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
$titleBox = Wait-Until { $e = Find 'Clip title'; if ($e -and $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value -eq $clip.title) { $e } } 30 'editor with the clip'
$captionBox = Wait-Until { $e = Find 'Caption text'; if ($e -and $e.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value) { $e } } 30 'caption text'
$words = $captionBox.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).Current.Value -split ' '
$words[[int]($words.Count / 2)] = 'EDITED'   # mid-clip: the +1 s trim below must not cut it out
Invoke 'Start 1 second later'
$titleBox.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue('E2E edited title')
$captionBox.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern).SetValue($words -join ' ')
$before = (Get-Item $videos[0].FullName).LastWriteTime
Invoke 'Export clip'
$status = Wait-Until { Texts | Where-Object { $_ -like 'Exported ·*' } } 600 'editor export'
"editor:  $status"
$run2 = Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json
$c2 = $run2.candidates | Where-Object rank -eq $rank
if ($c2.title -ne 'E2E edited title') { throw "title not saved: $($c2.title)" }
if ([math]::Abs($c2.start - $clip.start - 1) -gt 0.05) { throw "trim not saved: $($clip.start) -> $($c2.start)" }
$v1 = Get-ChildItem $run.clips_dir -Filter ("clip_{0:00}_*.mp4" -f $rank) | Select-Object -First 1
if ($v1.LastWriteTime -le $before) { throw 'clip was not exported again' }
$srt = Get-Content ([IO.Path]::ChangeExtension($v1.FullName, '.srt')) -Raw -Encoding UTF8
if ($srt -notmatch 'EDITED') { throw "corrected caption missing from the SRT:`n$srt" }
'editor:  trim, title and caption fix saved; clip, SRT and XML exported again'
Invoke 'Show in folder'
$null = Explorer-Shows $run.clips_dir
'editor:  Show in folder opens Explorer'

# ── Editor options: undo trim, transcript seek, new clip, look (position, size, speed, fade), delete ──
function Text-Of($name) { $e = Find $name; if ($e) { $e.Current.Name } }
function Choose($combo, $item) {
    $c = Enabled $combo
    $c.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Expand(); Start-Sleep -Milliseconds 300
    $i = Wait-Until { $script:win.FindFirst($T::Descendants, (New-Object System.Windows.Automation.AndCondition((New-Object $P($A::NameProperty, $item)), (New-Object $P($A::ControlTypeProperty, $CT::ListItem))))) } 10 "$item in $combo"
    $i.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
    try { $c.GetCurrentPattern([System.Windows.Automation.ExpandCollapsePattern]::Pattern).Collapse() } catch {}
    Start-Sleep -Milliseconds 300
}
$startBefore = (Texts | Where-Object { $_ -match '^\d+:\d\d\.\d$' } | Select-Object -First 1)
Invoke 'Start 1 second later'
Invoke 'Undo trim'
$startAfter = (Texts | Where-Object { $_ -match '^\d+:\d\d\.\d$' } | Select-Object -First 1)
if ($startAfter -ne $startBefore) { throw "undo trim: start $startBefore became $startAfter" }
Invoke 'Redo trim'; Invoke 'Undo trim'
'editor:  undo / redo trim'

$link = $script:win.FindFirst($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::Hyperlink)))
if (-not $link) { throw 'transcript has no clickable words' }
$all = @($script:win.FindAll($T::Descendants, (New-Object $P($A::ControlTypeProperty, $CT::Hyperlink))))
$all[[int]($all.Count / 2)].GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke(); Start-Sleep -Milliseconds 400
"editor:  transcript has $($all.Count) clickable words"

$count = @((Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json).candidates).Count
Invoke 'New clip at playhead'
$added = Wait-Until { $r = Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json; $n = @($r.candidates | Where-Object { $_.reasons -contains 'Added by you' }); if ($n.Count -eq 1) { $n[0] } } 15 'new clip saved'
if (@((Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json).candidates).Count -ne $count + 1) { throw 'new clip count' }
"editor:  new clip #$($added.rank) at $([math]::Round($added.start,1))-$([math]::Round($added.end,1)) s"

Choose 'Caption position' 'Top'
(Enabled 'Caption size').GetCurrentPattern([System.Windows.Automation.RangeValuePattern]::Pattern).SetValue(140)
Choose 'Export speed' '1.25×'
(Enabled 'Fade in and out').GetCurrentPattern([System.Windows.Automation.TogglePattern]::Pattern).Toggle()
Invoke 'Export clip'
$status2 = Wait-Until { Texts | Where-Object { $_ -like 'Exported ·*' } } 600 'export of the new clip'
$opts = Get-Content (Join-Path $runDir.FullName 'render.json') -Raw | ConvertFrom-Json
if ($opts.caption_position -ne 'top' -or [math]::Abs($opts.caption_size - 1.4) -gt 0.01 -or $opts.speed -ne 1.25 -or -not $opts.fade) { throw "export options not applied: $($opts | ConvertTo-Json -Compress)" }
$newVideo = Get-ChildItem $run.clips_dir -Filter ("clip_{0:00}_*.mp4" -f $added.rank) | Select-Object -First 1
if (-not $newVideo) { throw 'new clip was not exported' }
$len = [double](& ffprobe -v error -show_entries format=duration -of csv=p=0 $newVideo.FullName)
# Frame brightness (metadata=print writes it to a file; PowerShell 5.1 turns ffmpeg's stderr into errors).
$luma = { param($t) Push-Location $env:TEMP; try { & ffmpeg -hide_banner -v error -y -ss $t -i $newVideo.FullName -frames:v 1 -vf 'signalstats,metadata=print:key=lavfi.signalstats.YAVG:file=clipingo-luma.txt' -f null - ; [double]((Get-Content clipingo-luma.txt | Select-String 'YAVG=' | Select-Object -First 1).Line -replace '.*YAVG=', '') } finally { Pop-Location } }
$first, $mid = (& $luma 0), (& $luma ($len / 2))
if ($first -ge $mid) { throw "fade in missing: first frame luma $first vs middle $mid" }
"editor:  new clip exported ($([math]::Round($len,1)) s at 1.25x, fades, top captions at 140%): $status2"
$frameOut = Join-Path $env:TEMP 'clipingo-e2e\editor-frame.png'
& ffmpeg -v error -y -ss ($len / 2) -i $newVideo.FullName -frames:v 1 -vf scale=270:-2 $frameOut

Invoke 'Delete clip'
$dlg = Wait-Until { $script:win.FindFirst($T::Descendants, (New-Object System.Windows.Automation.AndCondition((New-Object $P($A::NameProperty, 'Delete')), (New-Object $P($A::ControlTypeProperty, $CT::Button))))) } 10 'delete confirmation'
$dlg.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
$null = Wait-Until { -not @((Get-Content (Join-Path $runDir.FullName 'run.json') -Raw -Encoding UTF8 | ConvertFrom-Json).candidates | Where-Object rank -eq $added.rank) } 15 'clip removed'
'editor:  delete clip (after confirmation)'

$log = Join-Path $env:LOCALAPPDATA 'Clipingo\logs\app.log'
if ((Test-Path $log) -and (Get-Item $log).LastWriteTime -gt $started) { throw "unhandled app errors:`n$(Get-Content $log -Tail 30 | Out-String)" }
"total time: $([int]$sw.Elapsed.TotalSeconds) s"
