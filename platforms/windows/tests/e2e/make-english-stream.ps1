# E2E fixture: a ~2 minute 1080p "stream" with synthetic English speech (Windows TTS), long pauses
# (dead air to cut), and a webcam box, so the whole pipeline can be tested without real footage. english-speech.vtt
# next to it has the synthesizer's own word times: `clip eval <run-id> english-speech.vtt` measures caption sync.
# Usage: powershell -File platforms/windows/tests/e2e/make-english-stream.ps1 [-Out <file.mp4>]
param([string]$Out = (Join-Path $env:TEMP 'clipingo-e2e\english-stream.mp4'))
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$dir = Split-Path -Parent $Out
New-Item -ItemType Directory -Force $dir | Out-Null
$wav = Join-Path $dir 'english-speech.wav'

$lines = @(
    'Okay so we are setting up the project folder and moving some files around.',
    'And then we just kind of open the editor and wait for it to load.',
    '<break time="3s"/>',
    'Why does nobody tell you this about shipping software fast?',
    'I remember the first time I deployed on a Friday and lost a whole weekend.',
    'The lesson is simple. Never deploy what you have not tested, ever!',
    '<break time="2500ms"/>',
    'It was the worst mistake of my career, and honestly it was hilarious later.',
    'Anyway, let us go back to the build and check the logs.',
    'So the logs say the cache is warm and the tests are running.',
    '<break time="3s"/>',
    'Here is the secret nobody talks about: one flag made the build ten times faster.',
    'What if I told you that most people never turn it on?',
    'That one change saved us a million dollars in compute this year!',
    'Okay, moving on, the next part is boring configuration.',
    'We set the timeout, we set the region, and we save the file.'
)
$ssml = '<speak version="1.0" xmlns="http://www.w3.org/2001/10/synthesis" xml:lang="en-US">' + ($lines -join ' <break time="600ms"/> ') + '</speak>'
# Word onsets from the synthesizer itself: an exact reference for `clip eval` (english-speech.vtt, a cue per word).
# Its events come on another thread, so a small C# helper collects them.
Add-Type -ReferencedAssemblies System.Speech -TypeDefinition @'
using System.Collections.Generic; using System.Speech.Synthesis;
public static class WordTimes {
    public static List<string> Speak(SpeechSynthesizer s, string ssml) {
        var cues = new List<string>();
        s.SpeakProgress += (o, e) => { lock (cues) cues.Add(e.AudioPosition.TotalSeconds.ToString("0.000", System.Globalization.CultureInfo.InvariantCulture) + " " + e.Text); };
        s.SpeakSsml(ssml);
        return cues;
    }
}
'@
$s = New-Object System.Speech.Synthesis.SpeechSynthesizer
$s.Rate = 0
# 16 kHz like the voice: with other formats the reported word positions run 22050/16000 too fast.
$s.SetOutputToWaveFile($wav, (New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo 16000, ([System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen), ([System.Speech.AudioFormat.AudioChannel]::Mono)))
$times = [WordTimes]::Speak($s, $ssml)
$s.Dispose()
$stamp = { param($t) [TimeSpan]::FromSeconds($t).ToString('hh\:mm\:ss\.fff') }
$vtt = @('WEBVTT', '')
foreach ($c in $times) {
    $at, $word = $c.Split(' ', 2)
    $at = [double]::Parse($at, [Globalization.CultureInfo]::InvariantCulture)
    $vtt += "$(& $stamp $at) --> $(& $stamp ($at + 0.2))", $word, ''
}
Set-Content -Path ([IO.Path]::ChangeExtension($wav, '.vtt')) -Value $vtt -Encoding utf8

$font = 'C\:/Windows/Fonts/arialbd.ttf'
ffmpeg -v error -y -f lavfi -i 'testsrc2=s=1920x1080:r=30' -i $wav -filter_complex `
    "[0]drawbox=x=1440:y=700:w=440:h=340:color=orange@0.9:t=fill,drawtext=fontfile='$font':text='WEBCAM':fontsize=60:x=1530:y=840:fontcolor=black[v]" `
    -map '[v]' -map 1 -shortest -c:v libx264 -preset veryfast -pix_fmt yuv420p -c:a aac $Out
Get-Item $Out | Select-Object FullName, Length
