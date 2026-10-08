<#
.SYNOPSIS
Checks the exact master commit before a NOORwave release tag is pushed.
.EXAMPLE
scripts\release-preflight.ps1 -Tag v0.17.17
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^v\d+\.\d+\.\d+$')]
    [string]$Tag,
    [switch]$AllowExistingTag
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path $PSScriptRoot -Parent
Push-Location -LiteralPath $repoRoot
try {
    function Invoke-Checked([string]$File, [string[]]$Arguments) {
        $output = & $File @Arguments
        if ($LASTEXITCODE -ne 0) {
            throw "$File $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
        }
        return $output
    }

    $version = $Tag.Substring(1)
    $dirtyTracked = @(Invoke-Checked git @('status', '--porcelain', '--untracked-files=no'))
    if ($dirtyTracked.Count -gt 0) { throw 'Tracked files are dirty. Commit the release preparation first.' }

    $head = (Invoke-Checked git @('rev-parse', 'HEAD') | Select-Object -First 1).Trim()
    $remoteMaster = (Invoke-Checked git @('ls-remote', 'origin', 'refs/heads/master') | Select-Object -First 1).Split("`t")[0]
    if ($head -ne $remoteMaster) { throw "HEAD $head is not current origin/master $remoteMaster. Merge and update the checkout first." }

    foreach ($manifestPath in @('noor-server/Cargo.toml', 'noor-app/Cargo.toml')) {
        $content = Get-Content -Raw -LiteralPath $manifestPath
        $found = [regex]::Match($content, '(?m)^version = "([^"]+)"')
        if (-not $found.Success -or $found.Groups[1].Value -ne $version) {
            throw "$manifestPath must declare version $version"
        }
    }
    $tauri = Get-Content -Raw -LiteralPath 'noor-app/tauri.conf.json' | ConvertFrom-Json
    if ($tauri.version -ne $version) { throw "Tauri config must declare version $version" }
    if ($tauri.bundle.windows.nsis.installMode -ne 'currentUser') { throw 'NSIS installMode must remain currentUser' }

    $lock = Get-Content -Raw -LiteralPath 'Cargo.lock'
    foreach ($package in @('noor-app', 'noor-server')) {
        $pattern = '(?m)^name = "' + [regex]::Escape($package) + '"\r?\nversion = "([^"]+)"'
        $found = [regex]::Match($lock, $pattern)
        if (-not $found.Success -or $found.Groups[1].Value -ne $version) {
            throw "Cargo.lock $package must declare version $version"
        }
    }

    $notesPath = Join-Path 'docs/releases' "$Tag.md"
    if (-not (Test-Path -LiteralPath $notesPath)) { throw "Missing $notesPath" }
    $notes = (Get-Content -Raw -LiteralPath $notesPath).Trim()
    $heading = "## What's new in $Tag"
    if (-not $notes.StartsWith($heading) -or [string]::IsNullOrWhiteSpace($notes.Substring($heading.Length))) {
        throw "$notesPath needs the exact heading and user-facing details"
    }

    $null = Invoke-Checked gh @('api', 'user', '--jq', '.login')
    $secretRows = @(Invoke-Checked gh @('secret', 'list', '--app', 'actions'))
    foreach ($secret in @('TAURI_SIGNING_PRIVATE_KEY', 'TAURI_SIGNING_PRIVATE_KEY_PASSWORD')) {
        if (-not ($secretRows | Where-Object { $_ -match ('^' + $secret + '\s') })) {
            throw "Missing GitHub Actions secret $secret"
        }
    }

    # A release-only commit cannot change what PR check verifies, and the Release workflow
    # rebuilds every platform anyway, so the green run of its parent is enough.
    $ciSha = $head
    $releaseFiles = @('noor-server/Cargo.toml', 'noor-app/Cargo.toml', 'noor-app/tauri.conf.json', 'Cargo.lock', "docs/releases/$Tag.md")
    $changed = @(Invoke-Checked git @('diff', '--name-only', 'HEAD^1', 'HEAD'))
    if ($changed.Count -gt 0 -and @($changed | Where-Object { $releaseFiles -notcontains $_ }).Count -eq 0) {
        $ciSha = (Invoke-Checked git @('rev-parse', 'HEAD^1') | Select-Object -First 1).Trim()
    }
    $runs = Invoke-Checked gh @('run', 'list', '--workflow', 'PR check', '--branch', 'master', '--limit', '20', '--json', 'headSha,status,conclusion,databaseId') | ConvertFrom-Json
    $run = @($runs | Where-Object { $_.headSha -eq $ciSha -and $_.status -eq 'completed' -and $_.conclusion -eq 'success' } | Select-Object -First 1)
    if ($run.Count -eq 0) { throw "No successful completed master PR check for $ciSha. Wait for it before tagging." }

    if (-not $AllowExistingTag) {
        $remoteTag = @(Invoke-Checked git @('ls-remote', '--tags', 'origin', "refs/tags/$Tag"))
        if ($remoteTag.Count -gt 0) { throw "Remote tag $Tag already exists" }
    }

    Write-Host "Release preflight passed: $Tag on $head, master CI run $($run[0].databaseId) on $ciSha, signing secrets present."
}
finally {
    Pop-Location
}
