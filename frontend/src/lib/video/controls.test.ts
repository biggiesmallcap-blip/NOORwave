import { afterEach, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { api, type TidalSearchVideo } from '$lib/api/client';
import { clearVideoSession, playVideo, videoSession } from '$lib/stores/video_session';
import { dispatchVideoControl, handleVideoShortcut, registerVideoControls } from './controls';

vi.mock('$lib/api/client', () => ({ api: {
	getTidalVideoStream: vi.fn().mockResolvedValue({ hls_url: 'https://example.test/next.m3u8', expires_at: null }),
	recordVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
	finishVideoHistory: vi.fn().mockResolvedValue({ ok: true }),
} }));

const queue = [1, 2].map(tidal_id => ({ tidal_id, title: `Video ${tidal_id}` } as TidalSearchVideo));
const cleanups: (() => void)[] = [];
const key = (value: string, patch = {}) => ({
	key: value, defaultPrevented: false, ctrlKey: false, metaKey: false, altKey: false, shiftKey: false,
	preventDefault: vi.fn(), ...patch,
}) as unknown as KeyboardEvent;

async function start(index = 0) {
	await playVideo(queue[index], { queue, source: 'mix', sourceLabel: 'Videos', autoplay: false },
		{ preloaded: { url: 'https://example.test/current.m3u8', expiresAt: null } });
}

afterEach(() => { cleanups.splice(0).forEach(cleanup => cleanup()); clearVideoSession(); vi.clearAllMocks(); });

test('music retains its controls when there is no active video session', () => {
	expect(dispatchVideoControl('next')).toBe(false);
	expect(handleVideoShortcut(key('MediaTrackNext'))).toBe(false);
	expect(api.getTidalVideoStream).not.toHaveBeenCalled();
});

test.each(['MediaTrackNext', 'ArrowRight'])('%s advances the video queue even with autoplay off', async value => {
	await start();
	const event = key(value, { shiftKey: value === 'ArrowRight' });
	expect(handleVideoShortcut(event)).toBe(true);
	expect(event.preventDefault).toHaveBeenCalledOnce();
	await vi.waitFor(() => expect(get(videoSession).loading).toBe(false));
	expect(get(videoSession).current?.tidal_id).toBe(2);
	expect(get(videoSession).autoplay).toBe(false);
});

test('previous media key returns to the previous video', async () => {
	await start(1);
	expect(handleVideoShortcut(key('MediaTrackPrevious'))).toBe(true);
	await vi.waitFor(() => expect(get(videoSession).loading).toBe(false));
	expect(get(videoSession).current?.tidal_id).toBe(1);
});

test('queue boundaries consume media keys without falling through to music', async () => {
	await start(1);
	expect(dispatchVideoControl('next')).toBe(true);
	expect(get(videoSession).current?.tidal_id).toBe(2);
	expect(api.getTidalVideoStream).not.toHaveBeenCalled();
});

test('a handled local shortcut cannot fire again in the global listener', async () => {
	await start();
	expect(handleVideoShortcut(key('ArrowRight', { shiftKey: true, defaultPrevented: true }))).toBe(false);
	expect(get(videoSession).current?.tidal_id).toBe(1);
});

test('player element controls route to the video and respect modified shortcuts', async () => {
	await start();
	const handler = vi.fn(() => true);
	cleanups.push(registerVideoControls(handler));
	for (const [value, action] of [[' ', 'toggle'], ['ArrowRight', 'seek-forward'], ['m', 'mute'], ['f', 'fullscreen'], ['q', 'queue']]) {
		expect(handleVideoShortcut(key(value))).toBe(true);
		expect(handler).toHaveBeenLastCalledWith(action);
	}
	handler.mockClear();
	expect(handleVideoShortcut(key('f', { ctrlKey: true }))).toBe(false);
	expect(handler).not.toHaveBeenCalled();
});

test('closing an old player cannot unregister its replacement', async () => {
	await start();
	const unregisterOld = registerVideoControls(() => true);
	const current = vi.fn(() => false);
	cleanups.push(registerVideoControls(current));
	unregisterOld();
	expect(handleVideoShortcut(key('q'))).toBe(false);
	expect(current).toHaveBeenCalledWith('queue');
});
