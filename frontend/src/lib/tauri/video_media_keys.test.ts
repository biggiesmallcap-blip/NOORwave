import { afterEach, expect, test, vi } from 'vitest';
import { writable } from 'svelte/store';

const mocks = vi.hoisted(() => ({ invoke: vi.fn().mockResolvedValue(undefined), listen: vi.fn(), dispatch: vi.fn() }));
const session = writable({ active: false });
vi.mock('$lib/stores/video_session', () => ({ videoSession: { subscribe: (fn: (value: { active: boolean }) => void) => session.subscribe(fn) } }));
vi.mock('$lib/video/controls', () => ({ dispatchVideoControl: mocks.dispatch }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mocks.invoke }));
vi.mock('@tauri-apps/api/webviewWindow', () => ({ getCurrentWebviewWindow: () => ({ listen: mocks.listen }) }));
import { installVideoMediaKeys } from './video_media_keys';

afterEach(() => { session.set({ active: false }); vi.clearAllMocks(); vi.unstubAllGlobals(); });

test('native ownership follows the active session and routes hardware controls', async () => {
	vi.stubGlobal('window', { __TAURI_INTERNALS__: { invoke: true } });
	const unlisten = vi.fn();
	mocks.listen.mockResolvedValueOnce(unlisten);
	const cleanup = installVideoMediaKeys();
	await vi.waitFor(() => expect(mocks.invoke).toHaveBeenCalledWith('set_video_media_active', { active: false }));
	session.set({ active: true });
	await vi.waitFor(() => expect(mocks.invoke).toHaveBeenLastCalledWith('set_video_media_active', { active: true }));
	// Playback updates while loading must not give the music queue ownership.
	session.set({ active: true });
	expect(mocks.invoke).toHaveBeenCalledTimes(2);
	const listener = mocks.listen.mock.calls[0][1];
	listener({ payload: 'next' });
	expect(mocks.dispatch).toHaveBeenCalledWith('next');
	cleanup();
	await vi.waitFor(() => expect(mocks.invoke).toHaveBeenLastCalledWith('set_video_media_active', { active: false }));
	expect(unlisten).toHaveBeenCalledOnce();
	listener({ payload: 'previous' });
	expect(mocks.dispatch).toHaveBeenCalledTimes(1);
});

test('teardown during listener setup never claims native keys', async () => {
	vi.stubGlobal('window', { __TAURI_INTERNALS__: { invoke: true } });
	let ready!: (cleanup: () => void) => void;
	mocks.listen.mockImplementationOnce(() => new Promise(resolve => { ready = resolve; }));
	const cleanup = installVideoMediaKeys();
	await vi.waitFor(() => expect(mocks.listen).toHaveBeenCalled());
	cleanup();
	const unlisten = vi.fn();
	ready(unlisten);
	await vi.waitFor(() => expect(unlisten).toHaveBeenCalledOnce());
	expect(mocks.invoke).not.toHaveBeenCalled();
});

test('web playback does not install a native bridge', () => {
	vi.stubGlobal('window', {});
	installVideoMediaKeys()();
	expect(mocks.listen).not.toHaveBeenCalled();
	expect(mocks.invoke).not.toHaveBeenCalled();
});
