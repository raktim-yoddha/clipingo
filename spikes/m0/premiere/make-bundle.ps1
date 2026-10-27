# M0 spike: build the Premiere test bundle (media + FCP 7 XML + SRT) next to the test scripts.
#   test.mp4        20 s, 1920x1080 30 fps, burned-in timecode, a "webcam" box bottom-right, tone
#   test.srt        Hindi / Hinglish / English cues for the caption-track test
#   clipingo-m0.xml xmeml v5: 1080x1920 sequence, V1 screen crop + V2 webcam crop with Motion
#                   keyframes, two cuts, markers. Effect ids are our best reading of FCP 7 XML;
#                   which ones Premiere honours is exactly what this test finds out.
# Usage: pwsh spikes/m0/premiere/make-bundle.ps1
$ErrorActionPreference = 'Stop'
$dir = Join-Path (Split-Path -Parent $MyInvocation.MyCommand.Path) 'bundle'
New-Item -ItemType Directory -Force $dir | Out-Null
Push-Location $dir
try {
    $font = 'C\:/Windows/Fonts/arialbd.ttf'
    ffmpeg -v error -y -f lavfi -i 'testsrc2=s=1920x1080:r=30:d=20' -f lavfi -i 'sine=f=440:d=20' -filter_complex `
        "[0]drawbox=x=1440:y=700:w=440:h=340:color=orange@0.9:t=fill,drawtext=fontfile='$font':text='WEBCAM':fontsize=60:x=1530:y=840:fontcolor=black,drawtext=fontfile='$font':timecode='00\:00\:00\:00':rate=30:fontsize=72:x=60:y=60:fontcolor=white:box=1:boxcolor=black@0.6[v]" `
        -map '[v]' -map 1 -c:v libx264 -pix_fmt yuv420p -c:a aac -shortest test.mp4

    $srt = @"
1
00:00:00,500 --> 00:00:02,500
क्या आप तैयार हैं?

2
00:00:03,000 --> 00:00:05,000
bhai kya scene hai

3
00:00:05,500 --> 00:00:08,000
This caption came from Clipingo.
"@
    [IO.File]::WriteAllText((Join-Path $dir 'test.srt'), $srt, (New-Object Text.UTF8Encoding $false))

    $url = 'file://localhost/' + ((Join-Path $dir 'test.mp4') -replace '\\', '/' -replace ' ', '%20')
    $rate = '<rate><timebase>30</timebase><ntsc>FALSE</ntsc></rate>'
    $file = "<file id=`"file-1`"><name>test.mp4</name><pathurl>$url</pathurl>$rate<duration>600</duration><media><video><samplecharacteristics><width>1920</width><height>1080</height></samplecharacteristics></video><audio><channelcount>1</channelcount></audio></media></file>"
    $fileRef = '<file id="file-1"/>'
    # Basic Motion: scale in %, centre in FCP 7 normalised units; Crop: percent per edge.
    function Motion($scale, $keys) {
        $kf = ($keys | ForEach-Object { "<keyframe><when>$($_[0])</when><value><horiz>$($_[1])</horiz><vert>$($_[2])</vert></value></keyframe>" }) -join ''
        "<filter><effect><name>Basic Motion</name><effectid>basic</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype><parameter><parameterid>scale</parameterid><name>Scale</name><valuemin>0</valuemin><valuemax>1000</valuemax><value>$scale</value></parameter><parameter><parameterid>center</parameterid><name>Center</name>$kf</parameter></effect></filter>"
    }
    function Crop($l, $r, $t, $b) {
        "<filter><effect><name>Crop</name><effectid>crop</effectid><effectcategory>motion</effectcategory><effecttype>motion</effecttype><mediatype>video</mediatype><parameter><parameterid>left</parameterid><name>left</name><value>$l</value></parameter><parameter><parameterid>right</parameterid><name>right</name><value>$r</value></parameter><parameter><parameterid>top</parameterid><name>top</name><value>$t</value></parameter><parameter><parameterid>bottom</parameterid><name>bottom</name><value>$b</value></parameter></effect></filter>"
    }
    # Two kept pieces of the source (a jump cut): source 60-210 and 300-450 frames.
    $pieces = @(@{ s = 0; e = 150; i = 60; o = 210 }, @{ s = 150; e = 300; i = 300; o = 450 })
    $n = 0
    function Clip($p, $fileXml, $filters) {
        $script:n++
        "<clipitem id=`"clip-$script:n`"><name>test.mp4</name><duration>600</duration>$rate<start>$($p.s)</start><end>$($p.e)</end><in>$($p.i)</in><out>$($p.o)</out>$fileXml$filters</clipitem>"
    }
    $screen = ($pieces | ForEach-Object -Begin { $first = $true } -Process { Clip $_ ($(if ($first) { $file } else { $fileRef })) ((Crop 0 25 0 0) + (Motion 100 @(,@(0, 0, 0.2)))); $first = $false }) -join ''
    $cam = ($pieces | ForEach-Object { Clip $_ $fileRef ((Crop 75 0 65 3) + (Motion 250 @(@(0, -0.9, -0.55), @(150, -0.8, -0.55)))) }) -join ''
    $audio = ($pieces | ForEach-Object { Clip $_ $fileRef '' }) -join ''
    $xml = @"
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE xmeml>
<xmeml version="5">
  <sequence id="sequence-1">
    <name>Clipingo M0 Short</name>
    <duration>300</duration>
    $rate
    <media>
      <video>
        <format><samplecharacteristics>$rate<width>1080</width><height>1920</height><pixelaspectratio>square</pixelaspectratio><fielddominance>none</fielddominance></samplecharacteristics></format>
        <track>$screen</track>
        <track>$cam</track>
      </video>
      <audio><track>$audio</track></audio>
    </media>
    <marker><name>Hype peak</name><comment>chat spike x4.2</comment><in>45</in><out>-1</out></marker>
    <marker><name>Hook title</name><comment>Kya scene hai</comment><in>0</in><out>-1</out></marker>
  </sequence>
</xmeml>
"@
    [IO.File]::WriteAllText((Join-Path $dir 'clipingo-m0.xml'), $xml, (New-Object Text.UTF8Encoding $false))
    ([xml]$xml) | Out-Null   # well-formed check
    Get-ChildItem $dir | Select-Object Name, Length
}
finally { Pop-Location }
