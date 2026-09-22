<#
.SYNOPSIS
Creates an isolated phone-remote test profile from the installed NOORwave database.

.DESCRIPTION
Uses SQLite's online backup API so a live WAL database is copied consistently.
The source database is opened read-only and is never replaced, checkpointed, or
otherwise modified. The destination defaults to the git-ignored .scratch tree.

Run from the repository root with PowerShell 7:
  pwsh .\scripts\prepare-phone-remote-test.ps1
#>

param(
    [string]$SourceDataDir = (Join-Path $env:LOCALAPPDATA "NOORwave"),
    [string]$FixtureRoot = ".scratch\phone-remote-installed",
    [switch]$IncludeSecret
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = Split-Path $PSScriptRoot -Parent
$Root = (Resolve-Path -LiteralPath $Root).Path
$FixtureRoot = [System.IO.Path]::GetFullPath((Join-Path $Root $FixtureRoot))
if (-not $FixtureRoot.StartsWith($Root, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Fixture root must stay inside the repository: $FixtureRoot"
}

$sqlite = Get-Command sqlite3 -ErrorAction Stop
$sourceDb = Join-Path ([System.IO.Path]::GetFullPath($SourceDataDir)) "noor.db"
if (-not (Test-Path -LiteralPath $sourceDb -PathType Leaf)) {
    throw "Source database not found: $sourceDb"
}

$fixtureDataDir = Join-Path $FixtureRoot "LocalAppData\NOORwave"
$fixtureDb = Join-Path $fixtureDataDir "noor.db"
if (Test-Path -LiteralPath $fixtureDb) {
    throw "Fixture already exists; choose a new -FixtureRoot: $fixtureDb"
}
New-Item -ItemType Directory -Path $fixtureDataDir -Force | Out-Null

$sqliteDestination = $fixtureDb.Replace("\", "/").Replace("'", "''")
& $sqlite.Source $sourceDb ".timeout 30000" ".backup '$sqliteDestination'"
if ($LASTEXITCODE -ne 0) {
    throw "SQLite online backup failed with exit code $LASTEXITCODE"
}

$quickCheck = & $sqlite.Source -readonly $fixtureDb "PRAGMA quick_check;"
if ($LASTEXITCODE -ne 0 -or $quickCheck -ne "ok") {
    throw "Fixture verification failed: $quickCheck"
}

$config = "{`n  `"host_mode`": false,`n  `"minimize_to_tray`": false`n}`n"
[System.IO.File]::WriteAllText(
    (Join-Path $fixtureDataDir "noor-config.json"),
    $config,
    [System.Text.UTF8Encoding]::new($false)
)

if ($IncludeSecret) {
    $sourceSecret = Join-Path $SourceDataDir ".noor_secret"
    if (-not (Test-Path -LiteralPath $sourceSecret -PathType Leaf)) {
        throw "-IncludeSecret was requested but the source key is missing: $sourceSecret"
    }
    Copy-Item -LiteralPath $sourceSecret -Destination (Join-Path $fixtureDataDir ".noor_secret")
}

$source = Get-Item -LiteralPath $sourceDb
$fixture = Get-Item -LiteralPath $fixtureDb
[pscustomobject]@{
    Source = $source.FullName
    SourceBytes = $source.Length
    Fixture = $fixture.FullName
    FixtureBytes = $fixture.Length
    QuickCheck = $quickCheck
    SecretCopied = [bool]$IncludeSecret
} | Format-List
