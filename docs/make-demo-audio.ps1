# Creates a short synthetic meeting (Windows text-to-speech, three voices) as 16 kHz mono WAVs for README
# screenshots and end-to-end tests:  transcribe.exe --feed <dir>\00.wav <dir>\01.wav ...
# No real voices or call content are involved.
param([string]$OutDir = (Join-Path $env:TEMP "transcribe-demo"))
$ErrorActionPreference = "Stop"
New-Item -ItemType Directory $OutDir -Force | Out-Null
Add-Type -AssemblyName System.Speech
$fmt = New-Object System.Speech.AudioFormat.SpeechAudioFormatInfo(16000,
    [System.Speech.AudioFormat.AudioBitsPerSample]::Sixteen, [System.Speech.AudioFormat.AudioChannel]::Mono)
$installed = (New-Object System.Speech.Synthesis.SpeechSynthesizer).GetInstalledVoices() |
    Where-Object Enabled | ForEach-Object { $_.VoiceInfo.Name }
function Voice($want) { if ($installed -contains $want) { $want } else { $installed | Select-Object -First 1 } }

$script = @(
    @("Microsoft David Desktop", "Good morning everyone. Let's go through the release plan for the new reporting module."),
    @("Microsoft Zira Desktop",  "Thanks. The data import is finished, and the first test run on Friday looked really good."),
    @("Microsoft Mark",          "Great news. What is the difference between a rolling deployment and a blue green deployment?"),
    @("Microsoft David Desktop", "Good question. Can you send the test report to the whole team by Wednesday?"),
    @("Microsoft Zira Desktop",  "Sure. I will send it on Wednesday and add the open issues from the import."),
    @("Microsoft Mark",          "Perfect. Then we plan the go-live for the second week of December.")
)
$i = 0
foreach ($line in $script) {
    $t = New-Object System.Speech.Synthesis.SpeechSynthesizer
    $t.SelectVoice((Voice $line[0]))
    $file = Join-Path $OutDir ("{0:D2}.wav" -f $i)
    $t.SetOutputToWaveFile($file, $fmt)
    $t.Speak($line[1])
    $t.Dispose()
    Write-Host "$file  ($($line[0]))"
    $i++
}
