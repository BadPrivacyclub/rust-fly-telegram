# fly-telegram installer for Windows.
#
#   irm https://raw.githubusercontent.com/BadPrivacyclub/rust-fly-telegram/main/install.ps1 | iex
#
# With options (download first):
#   .\install.ps1 -DataDir D:\fly -Version v0.2.0 -Service -Start
param(
    [string]$InstallDir = "$env:LOCALAPPDATA\fly-telegram",
    [string]$DataDir = "$env:USERPROFILE\fly-telegram",
    [string]$Version = "latest",
    [switch]$Service,
    [switch]$Start
)

$ErrorActionPreference = "Stop"
$ProgressPreference = "SilentlyContinue"
$Repo = "BadPrivacyclub/rust-fly-telegram"
$Target = "x86_64-pc-windows-msvc"

function Info($text) { Write-Host "> $text" -ForegroundColor Cyan }
function Ok($text) { Write-Host "OK $text" -ForegroundColor Green }
function Warn($text) { Write-Host "! $text" -ForegroundColor Yellow }

Write-Host ""
Write-Host "  fly-telegram installer" -ForegroundColor White
Write-Host ""

if ($env:PROCESSOR_ARCHITECTURE -ne "AMD64") {
    Warn "Only x64 Windows has prebuilt binaries. On ARM64, x64 emulation is used."
}

$base = if ($Version -eq "latest") {
    "https://github.com/$Repo/releases/latest/download"
} else {
    "https://github.com/$Repo/releases/download/$Version"
}
$archive = "fly-telegram-$Target.zip"
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("fly-" + [Guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null

try {
    Info "Downloading $archive ($Version)"
    Invoke-WebRequest -Uri "$base/$archive" -OutFile "$tmp\$archive" -UseBasicParsing

    try {
        Invoke-WebRequest -Uri "$base/sha256sums.txt" -OutFile "$tmp\sha256sums.txt" -UseBasicParsing
        $line = Select-String -Path "$tmp\sha256sums.txt" -Pattern ([regex]::Escape($archive) + '$') | Select-Object -First 1
        if ($line) {
            $expected = ($line.Line -split '\s+')[0]
            $actual = (Get-FileHash "$tmp\$archive" -Algorithm SHA256).Hash.ToLower()
            if ($expected -ne $actual) { throw "checksum mismatch for $archive" }
            Ok "Checksum verified"
        }
    } catch [System.Net.WebException] {
        Warn "No checksum file published; skipping verification."
    }

    Expand-Archive -Path "$tmp\$archive" -DestinationPath "$tmp\out" -Force
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item "$tmp\out\fly-telegram.exe" "$InstallDir\fly-telegram.exe" -Force
    Copy-Item "$tmp\out\music-worker.exe" "$InstallDir\fly-telegram-music-worker.exe" -Force
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}

$exe = Join-Path $InstallDir "fly-telegram.exe"
Ok ("Installed $exe (" + (& $exe --version) + ")")

New-Item -ItemType Directory -Force -Path $DataDir | Out-Null
& $exe --data-dir $DataDir init | Out-Null
Ok "Data directory: $DataDir"

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($userPath -split ';' | Where-Object { $_ -eq $InstallDir })) {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$InstallDir", "User")
    Ok "Added $InstallDir to your PATH (open a new terminal to use it)"
}

if ($Service) {
    & $exe --data-dir $DataDir service install
    & schtasks /Run /TN fly-telegram | Out-Null
}

Write-Host ""
Write-Host "Next steps" -ForegroundColor White
if ($Service) {
    Write-Host "  Open http://127.0.0.1:8080 and follow the setup wizard."
} else {
    Write-Host "  1. Run:   fly-telegram --data-dir `"$DataDir`""
    Write-Host "  2. Open:  http://127.0.0.1:8080 and follow the setup wizard"
    Write-Host "  3. Later: fly-telegram --data-dir `"$DataDir`" service install  (start at logon)"
}
Write-Host "  Check your setup any time with: fly-telegram --data-dir `"$DataDir`" doctor"
Write-Host ""

if ($Start -and -not $Service) {
    Start-Process "http://127.0.0.1:8080"
    & $exe --data-dir $DataDir
}
