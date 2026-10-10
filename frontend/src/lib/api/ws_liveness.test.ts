import { describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

vi.mock('$lib/api/client', () => ({ getApiBase: () => 'http://127.0.0.1:17600', getStoredToken: () => null }));
vi.mock('$lib/stores/player', () => ({ refreshPlaybackState: vi.fn(), refreshPlaybackRuntime: vi.fn() }));
vi.mock('$lib/stores/tidal', () => ({}));
vi.mock('$lib/stores/training', () => ({}));
vi.mock('$lib/stores/audio_analysis', () => ({}));
vi.mock('$lib/components/DiscoverSpace/discover_space_store', () => ({}));
vi.mock('$lib/stores/exclusive_status', () => ({}));
vi.mock('$lib/stores/downloads', () => ({}));
vi.mock('$lib/stores/toast', () => ({ showToast: vi.fn() }));
vi.mock('$lib/stores/audioSpectrum', () => ({ setAudioSpectrum: vi.fn() }));
vi.mock('$lib/cache/ws_events', () => ({ applyCacheUpdateForWsMessage: vi.fn() }));

const { socketIsStale, HEARTBEAT_STALE_MS } = await import('./ws');
const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), 'ws.ts'), 'utf8');
const wallpaper = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../components/wallpaper/ShaderWallpaper.svelte'), 'utf8');

describe('WebSocket liveness', () => {
	it('treats a heartbeat socket that went quiet as dead', () => {
		expect(socketIsStale(HEARTBEAT_STALE_MS + 1, 0, true)).toBe(true);
		expect(socketIsStale(HEARTBEAT_STALE_MS - 1, 0, true)).toBe(false);
	});

	it('never recycles a socket from a server that does not send heartbeats', () => {
		expect(socketIsStale(10 * HEARTBEAT_STALE_MS, 0, false)).toBe(false);
	});

	it('reconnects immediately when a suspended page comes back', () => {
		expect(source).toContain("document.addEventListener('visibilitychange', resume)");
		expect(source).toContain("window.addEventListener('pageshow', resume)");
		expect(source).toContain("window.addEventListener('online', resume)");
	});

	it('only streams visualiser frames to views that render them', () => {
		expect(source).toContain("type: 'spectrum', enabled: spectrumSubscribers > 0");
		expect(wallpaper).toContain('subscribeAudioSpectrum()');
		expect(wallpaper).toContain('releaseSpectrumStream()');
	});
});
