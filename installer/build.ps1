# Builds the Windows installer: target\installer\Clipingo-Setup-<version>.exe
#   Rust daemon (release) + trimmed self-contained WinUI app → Inno Setup (fetched by dotnet restore).
# Usage: powershell -File installer/build.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Set-Location $root
$version = ([xml](Get-Content app\Clipingo\Clipingo.csproj)).Project.PropertyGroup.Version | Where-Object { $_ } | Select-Object -First 1
$stage = Join-Path $root 'target\installer\app'
Remove-Item $stage -Recurse -Force -ErrorAction SilentlyContinue

cargo build --release -p clipd
if ($LASTEXITCODE) { throw 'cargo build failed' }
dotnet publish app\Clipingo\Clipingo.csproj -c Release -p:Platform=x64 -r win-x64 -p:DebugType=none -o $stage -nologo -v q
if ($LASTEXITCODE) { throw 'dotnet publish failed' }
Copy-Item target\release\clipd.exe $stage
# Debugging aids only: no symbols are shipped.
Get-ChildItem $stage -Recurse -Include *.pdb, Microsoft.DiaSymReader.Native.*.dll | Remove-Item -Force

dotnet restore installer\tools.proj -nologo -v q
if ($LASTEXITCODE) { throw 'could not fetch Inno Setup' }
$iscc = Get-ChildItem (Join-Path $env:USERPROFILE '.nuget\packages\tools.innosetup') -Recurse -Filter ISCC.exe | Select-Object -First 1 -ExpandProperty FullName
& $iscc /Q "/DAppVersion=$version" "/DSource=$stage" installer\Clipingo.iss
if ($LASTEXITCODE) { throw 'Inno Setup failed' }

$app = Get-ChildItem $stage -Recurse -File | Measure-Object Length -Sum
$setup = Get-Item "target\installer\Clipingo-Setup-$version.exe"
'app:       {0:N1} MB in {1} files' -f ($app.Sum / 1MB), $app.Count
'installer: {0:N1} MB  {1}' -f ($setup.Length / 1MB), $setup.FullName
