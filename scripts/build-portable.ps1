<#
.SYNOPSIS
Builds the NOORwave portable zip from source.
.DESCRIPTION
Run from the workspace root: .\scripts\build-portable.ps1
Outputs: dist\NOORwave-portable.zip

CI passes -UsePrebuiltFrontend -UsePrebuiltBinaries after compiling the UI and
both release executables. Package before installer bundling patches the app.
#>

param(
    [switch]$UsePrebuiltFrontend,
    [switch]$UsePrebuiltBinaries
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,

        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]]$Arguments
    )

    & $FilePath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FilePath $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

Write-Host "=== NOORwave Portable Build ===" -ForegroundColor Cyan

# 1. Build frontend, unless CI already did it.
Write-Host "1/4 Preparing frontend..." -ForegroundColor Yellow
$FrontendBuild = Join-Path $Root "frontend\build"
$FrontendIndex = Join-Path $FrontendBuild "index.html"
if ($UsePrebuiltFrontend) {
    if (-not (Test-Path $FrontendIndex)) {
        throw "-UsePrebuiltFrontend was set, but frontend\build\index.html does not exist"
    }
    Write-Host "    using prebuilt frontend" -ForegroundColor Green
} else {
    Push-Location (Join-Path $Root "frontend")
    try {
        Invoke-Native -FilePath pnpm -Arguments @("run", "build")
    }
    finally {
        Pop-Location
    }
    Write-Host "    frontend built" -ForegroundColor Green
}

# 2/3. Build the binaries locally, or package the outputs CI already compiled.
if ($UsePrebuiltBinaries) {
    foreach ($binary in @("noor-server.exe", "noor-app.exe")) {
        $binaryPath = Join-Path $Root "target\release\$binary"
        if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
            throw "-UsePrebuiltBinaries was set, but target\release\$binary does not exist"
        }
    }
    Write-Host "2/4 Using prebuilt noor-server..." -ForegroundColor Green
    Write-Host "3/4 Using prebuilt noor-app..." -ForegroundColor Green
} else {
    Write-Host "2/4 Building noor-server..." -ForegroundColor Yellow
    Invoke-Native -FilePath cargo -Arguments @("build", "--release", "--locked", "-p", "noor-server")
    Write-Host "    noor-server built" -ForegroundColor Green

    Write-Host "3/4 Building noor-app..." -ForegroundColor Yellow
    Invoke-Native -FilePath cargo -Arguments @("build", "--release", "--locked", "-p", "noor-app")
    Write-Host "    noor-app built" -ForegroundColor Green
}

# 4. Assemble portable folder
Write-Host "4/4 Assembling portable folder..." -ForegroundColor Yellow
$Dist = Join-Path $Root "dist\NOORwave"
if (Test-Path $Dist) { Remove-Item $Dist -Recurse -Force }
New-Item -ItemType Directory -Force $Dist | Out-Null

Copy-Item (Join-Path $Root "target\release\noor-app.exe") (Join-Path $Dist "NOORwave.exe")
Copy-Item (Join-Path $Root "target\release\noor-server.exe") (Join-Path $Dist "noor-server.exe")
Copy-Item -Recurse $FrontendBuild (Join-Path $Dist "www")

# 5. Zip
$ZipPath = Join-Path $Root "dist\NOORwave-portable.zip"
if (Test-Path $ZipPath) { Remove-Item $ZipPath -Force }
Compress-Archive -Path $Dist -DestinationPath $ZipPath

$Size = [math]::Round((Get-Item $ZipPath).Length / 1MB, 1)
Write-Host ""
Write-Host "Build complete!" -ForegroundColor Green
Write-Host "Output: dist\NOORwave-portable.zip ($Size MB)"
