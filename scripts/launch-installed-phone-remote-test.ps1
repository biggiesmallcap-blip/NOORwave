<#
.SYNOPSIS
Launches the installed NOORwave package against an isolated test profile.

.DESCRIPTION
LOCALAPPDATA is overridden only for the launched process and its managed
sidecar. The ordinary installed profile remains untouched. The script refuses
to launch while another NOORwave app or server process is present because the
single-instance owner could otherwise redirect the request to the live app.
#>

param(
    [string]$FixtureRoot = ".scratch\phone-remote-installed",
    [string]$InstalledApp = (Join-Path $env:LOCALAPPDATA "Programs\NOORwave\noor-app.exe")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$Root = (Resolve-Path -LiteralPath (Split-Path $PSScriptRoot -Parent)).Path
$FixtureRoot = [System.IO.Path]::GetFullPath((Join-Path $Root $FixtureRoot))
if (-not $FixtureRoot.StartsWith($Root, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Fixture root must stay inside the repository: $FixtureRoot"
}

$fixtureLocalAppData = Join-Path $FixtureRoot "LocalAppData"
$fixtureDb = Join-Path $fixtureLocalAppData "NOORwave\noor.db"
if (-not (Test-Path -LiteralPath $fixtureDb -PathType Leaf)) {
    throw "Fixture database not found: $fixtureDb"
}
if (-not (Test-Path -LiteralPath $InstalledApp -PathType Leaf)) {
    throw "Installed NOORwave executable not found: $InstalledApp"
}

$running = Get-Process -Name "noor-app", "noor-server" -ErrorAction SilentlyContinue
if ($running) {
    $details = ($running | ForEach-Object { "$($_.ProcessName) ($($_.Id))" }) -join ", "
    throw "Exit every NOORwave instance before launching the fixture: $details"
}

$startInfo = [System.Diagnostics.ProcessStartInfo]::new()
$startInfo.FileName = [System.IO.Path]::GetFullPath($InstalledApp)
$startInfo.UseShellExecute = $false
$startInfo.EnvironmentVariables["LOCALAPPDATA"] = $fixtureLocalAppData
$process = [System.Diagnostics.Process]::Start($startInfo)
if ($null -eq $process) {
    throw "NOORwave did not start"
}

[pscustomobject]@{
    ProcessId = $process.Id
    Executable = $startInfo.FileName
    FixtureLocalAppData = $fixtureLocalAppData
    FixtureDatabase = $fixtureDb
} | Format-List
