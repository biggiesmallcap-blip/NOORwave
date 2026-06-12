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

Hardened sync (guard against silent-stale):

```powershell
scripts\repowise-sync.ps1                              # update with preflight + post-verify
scripts\repowise-sync.ps1 -Since <ref> -CascadeBudget 300 -Reindex   # full catch-up
```

Use this instead of a bare `repowise update` for any manual catch-up. Why:
`repowise update` ignores `REPOWISE_MODEL` from `.repowise/.env` and falls back
to its built-in default model (`llama3.2`). That model isn't pulled here, so
every page 404s, yet `update` still **exits 0 and advances the sync pointer** -
the wiki looks current but was never regenerated. `repowise-sync.ps1` reads the
model from `config.yaml`, passes it explicitly, preflights that ollama + the
model + the embedder alias are present, and fails loudly (non-zero exit) if any
page generation errors.

The post-commit hook (`.githooks/post-commit`) applies the same three guards
automatically after each commit. When it detects a failure it writes a marker
file at `.repowise/.update.error` (cleared on the next successful run); if the
wiki ever looks stale, check for that file and read `.repowise/.update.run.log`.
The hook lives in `.githooks/` on purpose - this repo sets
`core.hooksPath=.githooks`, so a hook in `.git/hooks/` (where `repowise hook
install` writes) is silently ignored.

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
