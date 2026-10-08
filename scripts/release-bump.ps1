<#
.SYNOPSIS
Prepares a NOORwave release commit on master: version bump, minimal Cargo.lock edit, notes check.
.DESCRIPTION
First run with a new tag scaffolds docs/releases/vX.Y.Z.md from the commits since the last tag
and stops so the notes can be rewritten for listeners. The second run bumps the five release
lines and commits. -Ship also pushes master, runs release-preflight.ps1 and pushes the tag.
Cargo is never invoked: `cargo update` re-resolves transitive deps and has broken releases before.
.EXAMPLE
scripts\release-bump.ps1 -Tag v0.19.48
scripts\release-bump.ps1 -Tag v0.19.48 -Ship
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^v\d+\.\d+\.\d+$')]
    [string]$Tag,
    [switch]$Ship
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

    $utf8 = New-Object System.Text.UTF8Encoding $false
    function Set-FirstMatch([string]$Path, [string]$Pattern, [string]$Replacement) {
        $full = Join-Path $repoRoot $Path
        $content = [System.IO.File]::ReadAllText($full)
        $regex = [regex]::new($Pattern)
        if (-not $regex.IsMatch($content)) { throw "No version line found in $Path" }
        [System.IO.File]::WriteAllText($full, $regex.Replace($content, $Replacement, 1), $utf8)
    }

    $version = $Tag.Substring(1)
    $branch = (Invoke-Checked git @('rev-parse', '--abbrev-ref', 'HEAD') | Select-Object -First 1).Trim()
    if ($branch -ne 'master') { throw "Run on master (currently on $branch)." }
    $dirtyTracked = @(Invoke-Checked git @('status', '--porcelain', '--untracked-files=no'))
    if ($dirtyTracked.Count -gt 0) { throw 'Tracked files are dirty. Commit or stash them first.' }
    $null = Invoke-Checked git @('fetch', '--quiet', 'origin', 'master')
    $null = Invoke-Checked git @('merge', '--ff-only', '--quiet', 'origin/master')
    $existing = @(Invoke-Checked git @('tag', '--list', $Tag)) + @(Invoke-Checked git @('ls-remote', '--tags', 'origin', "refs/tags/$Tag"))
    if ($existing.Count -gt 0) { throw "Tag $Tag already exists." }

    $notesPath = "docs/releases/$Tag.md"
    $heading = "## What's new in $Tag"
    if (-not (Test-Path -LiteralPath $notesPath)) {
        $lastTag = (Invoke-Checked git @('describe', '--tags', '--abbrev=0') | Select-Object -First 1).Trim()
        $subjects = @(Invoke-Checked git @('log', '--no-merges', '--format=- %s', "$lastTag..HEAD"))
        $scaffold = @(
            $heading, '',
            '### Area', '',
            '- TODO: rewrite as listener-facing changes, then delete the commit list below.', '',
            "<!-- commits since $lastTag -->"
        ) + $subjects
        [System.IO.File]::WriteAllText((Join-Path $repoRoot $notesPath), (($scaffold -join "`n") + "`n"), $utf8)
        Write-Host "Scaffolded $notesPath from $($subjects.Count) commits since $lastTag. Edit it, then rerun."
        exit 1
    }
    $notes = (Get-Content -Raw -LiteralPath $notesPath).Trim()
    if (-not $notes.StartsWith($heading) -or [string]::IsNullOrWhiteSpace($notes.Substring($heading.Length))) {
        throw "$notesPath must start with '$heading' and contain release details."
    }
    if ($notes -match 'TODO|<!-- commits since') { throw "$notesPath still has scaffold text." }

    Set-FirstMatch 'noor-server/Cargo.toml' '(?m)^version = "[^"]+"' "version = `"$version`""
    Set-FirstMatch 'noor-app/Cargo.toml' '(?m)^version = "[^"]+"' "version = `"$version`""
    Set-FirstMatch 'noor-app/tauri.conf.json' '"version":\s*"[^"]+"' "`"version`": `"$version`""
    foreach ($package in @('noor-app', 'noor-server')) {
        $pattern = '(?m)(^name = "' + [regex]::Escape($package) + '"\r?\nversion = )"[^"]+"'
        Set-FirstMatch 'Cargo.lock' $pattern "`${1}`"$version`""
    }

    $lockStat = (Invoke-Checked git @('diff', '--numstat', '--', 'Cargo.lock') | Select-Object -First 1)
    if ($lockStat -notmatch '^2\s+2\s') { throw "Cargo.lock diff should be exactly 2 lines, got: $lockStat" }
    $null = Invoke-Checked git @('diff', '--check')

    $releaseFiles = @('noor-server/Cargo.toml', 'noor-app/Cargo.toml', 'noor-app/tauri.conf.json', 'Cargo.lock', $notesPath)
    $null = Invoke-Checked git (@('add', '--') + $releaseFiles)
    $null = Invoke-Checked git @('commit', '--quiet', '-m', "chore(release): prepare $Tag")
    Write-Host "Committed release preparation for $Tag."

    if ($Ship) {
        $null = Invoke-Checked git @('push', '--quiet', 'origin', 'master')
        & (Join-Path $PSScriptRoot 'release-preflight.ps1') -Tag $Tag
        $null = Invoke-Checked git @('tag', '-m', $Tag, $Tag)
        $null = Invoke-Checked git @('push', '--quiet', 'origin', $Tag)
        Write-Host "Pushed $Tag. Watch: gh run watch `$(gh run list --workflow Release --limit 1 --json databaseId --jq '.[0].databaseId')"
    } else {
        Write-Host "Next: git push origin master; scripts\release-preflight.ps1 -Tag $Tag; git tag -m $Tag $Tag; git push origin $Tag"
    }
}
finally {
    Pop-Location
}
