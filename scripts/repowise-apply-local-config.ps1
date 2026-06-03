[CmdletBinding()]
param(
    [string]$Root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path,
    [string]$OllamaBaseUrl = "http://localhost:11434",
    [string]$OpenAiBaseUrl = "http://localhost:11434/v1",
    [string]$ChatModel = "qwen2.5-coder:7b",
    [string]$EmbeddingModel = "qwen3-embedding:0.6b",
    [string]$EmbeddingAlias = "text-embedding-3-small",
    [switch]$SkipOllamaModelSetup
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$repoRoot = (Resolve-Path -LiteralPath $Root).Path
$repowiseDir = Join-Path $repoRoot ".repowise"
New-Item -ItemType Directory -Path $repowiseDir -Force | Out-Null

$excludePatterns = @(
    ".repowise/**",
    ".scratch/**",
    "_tmp_*",
    "tmp/**",
    "temp/**",
    "target/**",
    "**/target/**",
    "frontend/.svelte-kit/**",
    "frontend/build/**",
    "frontend/coverage/**",
    "coverage/**",
    "node_modules/**",
    "**/node_modules/**",
    "promo/**",
    "**/screenshots/**",
    "**/videos/**",
    "**/playwright-report/**",
    "**/test-results/**",
    "*.db",
    "*.db-*",
    "*.sqlite",
    "*.sqlite3",
    "*.bak",
    "*.backup*",
    "*.log",
    "*.png",
    "*.jpg",
    "*.jpeg",
    "*.webp",
    "*.gif",
    "*.mp4",
    "*.webm",
    "*.mov",
    "*.avi"
)

$configLines = @(
    "provider: ollama",
    "model: $ChatModel",
    "embedder: openai",
    "reasoning: auto",
    "exclude_patterns:"
)

foreach ($pattern in $excludePatterns) {
    $escaped = $pattern.Replace('"', '\"')
    $configLines += "  - ""$escaped"""
}

Set-Content -LiteralPath (Join-Path $repowiseDir "config.yaml") -Value $configLines -Encoding UTF8

$envLines = @(
    "OLLAMA_BASE_URL=$OllamaBaseUrl",
    "OPENAI_API_KEY=ollama",
    "OPENAI_BASE_URL=$OpenAiBaseUrl",
    "REPOWISE_PROVIDER=ollama",
    "REPOWISE_MODEL=$ChatModel",
    "REPOWISE_EMBEDDER=openai"
)

Set-Content -LiteralPath (Join-Path $repowiseDir ".env") -Value $envLines -Encoding UTF8

if (-not $SkipOllamaModelSetup) {
    $ollamaCommand = Get-Command "ollama" -ErrorAction SilentlyContinue
    $ollamaPath = if ($ollamaCommand) {
        $ollamaCommand.Source
    } else {
        Join-Path $env:LOCALAPPDATA "Programs\Ollama\ollama.exe"
    }

    if (-not (Test-Path -LiteralPath $ollamaPath)) {
        Write-Warning "Ollama executable was not found. Config was written, but model setup was skipped."
    } else {
        $models = & $ollamaPath list
        if ($LASTEXITCODE -ne 0) {
            throw "ollama list failed with exit code $LASTEXITCODE"
        }

        if (-not ($models -match [regex]::Escape($EmbeddingModel))) {
            & $ollamaPath pull $EmbeddingModel
            if ($LASTEXITCODE -ne 0) {
                throw "ollama pull $EmbeddingModel failed with exit code $LASTEXITCODE"
            }
        }

        & $ollamaPath cp $EmbeddingModel $EmbeddingAlias
        if ($LASTEXITCODE -ne 0) {
            throw "ollama cp $EmbeddingModel $EmbeddingAlias failed with exit code $LASTEXITCODE"
        }
    }
}

Write-Host "Repowise local config written to $repowiseDir"
Write-Host "Embedding requests for $EmbeddingAlias will resolve through $OpenAiBaseUrl"
