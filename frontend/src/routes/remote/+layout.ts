// /remote pages hit the API at module init time (via stores + $effects).
// Skip SSR so SvelteKit doesn't try to call browser-only APIs (localStorage,
// MediaSession, WakeLock, WebSocket) during prerender.
export const ssr = false;
export const prerender = false;
