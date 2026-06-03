# RepoWise with Ollama on Windows

This repo's local RepoWise setup uses Ollama at `http://localhost:11434` with:

- Generation model: `qwen2.5-coder:7b`
- Embedding model: `qwen3-embedding:0.6b`
- OpenAI-compatible embedding alias: `text-embedding-3-small`

RepoWise reads provider settings from `.repowise/config.yaml` and local
environment values from `.repowise/.env`. Both files are ignored by git. The
tracked source of truth for rebuilding them is
`scripts/repowise-apply-local-config.ps1`.

Apply the local config:

```powershell
scripts\repowise-apply-local-config.ps1
```

That script writes:

- `provider: ollama`
- `model: qwen2.5-coder:7b`
- `embedder: openai`
- `OPENAI_BASE_URL=http://localhost:11434/v1`
- `OPENAI_API_KEY=ollama`

RepoWise `reindex` currently supports OpenAI and Gemini embedders directly.
Ollama exposes an OpenAI-compatible embedding endpoint, so the script aliases
`qwen3-embedding:0.6b` to `text-embedding-3-small` with `ollama cp`.

PowerShell startup:

```powershell
& "$env:LOCALAPPDATA\Programs\Ollama\ollama.exe" serve
```

In another PowerShell window:

```powershell
cd <repo-root>
repowise reindex --embedder openai
repowise serve
```

Quick checks:

```powershell
& "$env:LOCALAPPDATA\Programs\Ollama\ollama.exe" list
repowise status
repowise doctor
repowise search --mode semantic --limit 5 "playback queue"
```

The local config excludes noise that hurts RepoWise signal:

- `.repowise/` local database and vector state
- local DB files, WAL files, logs, backups, and SQLite snapshots
- generated promo output under `promo/`
- scratch work and nested temporary repos under `.scratch/` and `_tmp_*`
- screenshots, videos, Playwright reports, and test result artifacts
- `target/`, frontend build output, coverage output, and `node_modules/`

Coverage and health:

```powershell
scripts\repowise-health.ps1
```

Use existing LCOV files without regenerating coverage:

```powershell
scripts\repowise-health.ps1 -UseExistingCoverage
```

The Rust LCOV path is `target\llvm-cov\noorwave.lcov`. The frontend LCOV path
is `frontend\coverage\lcov.info`.

Advisory review gate:

```powershell
scripts\repowise-review-gate.ps1 main..HEAD
```

The review gate records status, doctor, risk, health refactoring targets, and
dead-code audit output under `docs/dev/repowise-review-gate/`. It is advisory
only and exits successfully even when a RepoWise command reports a concern.
