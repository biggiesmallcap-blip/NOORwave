<#
.SYNOPSIS
  Run a repowise wiki update with preflight + post-verify guards.

.DESCRIPTION
  Defends against the silent-stale regression where `repowise update` falls
  back to its built-in default model (llama3.2, not pulled here), 404s every
  page, yet still exits 0 and advances the sync pointer -- leaving a wiki that
  looks current but was never regenerated.

  Guards:
    1. Model + provider are read from .repowise/config.yaml (single source of
       truth) and passed to repowise explicitly, never left to env/defaults.
    2. Preflight verifies ollama is up and the generation model + embedder
       alias are actually pulled.
    3. Post-verify scans the run output and FAILS LOUDLY (non-zero exit, error
       marker) if any page generation errored, instead of trusting exit 0.

.EXAMPLE
  scripts\repowise-sync.ps1
  scripts\repowise-sync.ps1 -Since 5e9dc7ca -CascadeBudget 300 -Reindex
#>
[CmdletBinding()]
param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [string]$Since,
    [int]$CascadeBudget,
    [switch]$Reindex,
    [switch]$SkipPreflight
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot   = (Resolve-Path -LiteralPath $Root).Path
$repowise   = Join-Path $repoRoot ".repowise"
$configPath = Join-Path $repowise "config.yaml"
$errMark    = Join-Path $repowise ".update.error"
$runLog     = Join-Path $repowise ".update.run.log"

if (-not (Test-Path -LiteralPath $configPath)) {
    throw "No .repowise/config.yaml at $configPath. Run scripts\repowise-apply-local-config.ps1 first."
}

# --- single source of truth: model/provider/embedder from config.yaml ---
$cfg = Get-Content -LiteralPath $configPath
function Get-CfgValue([string]$key) {
    $line = $cfg | Where-Object { $_ -match "^\s*$key\s*:" } | Select-Object -First 1
    if ($line) { ($line -replace "^\s*$key\s*:\s*", "").Trim().Trim('"') } else { $null }
}
$provider = Get-CfgValue "provider"; if (-not $provider) { $provider = "ollama" }
$model    = Get-CfgValue "model";    if (-not $model)    { $model    = "qwen2.5-coder:7b" }
$embedder = Get-CfgValue "embedder"; if (-not $embedder) { $embedder = "openai" }
# The openai-compatible endpoint resolves this alias (ollama cp) to the real
# embedding model; preflight checks the alias is present.
$embedAlias = "text-embedding-3-small"

Write-Host "Repowise sync: provider=$provider model=$model embedder=$embedder"

function Fail([string]$msg) {
    $stamp = (Get-Date).ToString("s")
    Set-Content -LiteralPath $errMark -Value "[$stamp] repowise-sync FAILED: $msg" -Encoding UTF8
    Write-Error "repowise-sync FAILED: $msg"
    exit 1
}

function Invoke-Repowise([string[]]$rwArgs, [string]$captureLog) {
    # repowise logs progress to stderr. Under $ErrorActionPreference='Stop' a
    # 2>&1 merge makes PowerShell raise a terminating NativeCommandError when
    # the process exits (stderr was non-empty), aborting the script AFTER the
    # run but BEFORE post-verify -- a silent "looked like it failed" even on a
    # clean run. Drop to 'Continue' for the native call and key success off
    # $LASTEXITCODE, which carries the real exit code.
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    Push-Location -LiteralPath $repoRoot
    try {
        # Tee to the log, then Out-Null: without discarding the passthrough,
        # repowise's stdout would leak into the function's output stream and
        # $rc would become @("...stdout...", exitcode) instead of just the int.
        & repowise @rwArgs 2>&1 | Tee-Object -FilePath $captureLog | Out-Null
        return $LASTEXITCODE
    } finally {
        Pop-Location
        $ErrorActionPreference = $prev
    }
}

if (Test-Path -LiteralPath $errMark) { Remove-Item -LiteralPath $errMark -Force }

# --- preflight ---
if (-not $SkipPreflight) {
    $ollama = (Get-Command "ollama" -ErrorAction SilentlyContinue)
    $ollamaPath = if ($ollama) { $ollama.Source } else { Join-Path $env:LOCALAPPDATA "Programs\Ollama\ollama.exe" }
    if ($provider -eq "ollama") {
        if (-not (Test-Path -LiteralPath $ollamaPath)) { Fail "ollama executable not found ($ollamaPath)" }
        try {
            $null = Invoke-WebRequest -Uri "http://localhost:11434/api/tags" -UseBasicParsing -TimeoutSec 5
        } catch {
            Fail "ollama not reachable at localhost:11434 -- start it with: ollama serve"
        }
        $models = & $ollamaPath list
        $modelBase = ($model -split ":")[0]
        if (-not ($models -match [regex]::Escape($modelBase))) {
            Fail "generation model '$model' not pulled -- run: ollama pull $model"
        }
        if (-not ($models -match [regex]::Escape($embedAlias))) {
            Fail "embedder alias '$embedAlias' missing -- run scripts\repowise-apply-local-config.ps1 to re-create it"
        }
    }
    if (-not (Get-Command "repowise" -ErrorAction SilentlyContinue)) { Fail "repowise CLI not on PATH" }
    Write-Host "Preflight OK: ollama up, '$model' and '$embedAlias' present."
}

# --- run update with the model pinned explicitly ---
$updateArgs = @("update", "--provider", $provider, "--model", $model)
if ($Since)         { $updateArgs += @("--since", $Since) }
if ($CascadeBudget) { $updateArgs += @("--cascade-budget", "$CascadeBudget") }

Write-Host "Running: repowise $($updateArgs -join ' ')"
$rc = Invoke-Repowise $updateArgs $runLog

# --- post-verify: do not trust exit 0 ---
if ($rc -ne 0) { Fail "repowise update exited $rc (see $runLog)" }
$runText = if (Test-Path $runLog) { Get-Content -LiteralPath $runLog -Raw } else { "" }
if ($runText -match "(?i)page_generation_failed|not_found_error|model .* not found|error code: 404") {
    Fail "page generation errored (model/provider problem) -- see $runLog"
}

Write-Host "Update OK." -ForegroundColor Green

if ($Reindex) {
    Write-Host "Reindexing embeddings (embedder=$embedder)..."
    $rrc = Invoke-Repowise @("reindex", "--embedder", $embedder) (Join-Path $repowise ".reindex.run.log")
    if ($rrc -ne 0) { Fail "repowise reindex exited $rrc" }
    Write-Host "Reindex OK." -ForegroundColor Green
}

Write-Host "repowise-sync complete." -ForegroundColor Green
