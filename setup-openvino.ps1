# One-time build-machine setup for the GPU backend: downloads the official OpenVINO GenAI runtime
# (same version as the Python version used) into %LOCALAPPDATA%\transcribe-build and checks its SHA-256.
# package.ps1 copies the needed DLLs from there into dist\transcribe\openvino.
$ErrorActionPreference = "Stop"
$version = "2026.4.1"
$name = "openvino_genai_windows_${version}.0_x86_64"
$url = "https://storage.openvinotoolkit.org/repositories/openvino_genai/packages/$version/windows/$name.zip"
$dir = Join-Path $env:LOCALAPPDATA "transcribe-build"
$zip = Join-Path $dir "genai.zip"
New-Item -ItemType Directory -Force $dir | Out-Null
if (Test-Path (Join-Path $dir "$name\runtime\bin\intel64\Release\openvino_genai_c.dll")) {
    Write-Host "OpenVINO GenAI $version already set up in $dir"
    exit 0
}
Write-Host "Downloading $url ..."
Invoke-WebRequest $url -OutFile $zip
$expected = ((Invoke-WebRequest "$url.sha256").Content -split '\s+')[0].Trim().ToLower()
$actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
if ($expected -ne $actual) { throw "SHA-256 mismatch: expected $expected, got $actual" }
Expand-Archive $zip -DestinationPath $dir -Force
Remove-Item $zip
Write-Host "OpenVINO GenAI $version ready in $dir\$name"
