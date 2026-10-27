# M0 spike: ffmpeg build check for Clipingo.
# 1. libass + HarfBuzz render Devanagari conjuncts/matras (shaping=complex vs simple).
# 2. Hardware H.264/HEVC/AV1 encoders actually open on this machine (not just compiled in).
# Usage: pwsh spikes/m0/ffmpeg/check.ps1 [-Out <dir>]
param([string]$Out = (Join-Path $env:TEMP 'clipingo-m0-ffmpeg'))

$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
Copy-Item (Join-Path $here 'devanagari.ass') (Join-Path $Out 'devanagari.ass') -Force
Push-Location $Out   # relative paths keep Windows drive colons out of the filter graph
try {
    $conf = (ffmpeg -hide_banner -buildconf 2>&1) -join ' '
    foreach ($lib in 'libass', 'libharfbuzz', 'libfribidi') {
        "{0,-12} {1}" -f $lib, ($(if ($conf -match "enable-$lib") { 'yes' } else { 'NO' }))
    }

    foreach ($shaping in 'complex', 'simple') {
        ffmpeg -v error -y -f lavfi -i 'color=c=0x202030:s=1080x1920:d=1' `
            -vf "ass=devanagari.ass:fontsdir=C\\:/Windows/Fonts:shaping=$shaping" -frames:v 1 "devanagari-$shaping.png"
        "render shaping=$shaping -> $(Join-Path $Out "devanagari-$shaping.png")"
    }

    $ErrorActionPreference = 'Continue'   # a missing encoder is a result, not a script failure
    foreach ($enc in 'h264_nvenc', 'hevc_nvenc', 'av1_nvenc', 'h264_amf', 'h264_qsv', 'h264_mf', 'libx264') {
        $log = ffmpeg -v error -y -f lavfi -i 'testsrc2=s=1920x1080:r=30:d=2' -c:v $enc -f null - 2>&1
        "{0,-12} {1}" -f $enc, ($(if ($LASTEXITCODE -eq 0) { 'works' } else { 'unavailable: ' + (($log | Select-Object -First 1) -as [string]) }))
    }
}
finally { Pop-Location }
