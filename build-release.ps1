<#
.SYNOPSIS
    Build a pr2md release zip for Windows.

.DESCRIPTION
    Runs the tests, builds the release binary and packs it with the installer, README and
    release notes into dist\pr2md-<version>-windows-<arch>.zip.

.EXAMPLE
    .\build-release.ps1
    .\build-release.ps1 -ClientId Ov23liABC...   # bake in the OAuth app for "Sign in with GitHub"
#>
param(
    [string]$ClientId = $env:PR2MD_CLIENT_ID,
    [switch]$SkipTests
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $root

# Files that go into the release, next to pr2md.exe
$files = @('install.cmd', 'install.ps1', 'README.md', 'RELEASE_NOTES.md')

$version = [regex]::Match((Get-Content Cargo.toml -Raw), '(?m)^version\s*=\s*"([^"]+)"').Groups[1].Value
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$name = "pr2md-$version-windows-$arch"
$exe = Join-Path $root 'target\release\pr2md.exe'

# A running pr2md locks the binary and the build would fail with "Access is denied".
$running = Get-Process -Name pr2md -ErrorAction SilentlyContinue | Where-Object { $_.Path -ieq $exe }
if ($running) { throw "pr2md is running from $exe - quit it from the tray menu first." }

if ($ClientId) {
    $env:PR2MD_CLIENT_ID = $ClientId
    Write-Host 'OAuth client id: baked in'
} else {
    Write-Host 'OAuth client id: none (users sign in via GitHub CLI or a token, or set client_id)'
}

function Invoke-Cargo([string[]]$cargoArgs) {
    # cargo reports progress on stderr; don't let PowerShell treat that as an error.
    $ErrorActionPreference = 'Continue'
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo $($cargoArgs -join ' ') failed" }
}

if (-not $SkipTests) { Invoke-Cargo @('test', '--release') }
Invoke-Cargo @('build', '--release')

# Stage and zip
$dist = Join-Path $root 'dist'
$stage = Join-Path $dist $name
$zip = Join-Path $dist "$name.zip"
Remove-Item -Recurse -Force $stage, $zip -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
Copy-Item $exe $stage
foreach ($f in $files) { Copy-Item (Join-Path $root $f) $stage }

# Write entries by hand: ZipFile.CreateFromDirectory on Windows PowerShell stores
# backslashes in entry names, which breaks unzip on other systems.
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$stream = [System.IO.File]::Open($zip, 'Create')
$archive = New-Object System.IO.Compression.ZipArchive($stream, 'Create')
try {
    foreach ($file in Get-ChildItem $stage) {
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
            $archive, $file.FullName, "$name/$($file.Name)", 'Optimal') | Out-Null
    }
} finally {
    $archive.Dispose()
    $stream.Dispose()
}

$size = '{0:N1} MB' -f ((Get-Item $zip).Length / 1MB)
Write-Host ''
Write-Host "Release: $zip ($size)"
Get-ChildItem $stage | ForEach-Object { Write-Host "  $name\$($_.Name)" }
