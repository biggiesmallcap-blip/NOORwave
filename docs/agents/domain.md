# Domain documentation

NOORwave uses one project context across the Rust server, Tauri shell and Svelte
frontend. Read `CLAUDE.md` and relevant existing `docs/` before exploring a domain.
Verify architecture statements against the current source before editing.

When present, also read root `CONTEXT.md` and relevant decisions in `docs/adr/`.
If a future `CONTEXT-MAP.md` introduces separate contexts, follow its links for
the affected surface. Proceed when these optional files are absent; create them
only when resolved domain terminology or decisions need a durable record.

Use established domain vocabulary. Surface conflicts with an existing
architectural decision rather than silently overriding it.
