# TIDAL session owns tokens and auth recovery

Supersedes the deferral in ADR-0001. A `TidalSession` in `AppState` is now the only owner of the TIDAL tokens and their lifecycle (login, logout, persistence, refresh, needs-reconnect latch). Every `TidalClient` is a handle obtained from the session; on an auth failure the client's transport performs one single-flight refresh and retries once, so handlers and background jobs no longer write retry arms or build clients from raw tokens.

Decisions that a future change should not undo without reason:

- Auth failure is classified from the response status and subStatus, not by substring-matching error text. A 401 with subStatus 4005 ("asset not ready") is a playability answer, not an expired session, and must not trigger a refresh.
- A failed refresh latches the session into "needs reconnect": calls fail fast until the next login instead of every request re-hitting the refresh endpoint.
- Background jobs (video crawler, metadata repair, catalogue) get recovery too; the shared single-flight lock keeps them from racing foreground refreshes on a rotating refresh token.
- The streaming path (`services/tidal/stream.rs`: playbackinfo and manifests) keeps its explicit recovery for now because of its timing sensitivity. Moving it behind the session is the remaining step.
