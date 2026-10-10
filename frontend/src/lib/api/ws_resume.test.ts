import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('$lib/api/client', () => ({ getApiBase: () => 'http://127.0.0.1:17600', getStoredToken: () => null }));
vi.mock('$lib/stores/player', () => ({ refreshPlaybackState: vi.fn(), refreshPlaybackRuntime: vi.fn() }));
vi.mock('$lib/stores/tidal', () => ({ loadTidalStatus: vi.fn() }));
vi.mock('$lib/stores/training', () => ({}));
vi.mock('$lib/stores/audio_analysis', () => ({}));
vi.mock('$lib/components/DiscoverSpace/discover_space_store', () => ({}));
vi.mock('$lib/stores/exclusive_status', () => ({}));
vi.mock('$lib/stores/downloads', () => ({}));
vi.mock('$lib/stores/toast', () => ({ showToast: vi.fn() }));
vi.mock('$lib/stores/audioSpectrum', () => ({ setAudioSpectrum: vi.fn() }));
vi.mock('$lib/cache/ws_events', () => ({ applyCacheUpdateForWsMessage: vi.fn() }));

class FakeSocket {
	static CONNECTING = 0;
	static OPEN = 1;
	static instances: FakeSocket[] = [];
	readyState = FakeSocket.CONNECTING;
	onopen: (() => void) | null = null;
	onmessage: ((event: { data: string }) => void) | null = null;
	onclose: ((event: { code: number }) => void) | null = null;
	onerror: (() => void) | null = null;
	constructor(public url: string) { FakeSocket.instances.push(this); }
	send() {}
	close() {}
	open() { this.readyState = FakeSocket.OPEN; this.onopen?.(); }
	message(data: unknown) { this.onmessage?.({ data: JSON.stringify(data) }); }
}

let listeners: Record<string, () => void> = {};

beforeEach(() => {
	vi.useFakeTimers();
	vi.resetModules();
	FakeSocket.instances = [];
	listeners = {};
	const on = (type: string, handler: () => void) => { listeners[type] = handler; };
	vi.stubGlobal('WebSocket', FakeSocket);
	vi.stubGlobal('window', { addEventListener: on });
	vi.stubGlobal('document', { addEventListener: on, visibilityState: 'visible' });
});

afterEach(() => {
	vi.unstubAllGlobals();
	vi.useRealTimers();
});

async function connectAndWait(handshake: Record<string, unknown>) {
	const { connectWebSocket } = await import('./ws');
	connectWebSocket();
	const first = FakeSocket.instances[0];
	first.open();
	first.message(handshake);
	vi.setSystemTime(Date.now() + 60_000);
	listeners.visibilitychange();
	return first;
}

describe('WebSocket resume recovery', () => {
	it('replaces a socket that died before its first heartbeat', async () => {
		await connectAndWait({ type: 'connected', heartbeat_ms: 15000 });
		expect(FakeSocket.instances).toHaveLength(2);
	});

	it('keeps a quiet socket from a server that never promised heartbeats', async () => {
		await connectAndWait({ type: 'connected' });
		expect(FakeSocket.instances).toHaveLength(1);
	});
});
