# Builds the release and puts everything needed to run it into dist\transcribe:
#   transcribe.exe              the app (whisper.cpp and the Rust/C runtime are linked in)
#   sherpa-onnx-c-api.dll       voice recognition (sherpa-onnx) ...
#   onnxruntime.dll             ... and the ONNX runtime it uses
#   openvino\                   GPU speech recognition: OpenVINO GenAI runtime + the VC++ runtime it needs
#                               (put there by build.rs; without it the app runs on the slower CPU backend)
# Copy the folder anywhere. config.toml, speakers.json, transcripts\ and logs\ are created next to the exe;
# models are downloaded on first start to %LOCALAPPDATA%\transcribe-models.
# Run setup-openvino.ps1 once on a new build machine first.
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot
cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
if (-not (Test-Path "target\release\openvino\openvino_genai_c.dll")) {
    throw "target\release\openvino is missing - run setup-openvino.ps1, then package again"
}

$out = Join-Path $PSScriptRoot "dist\transcribe"
if (Test-Path $out) { Remove-Item $out -Recurse -Force }
New-Item -ItemType Directory -Force $out | Out-Null
foreach ($f in "transcribe.exe", "sherpa-onnx-c-api.dll", "onnxruntime.dll") {
    Copy-Item (Join-Path "target\release" $f) $out
}
Copy-Item "target\release\openvino" $out -Recurse
Copy-Item "README.md" $out -ErrorAction SilentlyContinue

Write-Host "Packaged to $out"
Get-ChildItem $out -Recurse -File | ForEach-Object { "{0,12:N0}  {1}" -f $_.Length, $_.FullName.Substring($out.Length + 1) }
