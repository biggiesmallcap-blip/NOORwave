# Handler-layer TIDAL auth recovery

About a dozen TIDAL call sites retry after a 401 by refreshing the session and rebuilding the client. We consolidated the refresh-and-rebuild into a shared `recover_tidal_client` helper (with a single-flight re-check that reuses an already-refreshed in-memory token), but kept the retry *decision* in each handler instead of making `TidalClient` transparently refresh-and-retry on 401.

We chose this because centralizing recovery into the client would touch every TIDAL surface, including the timing-sensitive streaming paths, and is too large for a bug-fix branch; the handler-layer helper closes the immediate gaps with low blast radius. Pushing recovery down into the client (so no handler writes a retry arm) is the correct end state and is deferred — see `FOLLOWUPS.md`.
