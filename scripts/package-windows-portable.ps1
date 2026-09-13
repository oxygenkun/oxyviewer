$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
Set-StrictMode -Version Latest

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$version = (Get-Content -Raw (Join-Path $repositoryRoot "package.json") | ConvertFrom-Json).version
$portableName = "OxyViewer_${version}_Windows_x64_portable"
$portableRoot = Join-Path $env:RUNNER_TEMP $portableName
$verificationRoot = Join-Path $env:RUNNER_TEMP "$portableName-verify"
$bundleDirectory = Join-Path $repositoryRoot "target/release/bundle/portable"
$archivePath = Join-Path $bundleDirectory "$portableName.zip"

foreach ($path in @($portableRoot, $verificationRoot, $archivePath)) {
  if (Test-Path -LiteralPath $path) {
    Remove-Item -LiteralPath $path -Recurse -Force
  }
}

$oxyViewerLicenses = Join-Path $portableRoot "licenses/OxyViewer"
New-Item -ItemType Directory -Path @($portableRoot, $oxyViewerLicenses, $bundleDirectory) -Force | Out-Null

$files = @{
  "target/release/oxyviewer.exe" = "OxyViewer.exe"
  "apps/desktop/src-tauri/binaries/oxy-ffmpeg-x86_64-pc-windows-msvc.exe" = "oxy-ffmpeg.exe"
  "apps/desktop/src-tauri/binaries/oxy-ffprobe-x86_64-pc-windows-msvc.exe" = "oxy-ffprobe.exe"
  "LICENSE.md" = "licenses/OxyViewer/LICENSE.md"
  "LICENSE-AGPL-3.0" = "licenses/OxyViewer/LICENSE-AGPL-3.0"
  "LICENSE-COMMERCIAL.md" = "licenses/OxyViewer/LICENSE-COMMERCIAL.md"
  "THIRD_PARTY_NOTICES.md" = "licenses/OxyViewer/THIRD_PARTY_NOTICES.md"
}

foreach ($entry in $files.GetEnumerator()) {
  $source = Join-Path $repositoryRoot $entry.Key
  if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
    throw "Required portable file is missing: $source"
  }
  Copy-Item -LiteralPath $source -Destination (Join-Path $portableRoot $entry.Value)
}

$ffmpegResources = Join-Path $repositoryRoot "apps/desktop/src-tauri/resources/ffmpeg"
if (-not (Test-Path -LiteralPath $ffmpegResources -PathType Container)) {
  throw "FFmpeg resource directory is missing: $ffmpegResources"
}
Copy-Item -LiteralPath $ffmpegResources -Destination (Join-Path $portableRoot "licenses") -Recurse

& node (Join-Path $repositoryRoot "3rdpart/ffmpeg/prepare.mjs") --verify-bundle $portableRoot
Compress-Archive -LiteralPath $portableRoot -DestinationPath $archivePath -CompressionLevel Optimal

Expand-Archive -LiteralPath $archivePath -DestinationPath $verificationRoot
$expandedPortableRoot = Join-Path $verificationRoot $portableName
& node (Join-Path $repositoryRoot "3rdpart/ffmpeg/prepare.mjs") --verify-bundle $expandedPortableRoot

Write-Output "Created and verified $archivePath"
