# chocolateyinstall.ps1 — install HeliosLite from the GitHub release.
#
# The release ships one self-contained executable per architecture, so this
# downloads the matching asset, verifies it against the SHA-256 published
# alongside the release, and creates shims for the command names.
#
# This file previously contained a C# class stub that only printed messages and
# returned 0, so the package installed nothing while reporting success.

$ErrorActionPreference = 'Stop'

$toolsDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$version = '2.13.21-h.0.2.7'
$baseUrl = "https://github.com/KooshaPari/HeliosLite/releases/download/v$version"

# Per-architecture assets. Chocolatey publishes ARM64 separately; every other
# host gets the x64 build, which runs under emulation on ARM64 Windows anyway.
$targets = @{
  'arm64' = @{
    Asset    = 'forge-aarch64-pc-windows-msvc.exe'
    Checksum = '4eaf1d279ede97af72a9d7109f366586fbcca4e7a08d53f8f784c46088fe5b3f'
  }
  'x64'   = @{
    Asset    = 'forge-x86_64-pc-windows-msvc.exe'
    Checksum = 'fee73f9631a31a675dd6a763f65079d72228ecf21aa868e615bb10e5f24116d2'
  }
}

$architecture = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$target = $targets[$architecture]

$exePath = Join-Path $toolsDir 'helioslite.exe'

Get-ChocolateyWebFile `
  -PackageName    'helioslite' `
  -FileFullPath   $exePath `
  -Url64bit       "$baseUrl/$($target.Asset)" `
  -Checksum64     $target.Checksum `
  -ChecksumType64 'sha256'

# `helioslite` is the canonical command; `forge` is the retained legacy alias.
# Both shims target the same executable, which selects the name it reports from
# how it was invoked.
Install-BinFile -Name 'helioslite' -Path $exePath
Install-BinFile -Name 'forge' -Path $exePath
