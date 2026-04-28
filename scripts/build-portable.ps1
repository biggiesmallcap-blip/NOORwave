<#
.SYNOPSIS
Builds the NOORwave portable zip from source.
.DESCRIPTION
Run from the workspace root: .\scripts\build-portable.ps1
Outputs: dist\NOORwave-portable.zip
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root

Write-Host "=== NOORwave Portable Build ===" -ForegroundColor Cyan

# 1. Build noor-server
Write-Host "1/3 Building noor-server..." -ForegroundColor Yellow
cargo build --release -p noor-server
Write-Host "    noor-server built" -ForegroundColor Green

# 2. Build noor-app (Tauri shell)
Write-Host "2/3 Building noor-app..." -ForegroundColor Yellow
cargo build --release -p noor-app
Write-Host "    noor-app built" -ForegroundColor Green

# 3. Assemble portable folder
Write-Host "3/3 Assembling portable folder..." -ForegroundColor Yellow
$Dist = Join-Path $Root "dist\NOORwave"
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Force $Dist | Out-Null

Copy-Item (Join-Path $Root "target\release\noor-app.exe") (Join-Path $Dist "NOORwave.exe")
Copy-Item (Join-Path $Root "target\release\noor-server.exe") (Join-Path $Dist "noor-server.exe")
Copy-Item -Recurse (Join-Path $Root "frontend\build") (Join-Path $Dist "www")

# 5. Zip
$ZipPath = Join-Path $Root "dist\NOORwave-portable.zip"
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path $Dist -DestinationPath $ZipPath

$Size = [math]::Round((Get-Item $ZipPath).Length / 1MB, 1)
Write-Host ""
Write-Host "Build complete!" -ForegroundColor Green
Write-Host "Output: dist\NOORwave-portable.zip ($Size MB)"
Write-Host ""
Write-Host "NOTE: Frontend must be built first: cd frontend && pnpm install && pnpm run build"
