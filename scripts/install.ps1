# tuimux installer - Windows PowerShell.
#
# Usage:
#   irm https://raw.githubusercontent.com/hungryZoo/tuimux/main/scripts/install.ps1 | iex
#
# Environment variables:
#   TUIMUX_VERSION       Tag to install, e.g. v0.2.0-alpha.28. Default: latest prerelease/release.
#   TUIMUX_INSTALL_DIR   Where to put tuimux.exe. Default: %LOCALAPPDATA%\Programs\tuimux\bin.

$ErrorActionPreference = "Stop"

$Repo = "hungryZoo/tuimux"
$Binary = "tuimux"

function Info($Message) {
    Write-Host "==> $Message" -ForegroundColor Green
}

function Warn($Message) {
    Write-Warning $Message
}

function Die($Message) {
    Write-Error $Message
    exit 1
}

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
switch ($Arch) {
    "X64" { $Target = "x86_64-pc-windows-msvc" }
    "Arm64" { $Target = "aarch64-pc-windows-msvc" }
    default { Die "unsupported Windows architecture: $Arch" }
}
Info "Detected Windows / $Arch (target: $Target)"

Info "tmux is optional. The default tuimux UI now uses the Rust-native multiplexer."

$Version = $env:TUIMUX_VERSION
if ([string]::IsNullOrWhiteSpace($Version)) {
    Info "Resolving latest release/prerelease tag..."
    $Releases = Invoke-RestMethod -Headers @{ Accept = "application/vnd.github+json" } `
        -Uri "https://api.github.com/repos/$Repo/releases?per_page=20"
    if (-not $Releases -or -not $Releases[0].tag_name) {
        Die "could not determine the latest release tag. Set TUIMUX_VERSION=vX.Y.Z explicitly."
    }
    $Version = $Releases[0].tag_name
}
Info "Installing $Binary $Version"

$Asset = "$Binary-$Version-$Target.zip"
$BaseUrl = "https://github.com/$Repo/releases/download/$Version"
$DownloadUrl = "$BaseUrl/$Asset"
$Temp = Join-Path ([System.IO.Path]::GetTempPath()) ("tuimux-install-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory -Force -Path $Temp | Out-Null

try {
    $Archive = Join-Path $Temp $Asset
    Info "Downloading $Asset..."
    Invoke-WebRequest -Uri $DownloadUrl -OutFile $Archive

    $Sums = Join-Path $Temp "SHA256SUMS"
    try {
        Invoke-WebRequest -Uri "$BaseUrl/SHA256SUMS" -OutFile $Sums
        $ExpectedLine = Get-Content $Sums | Where-Object { $_ -match [regex]::Escape($Asset) } | Select-Object -First 1
        if ($ExpectedLine) {
            $Expected = ($ExpectedLine -split "\s+")[0].ToLowerInvariant()
            $Actual = (Get-FileHash -Algorithm SHA256 $Archive).Hash.ToLowerInvariant()
            if ($Expected -ne $Actual) {
                Die "checksum mismatch! expected $Expected, got $Actual"
            }
            Info "Checksum OK ($($Actual.Substring(0, 12))...)"
        }
    } catch {
        Warn "no SHA256SUMS published for $Version; skipping checksum verification."
    }

    Info "Extracting..."
    Expand-Archive -Force -Path $Archive -DestinationPath $Temp
    $Exe = Get-ChildItem -Path $Temp -Recurse -Filter "$Binary.exe" | Select-Object -First 1
    if (-not $Exe) {
        Die "could not find $Binary.exe inside the archive."
    }

    $InstallDir = $env:TUIMUX_INSTALL_DIR
    if ([string]::IsNullOrWhiteSpace($InstallDir)) {
        $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\tuimux\bin"
    }
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $Dest = Join-Path $InstallDir "$Binary.exe"
    Copy-Item -Force $Exe.FullName $Dest

    Info "Installed $Binary to $Dest"
    $PathParts = ($env:PATH -split ";") | Where-Object { $_ }
    if ($PathParts -notcontains $InstallDir) {
        Warn "$InstallDir is not on PATH. Add it from PowerShell with: [Environment]::SetEnvironmentVariable('Path', `$env:Path + ';$InstallDir', 'User')"
    }
    Info "Done. Verify with: $Binary --version and $Binary --doctor"
} finally {
    Remove-Item -Recurse -Force $Temp -ErrorAction SilentlyContinue
}
