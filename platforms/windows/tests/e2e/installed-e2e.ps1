# E2E on the shipped build: build the installer, install it silently into a temp folder, run the app and update
# suites against the installed Clipingo.exe, then uninstall.
# Usage: powershell -File platforms/windows/tests/e2e/installed-e2e.ps1 [-Source <video>] [-Language English] [-SkipBuild]
param(
    [string]$Source = (Join-Path $env:TEMP 'clipingo-e2e\english-stream.mp4'),
    [string]$Language = 'English',
    [switch]$SkipBuild
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path))))
$version = ([xml](Get-Content (Join-Path $root 'platforms\windows\Clipingo\Clipingo.csproj'))).Project.PropertyGroup.Version | Where-Object { $_ } | Select-Object -First 1
if (-not $SkipBuild) { & powershell -File (Join-Path $root 'platforms\windows\installer\build.ps1'); if ($LASTEXITCODE) { throw 'installer build failed' } }
$setup = Join-Path $root "target\installer\Clipingo-Setup-$version.exe"
$dir = Join-Path $env:TEMP 'clipingo-install-test'

function Stop-Clipingo { Get-Process Clipingo, clipd -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue }
Stop-Clipingo
if (Test-Path "$dir\unins000.exe") { & "$dir\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART | Out-Null; Start-Sleep 3 }
# Not Start-Process -Wait: it would also wait for the Clipingo the installer starts.
$p = Start-Process $setup -ArgumentList "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART /CURRENTUSER /NOICONS /DIR=`"$dir`"" -PassThru
$p.WaitForExit()
if ($p.ExitCode) { throw "setup exited with $($p.ExitCode)" }
"installed: $('{0:N1}' -f ((Get-Item $setup).Length / 1MB)) MB installer into $dir"
Start-Sleep 4
Stop-Clipingo

$exe = Join-Path $dir 'Clipingo.exe'
& powershell -File (Join-Path $PSScriptRoot 'app-e2e.ps1') -Source $Source -Language $Language -Exe $exe
$appExit = $LASTEXITCODE
Stop-Clipingo
& powershell -File (Join-Path $PSScriptRoot 'update-e2e.ps1') -Exe $exe
$updateExit = $LASTEXITCODE
Stop-Clipingo

& "$dir\unins000.exe" /VERYSILENT /SUPPRESSMSGBOXES /NORESTART | Out-Null
Start-Sleep 3
'uninstalled: ' + $(if (Test-Path $exe) { 'Clipingo.exe still there' } else { 'files removed' })
if ($appExit -or $updateExit) { throw "installed build failed: app e2e $appExit, update e2e $updateExit" }
'installed build: all checks passed'
