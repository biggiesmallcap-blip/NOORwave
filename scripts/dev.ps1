# Boots noor-server (backend) and the SvelteKit dev server (frontend)
# in two separate terminal windows. Uses Windows Terminal if present,
# otherwise falls back to plain PowerShell windows.

$root = Split-Path -Parent $PSScriptRoot
$frontend = Join-Path $root 'frontend'

# A copied database can still authenticate to the real cloud account.
# Override deliberately before launching only when provider writes are wanted.
if (-not $env:NOOR_TIDAL_LIBRARY_WRITES) { $env:NOOR_TIDAL_LIBRARY_WRITES = 'deny' }

$backendCmd = 'cargo run -p noor-server'
$frontendCmd = 'pnpm dev'

$wt = Get-Command wt.exe -ErrorAction SilentlyContinue

if ($wt) {
    # Single Windows Terminal window, split into two panes.
    wt.exe new-tab --title 'noor-server' --startingDirectory $root powershell -NoExit -Command $backendCmd `
        ';' split-pane --title 'frontend' --startingDirectory $frontend powershell -NoExit -Command $frontendCmd
}
else {
    Start-Process powershell -ArgumentList '-NoExit', '-Command', "Set-Location '$root'; $backendCmd"
    Start-Process powershell -ArgumentList '-NoExit', '-Command', "Set-Location '$frontend'; $frontendCmd"
}
