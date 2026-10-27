# M0 spike: can Antigravity CLI (agy) do Clipingo's visual checks headless on Windows?
# Writes a contact sheet with content no model can guess (random code + tile colours) into a job
# workspace, asks `agy -p` about it with stdout captured (piped), and checks:
#   1. stdout is not empty when piped (reported Windows bug),
#   2. the JSON envelope / --json-schema structured output arrives,
#   3. the answer matches the image, i.e. agy really read it.
# Usage: pwsh spikes/m0/agy/check.ps1 [-Model <slug>]
param([string]$Model = '', [string]$Out = (Join-Path $env:TEMP 'clipingo-m0-agy'))

$ErrorActionPreference = 'Continue'
New-Item -ItemType Directory -Force $Out | Out-Null
Push-Location $Out
try {
    $code = -join ((65..90) + (48..57) | Get-Random -Count 6 | ForEach-Object { [char]$_ })
    $font = 'C\:/Windows/Fonts/arialbd.ttf'
    # 2x2 sheet: red, green, blue, yellow tiles; the code is printed on the yellow tile.
    ffmpeg -v error -y -f lavfi -i 'color=c=red:s=480x270' -f lavfi -i 'color=c=green:s=480x270' `
        -f lavfi -i 'color=c=blue:s=480x270' -f lavfi -i 'color=c=yellow:s=480x270' -filter_complex `
        "[3]drawtext=fontfile='$font':text='$code':fontsize=90:fontcolor=black:x=(w-tw)/2:y=(h-th)/2[y];[0][1][2][y]xstack=inputs=4:layout=0_0|w0_0|0_h0|w0_h0" `
        -frames:v 1 sheet.jpg

    $schema = '{"type":"object","properties":{"code":{"type":"string"},"top_left":{"type":"string"},"top_right":{"type":"string"},"bottom_left":{"type":"string"},"bottom_right":{"type":"string"}},"required":["code","top_left","top_right","bottom_left","bottom_right"]}'
    # Schema goes in a file: Windows PowerShell 5.1 strips quotes from JSON passed as a native argument.
    Set-Content schema.json $schema -Encoding ascii
    $prompt = "Use your file viewing tool (not a shell command) to open the image $(Join-Path $Out 'sheet.jpg') and look at it. It is a 2x2 grid of coloured tiles. " +
              'Report the colour name of each tile and the exact text code printed on one tile. Do not guess; if you cannot view the image, set code to CANNOT_VIEW.'
    $agyArgs = @('-p', $prompt, '--output-format', 'json', '--json-schema', (Join-Path $Out 'schema.json'), '--add-dir', $Out, '--print-timeout', '4m')
    if ($Model) { $agyArgs += @('--model', $Model) }

    $sw = [Diagnostics.Stopwatch]::StartNew()
    $stdout = & agy @agyArgs 2> agy-stderr.txt   # captured = piped stdout
    $exit = $LASTEXITCODE
    $sw.Stop()
    $raw = ($stdout | Out-String)
    Set-Content agy-stdout.json $raw -Encoding utf8

    "expected code     : $code (tiles red, green, blue, yellow)"
    "exit code         : $exit   time: $([int]$sw.Elapsed.TotalSeconds) s"
    "stdout bytes      : $($raw.Length)"
    if ($raw.Trim()) {
        $env = $raw | ConvertFrom-Json
        "status            : $($env.status)"
        "structured_output : $($env.structured_output | ConvertTo-Json -Compress)"
        "response          : $($env.response)"
        $got = "$($env.structured_output.code)$($env.response)"
        "image actually read: $($got -match [regex]::Escape($code))"
    } else {
        "stdout EMPTY when piped -> reproduces the Windows bug"
    }
    "stderr (last lines):"; Get-Content agy-stderr.txt -Tail 5
}
finally { Pop-Location }
