<#
.SYNOPSIS
Builds a local Windows 11 portable NOORwave release.

.DESCRIPTION
Run from the workspace root:
  .\scripts\build-windows11-release.ps1

The script checks for the tools needed to build NOORwave, prints clear install
commands for missing tools, builds the frontend and both Rust binaries, then
assembles:
  dist\NOORwave-win11\
  dist\NOORwave-win11.zip

Use -CheckOnly to scan dependencies without building.
Use -InstallMissing to install missing tools with winget where possible.
#>

param(
    [switch]$CheckOnly,
    [switch]$InstallMissing,
    [switch]$SkipFrontendInstall,
    [switch]$NoZip
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path $PSScriptRoot -Parent
Set-Location $Root

$Missing = New-Object System.Collections.Generic.List[object]

function Write-Step {
    param([string]$Text)
    Write-Host ""
    Write-Host "== $Text ==" -ForegroundColor Cyan
}

function Write-Ok {
    param([string]$Text)
    Write-Host "OK  $Text" -ForegroundColor Green
}

function Write-Warn {
    param([string]$Text)
    Write-Host "NOTE $Text" -ForegroundColor Yellow
}

function Write-Fail {
    param([string]$Text)
    Write-Host "NEED $Text" -ForegroundColor Yellow
}

function Get-ToolPath {
    param([string]$Name)
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        return $null
    }
    return $cmd.Source
}

function Add-MissingTool {
    param(
        [string]$Name,
        [string]$Reason,
        [string]$InstallCommand
    )
    $Missing.Add([pscustomobject]@{
        Name = $Name
        Reason = $Reason
        InstallCommand = $InstallCommand
    }) | Out-Null
}

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

function Resolve-RequiredChildPath {
    param(
        [string]$BasePath,
        [string]$ChildPath
    )

    $base = (Resolve-Path -LiteralPath $BasePath).Path
    $full = [System.IO.Path]::GetFullPath((Join-Path $base $ChildPath))
    if (-not $full.StartsWith($base, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Refusing to use path outside workspace: $full"
    }
    return $full
}

function Test-MsvcBuildTools {
    $cl = Get-ToolPath "cl.exe"
    if ($cl) {
        return $true
    }

    $vswhere = Join-Path ${env:ProgramFiles(x86)} "Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path -LiteralPath $vswhere) {
        $installPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($installPath)) {
            return $true
        }
    }

    $commonPaths = @(
        "${env:ProgramFiles}\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat",
        "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
    )
    foreach ($path in $commonPaths) {
        if (Test-Path -LiteralPath $path) {
            return $true
        }
    }

    return $false
}

function Test-IsWindowsHost {
    if ($PSVersionTable.PSEdition -eq "Core") {
        return [System.Runtime.InteropServices.RuntimeInformation]::IsOSPlatform(
            [System.Runtime.InteropServices.OSPlatform]::Windows
        )
    }

    return $env:OS -eq "Windows_NT"
}

function Test-RequiredFile {
    param(
        [string]$Path,
        [string]$Name
    )
    if (-not (Test-Path -LiteralPath $Path)) {
        throw "$Name was not found at $Path. Run this script from the NOORwave repo root."
    }
}

function Check-Tool {
    param(
        [string]$Name,
        [string]$Command,
        [string]$InstallCommand
    )

    $path = Get-ToolPath $Command
    if ($path) {
        $version = ""
        try {
            $version = (& $Command --version 2>$null | Select-Object -First 1)
        } catch {
            $version = $path
        }
        Write-Ok "$Name found: $version"
        return
    }

        Write-Fail "$Name not found yet"
    Add-MissingTool -Name $Name -Reason "$Command is not on PATH" -InstallCommand $InstallCommand
}

function Install-MissingTools {
    if ($Missing.Count -eq 0) {
        return
    }

    $winget = Get-ToolPath "winget.exe"
    if (-not $winget) {
        Write-Fail "winget is not available. Install the missing tools manually, then rerun this script."
        return
    }

    Write-Step "Installing missing tools with winget"
    foreach ($tool in $Missing) {
        Write-Host ""
        Write-Host "Installing $($tool.Name)..."
        switch ($tool.Name) {
            "Rust" {
                Invoke-Native -FilePath $winget -Arguments @("install", "--id", "Rustlang.Rustup", "-e")
            }
            "Node.js LTS" {
                Invoke-Native -FilePath $winget -Arguments @("install", "--id", "OpenJS.NodeJS.LTS", "-e")
            }
            "pnpm" {
                Invoke-Native -FilePath $winget -Arguments @("install", "--id", "pnpm.pnpm", "-e")
            }
            "Visual Studio Build Tools" {
                Invoke-Native -FilePath $winget -Arguments @(
                    "install",
                    "--id",
                    "Microsoft.VisualStudio.2022.BuildTools",
                    "-e",
                    "--override",
                    "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
                )
            }
        }
    }

    Write-Warn "Close this PowerShell window, open a new one, then rerun the script."
}

function Scan-Dependencies {
    Write-Step "Checking build dependencies"

    if (-not (Test-IsWindowsHost)) {
        throw "This script only supports Windows."
    }

    $osCaption = "Windows"
    $osVersion = [System.Environment]::OSVersion.Version.ToString()
    try {
        $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop
        $osCaption = $os.Caption
        $osVersion = $os.Version
    } catch {
        Write-Warn "Could not read detailed Windows version: $($_.Exception.Message)"
    }

    Write-Ok "Windows detected: $osCaption $osVersion"
    if ($osCaption -notmatch "Windows 11") {
        Write-Warn "This is not Windows 11. The build may still work, but this script is tuned for Windows 11."
    }

    Test-RequiredFile -Path (Join-Path $Root "Cargo.toml") -Name "Cargo workspace"
    Test-RequiredFile -Path (Join-Path $Root "frontend\package.json") -Name "frontend package"
    Test-RequiredFile -Path (Join-Path $Root "noor-app\Cargo.toml") -Name "noor-app package"
    Test-RequiredFile -Path (Join-Path $Root "noor-server\Cargo.toml") -Name "noor-server package"

    Check-Tool -Name "Rust" -Command "cargo" -InstallCommand "winget install --id Rustlang.Rustup -e"
    Check-Tool -Name "Node.js LTS" -Command "node" -InstallCommand "winget install --id OpenJS.NodeJS.LTS -e"
    Check-Tool -Name "pnpm" -Command "pnpm" -InstallCommand "winget install --id pnpm.pnpm -e"

    if (Test-MsvcBuildTools) {
        Write-Ok "Visual Studio C++ Build Tools found"
    } else {
        Write-Fail "Visual Studio C++ Build Tools not found"
        Add-MissingTool `
            -Name "Visual Studio Build Tools" `
            -Reason "Rust MSVC builds need the C++ build tools" `
            -InstallCommand 'winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"'
    }

    if ($Missing.Count -eq 0) {
        Write-Ok "All required build tools found"
        return
    }

    Write-Step "Things to install"
    foreach ($tool in $Missing) {
        Write-Host ""
        Write-Fail "$($tool.Name): $($tool.Reason)"
        Write-Host "Install:"
        Write-Host "  $($tool.InstallCommand)"
    }

    if ($InstallMissing) {
        Install-MissingTools
    }

    throw "Install the items above, open a new PowerShell window, then rerun this script."
}

function Build-Frontend {
    Write-Step "Building frontend"

    $nodeModules = Join-Path $Root "frontend\node_modules"
    if (-not (Test-Path -LiteralPath $nodeModules)) {
        if ($SkipFrontendInstall) {
            throw "frontend\node_modules is missing. Rerun without -SkipFrontendInstall."
        }
        Write-Warn "frontend\node_modules not found. Running pnpm install."
        Invoke-Native -FilePath "pnpm" -Arguments @("--dir", "frontend", "install")
    }

    Invoke-Native -FilePath "pnpm" -Arguments @("--dir", "frontend", "run", "build")
}

function Build-Rust {
    Write-Step "Building Rust release binaries"
    Invoke-Native -FilePath "cargo" -Arguments @("build", "--release", "-p", "noor-server")
    Invoke-Native -FilePath "cargo" -Arguments @("build", "--release", "-p", "noor-app")
}

function Assemble-Portable {
    Write-Step "Assembling portable folder"

    $distRoot = Resolve-RequiredChildPath -BasePath $Root -ChildPath "dist"
    $outDir = Resolve-RequiredChildPath -BasePath $Root -ChildPath "dist\NOORwave-win11"
    $zipPath = Resolve-RequiredChildPath -BasePath $Root -ChildPath "dist\NOORwave-win11.zip"

    New-Item -ItemType Directory -Force -Path $distRoot | Out-Null
    if (Test-Path -LiteralPath $outDir) {
        Remove-Item -LiteralPath $outDir -Recurse -Force
    }
    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }

    New-Item -ItemType Directory -Force -Path $outDir | Out-Null

    Copy-Item -LiteralPath (Join-Path $Root "target\release\noor-app.exe") -Destination (Join-Path $outDir "NOORwave.exe")
    Copy-Item -LiteralPath (Join-Path $Root "target\release\noor-server.exe") -Destination (Join-Path $outDir "noor-server.exe")
    Copy-Item -LiteralPath (Join-Path $Root "frontend\build") -Destination (Join-Path $outDir "www") -Recurse

    if (Get-Command Unblock-File -ErrorAction SilentlyContinue) {
        Write-Step "Clearing local file block markers"
        Get-ChildItem -LiteralPath $outDir -Recurse -File | ForEach-Object {
            Unblock-File -LiteralPath $_.FullName -ErrorAction SilentlyContinue
        }
    }

    if (-not $NoZip) {
        Write-Step "Creating zip"
        Compress-Archive -LiteralPath $outDir -DestinationPath $zipPath -Force
        if (Get-Command Unblock-File -ErrorAction SilentlyContinue) {
            Unblock-File -LiteralPath $zipPath -ErrorAction SilentlyContinue
        }
        $size = [math]::Round((Get-Item -LiteralPath $zipPath).Length / 1MB, 1)
        Write-Ok "Zip ready: dist\NOORwave-win11.zip ($size MB)"
    }

    Write-Ok "Portable folder ready: dist\NOORwave-win11"
}

try {
    Write-Host "NOORwave Windows 11 Release Builder" -ForegroundColor Cyan
    Write-Host "Workspace: $Root"

    Scan-Dependencies
    if ($CheckOnly) {
        Write-Ok "Dependency check complete. No build was run because -CheckOnly was set."
        exit 0
    }

    Build-Frontend
    Build-Rust
    Assemble-Portable

    Write-Host ""
    Write-Ok "Build complete."
    Write-Host ""
    Write-Host "Run it:"
    Write-Host "  .\dist\NOORwave-win11\NOORwave.exe"
    Write-Host ""
    Write-Warn "These binaries are still unsigned. Smart App Control can still block them on strict Windows 11 installs."
} catch {
    Write-Host ""
    Write-Fail $_.Exception.Message
    exit 1
}
