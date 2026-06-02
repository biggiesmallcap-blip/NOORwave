[CmdletBinding()]
param(
    [string]$ChangeRef = "main..HEAD",
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Continue"
$emDash = [string][char]0x2014
$ellipsis = [string][char]0x2026

function Convert-ToAsciiText {
    param([string]$Text)

    $normalized = $Text.Replace($emDash, "-").Replace($ellipsis, "...")
    return [regex]::Replace($normalized, "[^\u0000-\u007F]", " ")
}

$repoRoot = (Resolve-Path -LiteralPath $Root).Path
$reportDir = Join-Path $repoRoot "docs\dev\repowise-review-gate"
New-Item -ItemType Directory -Path $reportDir -Force | Out-Null

$latestPath = Join-Path $reportDir "latest.md"
$timestamp = Get-Date -Format "yyyyMMdd-HHmmss"
$timestampPath = Join-Path $reportDir "$timestamp.md"
$sections = New-Object System.Collections.Generic.List[string]
$hadFailure = $false

function Add-CommandSection {
    param(
        [string]$Title,
        [string[]]$Command
    )

    $sections.Add("## $Title")
    $sections.Add("")
    $sections.Add('```powershell')
    $sections.Add(($Command -join " "))
    $sections.Add('```')
    $sections.Add("")

    Push-Location $repoRoot
    try {
        $exe = $Command[0]
        [string[]]$commandArgs = if ($Command.Count -gt 1) {
            @($Command[1..($Command.Count - 1)])
        } else {
            @()
        }
        $output = & $exe @commandArgs 2>&1
        $exitCode = $LASTEXITCODE
    } catch {
        $output = $_.Exception.Message
        $exitCode = 1
    } finally {
        Pop-Location
    }

    if ($exitCode -ne 0) {
        $script:hadFailure = $true
        $sections.Add("Advisory command exited with code $exitCode.")
        $sections.Add("")
    }

    $outputText = ($output | Out-String).TrimEnd()
    $sections.Add('```text')
    $sections.Add((Convert-ToAsciiText -Text $outputText))
    $sections.Add('```')
    $sections.Add("")
}

$sections.Add("# Repowise Advisory Review Gate")
$sections.Add("")
$sections.Add("- Change ref: ``$ChangeRef``")
$sections.Add("- Generated: $(Get-Date -Format o)")
$sections.Add("- Mode: advisory only; command failures are reported but do not fail this script.")
$sections.Add("")

Add-CommandSection -Title "Status" -Command @("repowise", "status")
Add-CommandSection -Title "Doctor" -Command @("repowise", "doctor")
Add-CommandSection -Title "Risk" -Command @("repowise", "risk", $ChangeRef)
Add-CommandSection -Title "Health Refactoring Targets" -Command @("repowise", "health", "--refactoring-targets")
Add-CommandSection -Title "Dead-Code Audit" -Command @("repowise", "dead-code", "--safe-only", "--format", "md")

if ($hadFailure) {
    $sections.Add("## Gate Result")
    $sections.Add("")
    $sections.Add("Advisory commands reported failures. Review the sections above before relying on this report.")
} else {
    $sections.Add("## Gate Result")
    $sections.Add("")
    $sections.Add("All advisory commands completed.")
}

Set-Content -LiteralPath $latestPath -Value $sections -Encoding UTF8
Copy-Item -LiteralPath $latestPath -Destination $timestampPath -Force

Write-Host "Repowise advisory report: $latestPath"
exit 0
