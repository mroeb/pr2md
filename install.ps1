<#
.SYNOPSIS
    Install or update pr2md for the current user (no admin rights needed).

.DESCRIPTION
    Copies pr2md.exe to %LOCALAPPDATA%\Programs\pr2md, registers it to start at login and
    starts the tray app. Run it again after rebuilding: if the binary changed, the running
    instance is stopped, the file is replaced and pr2md is restarted. If nothing changed,
    nothing is touched.

.EXAMPLE
    .\install.ps1                      # uses target\release\pr2md.exe (builds it if missing)
    .\install.ps1 -Source C:\Downloads\pr2md.exe
    .\install.ps1 -Build               # always run cargo build --release first
    .\install.ps1 -Uninstall
#>
param(
    [string]$Source,
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA 'Programs\pr2md'),
    [switch]$Build,
    [switch]$NoStart,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$dest = Join-Path $InstallDir 'pr2md.exe'

function Get-Sha256([string]$path) {
    # Get-FileHash is not available everywhere (module autoloading can be disabled).
    $sha = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($path)
    try { [System.BitConverter]::ToString($sha.ComputeHash($stream)) }
    finally { $stream.Dispose(); $sha.Dispose() }
}

function Stop-Pr2md {
    $running = Get-Process -Name pr2md -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $dest) }
    if ($running) {
        Write-Host 'Stopping running pr2md...'
        $running | Stop-Process -Force
        $running | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
    }
}

if ($Uninstall) {
    Stop-Pr2md
    if (Test-Path $dest) { & $dest autostart off | Out-Null }
    Remove-Item -Recurse -Force $InstallDir -ErrorAction SilentlyContinue
    Write-Host "pr2md removed. Your settings and token are still in $env:APPDATA\pr2md (delete that folder to remove them)."
    return
}

# --- find (or build) the binary to install
if (-not $Source) {
    $candidates = @(
        (Join-Path $here 'target\release\pr2md.exe'),   # source checkout
        (Join-Path $here 'pr2md.exe')                    # unpacked download
    )
    $Source = $candidates | Where-Object { Test-Path $_ } | Select-Object -First 1
    $isCheckout = Test-Path (Join-Path $here 'Cargo.toml')
    if ($isCheckout -and ($Build -or -not $Source)) {
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            throw 'cargo not found. Install Rust from https://rustup.rs or pass -Source <path to pr2md.exe>.'
        }
        Write-Host 'Building pr2md (cargo build --release)...'
        Push-Location $here
        try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build failed' } }
        finally { Pop-Location }
        $Source = $candidates[0]
    }
}
if (-not $Source -or -not (Test-Path $Source)) {
    throw "No pr2md.exe found. Build it with 'cargo build --release' or pass -Source <path>."
}
$Source = (Resolve-Path $Source).Path

# --- copy only if new or different
$changed = $true
if (Test-Path $dest) {
    $changed = (Get-Sha256 $Source) -ne (Get-Sha256 $dest)
}
if ($changed) {
    Stop-Pr2md
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force $Source $dest
    Write-Host "Installed $dest"
} else {
    Write-Host "Already up to date: $dest"
}

# --- start at login (HKCU Run key, points at the installed file)
& $dest autostart on | Out-Null
Write-Host 'Autostart enabled (starts at login).'

# --- (re)start the tray app
if (-not $NoStart) {
    $alive = Get-Process -Name pr2md -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and ($_.Path -ieq $dest) }
    if (-not $alive) {
        Start-Process -FilePath $dest -ArgumentList 'tray'
        Write-Host 'pr2md is running in the tray.'
    }
}
