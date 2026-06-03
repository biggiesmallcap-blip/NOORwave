[CmdletBinding()]
param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [switch]$UseExistingCoverage,
    [switch]$SkipFrontendCoverage,
    [switch]$SkipRustCoverage,
    [string]$FrontendLcov,
    [string]$RustLcov
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$emDash = [string][char]0x2014
$ellipsis = [string][char]0x2026

function Convert-ToAsciiText {
    param([string]$Text)

    $normalized = $Text.Replace($emDash, "-").Replace($ellipsis, "...")
    return [regex]::Replace($normalized, "[^\u0000-\u007F]", " ")
}

$repoRoot = (Resolve-Path -LiteralPath $Root).Path
if (-not $FrontendLcov) {
    $FrontendLcov = Join-Path $repoRoot "frontend\coverage\lcov.info"
}
if (-not $RustLcov) {
    $RustLcov = Join-Path $repoRoot "target\llvm-cov\noorwave.lcov"
}

$reportDir = Join-Path $repoRoot "docs\dev\repowise-health"
New-Item -ItemType Directory -Path $reportDir -Force | Out-Null

function Invoke-LoggedCommand {
    param(
        [string]$Title,
        [string]$Command,
        [string[]]$Arguments,
        [string]$WorkingDirectory
    )

    Write-Host ""
    Write-Host "== $Title =="
    Push-Location $WorkingDirectory
    try {
        $previousErrorActionPreference = $ErrorActionPreference
        $ErrorActionPreference = "Continue"
        $output = & $Command @Arguments 2>&1
        $exitCode = $LASTEXITCODE
        foreach ($line in @($output | ForEach-Object { Convert-ToAsciiText $_.ToString() })) {
            Write-Host $line
        }
        return $exitCode
    } finally {
        if ($null -ne $previousErrorActionPreference) {
            $ErrorActionPreference = $previousErrorActionPreference
        }
        Pop-Location
    }
}

function Invoke-RepowiseReport {
    param(
        [string[]]$Arguments,
        [string]$OutputPath
    )

    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & repowise @Arguments 2>&1
        $exitCode = $LASTEXITCODE
        $lines = @($output | ForEach-Object { Convert-ToAsciiText $_.ToString() })
        Set-Content -LiteralPath $OutputPath -Value $lines -Encoding UTF8
        foreach ($line in $lines) {
            Write-Host $line
        }
        return $exitCode
    } finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
}

if (-not $UseExistingCoverage) {
    if (-not $SkipFrontendCoverage) {
        $frontendExit = Invoke-LoggedCommand `
            -Title "frontend coverage" `
            -Command "pnpm" `
            -Arguments @("run", "test:coverage") `
            -WorkingDirectory (Join-Path $repoRoot "frontend")
        if ($frontendExit -ne 0) {
            throw "pnpm run test:coverage failed with exit code $frontendExit"
        }
    }

    if (-not $SkipRustCoverage) {
        $llvmCov = Get-Command "cargo-llvm-cov" -ErrorAction SilentlyContinue
        if (-not $llvmCov) {
            Write-Warning "cargo-llvm-cov was not found. Install with cargo install cargo-llvm-cov or rerun with -SkipRustCoverage."
        } else {
            New-Item -ItemType Directory -Path (Split-Path -Parent $RustLcov) -Force | Out-Null
            $rustExit = Invoke-LoggedCommand `
                -Title "rust coverage" `
                -Command "cargo" `
                -Arguments @("llvm-cov", "--package", "noor-server", "--package", "noor-mix", "--lcov", "--output-path", $RustLcov) `
                -WorkingDirectory $repoRoot
            if ($rustExit -ne 0) {
                throw "cargo llvm-cov failed with exit code $rustExit"
            }
        }
    }
}

$coverageArgs = @()
foreach ($coverageFile in @($FrontendLcov, $RustLcov)) {
    if (Test-Path -LiteralPath $coverageFile) {
        $coverageArgs += @("--coverage", $coverageFile)
    } else {
        Write-Warning "Coverage file not found: $coverageFile"
    }
}

$snapshotPath = Join-Path $reportDir "repowise-health-latest.md"
$trendPath = Join-Path $reportDir "repowise-health-trend.md"

Push-Location $repoRoot
try {
    $healthArgs = @("health", "--format", "md", "--refactoring-targets") + $coverageArgs
    $healthExit = Invoke-RepowiseReport -Arguments $healthArgs -OutputPath $snapshotPath
    if ($healthExit -ne 0) {
        throw "repowise health failed with exit code $healthExit"
    }

    $trendExit = Invoke-RepowiseReport -Arguments @("health", "--trend") -OutputPath $trendPath
    if ($trendExit -ne 0) {
        throw "repowise health --trend failed with exit code $trendExit"
    }
} finally {
    Pop-Location
}

Write-Host "Repowise health snapshot: $snapshotPath"
Write-Host "Repowise trend snapshot: $trendPath"
