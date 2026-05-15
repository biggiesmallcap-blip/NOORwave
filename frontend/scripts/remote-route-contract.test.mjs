import { describe, expect, test } from 'vitest';
import { readFileSync, existsSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, '..');
const routePath = resolve(root, 'src/routes/remote/+page.svelte');
const layoutPath = resolve(root, 'src/routes/+layout.svelte');

function read(path) {
	return readFileSync(path, 'utf8');
}

describe('remote route contract', () => {
	test('has a dedicated route file', () => {
		expect(existsSync(routePath)).toBe(true);
	});

	test('root layout exposes stripped chrome for remote', () => {
		const layout = read(layoutPath);
		expect(layout).toContain("page.url.pathname.startsWith('/remote')");
		expect(layout).toContain('remote-shell');
	});

	test('remote page does not use inline style attributes', () => {
		const page = read(routePath);
		expect(page).not.toMatch(/\sstyle=/);
	});

	test('remote imports the dedicated transport component', () => {
		const page = read(routePath);
		expect(page).toContain("RemoteTransport from '$lib/components/remote/RemoteTransport.svelte'");
		expect(page).toContain('<RemoteTransport');
	});

	test('transport exposes expected playback controls', () => {
		const transportPath = resolve(root, 'src/lib/components/remote/RemoteTransport.svelte');
		const transport = read(transportPath);
		for (const label of ['Previous', 'Play or pause', 'Next', 'Seek playback', 'Volume']) {
			expect(transport).toContain(`aria-label="${label}"`);
		}
	});

	test('remote imports the dedicated queue component', () => {
		const page = read(routePath);
		expect(page).toContain("RemoteQueue from '$lib/components/remote/RemoteQueue.svelte'");
		expect(page).toContain('<RemoteQueue');
	});

	test('queue component supports play and remove actions', () => {
		const queuePath = resolve(root, 'src/lib/components/remote/RemoteQueue.svelte');
		const queue = read(queuePath);
		expect(queue).toContain('playTrackNow');
		expect(queue).toContain('removeTrackFromQueue');
		expect(queue).toContain('aria-label="Play queued track"');
		expect(queue).toContain('aria-label="Remove from queue"');
	});

	test('remote imports the mini search component', () => {
		const page = read(routePath);
		expect(page).toContain("RemoteMiniSearch from '$lib/components/remote/RemoteMiniSearch.svelte'");
		expect(page).toContain('<RemoteMiniSearch');
	});

	test('mini search uses local search and queue actions', () => {
		const searchPath = resolve(root, 'src/lib/components/remote/RemoteMiniSearch.svelte');
		const search = read(searchPath);
		expect(search).toContain('api.search');
		expect(search).toContain('playTrackNow');
		expect(search).toContain('playTrackNext');
		expect(search).toContain('addTrackToQueue');
	});

	test('mini search can query TIDAL when the TIDAL segment is selected', () => {
		const searchPath = resolve(root, 'src/lib/components/remote/RemoteMiniSearch.svelte');
		const search = read(searchPath);
		expect(search).toContain("let mode = $state<'library' | 'tidal'>('library')");
		expect(search).toContain('api.searchTidal');
		expect(search).toContain('playTidalTrackNow');
		expect(search).toContain('playTidalTrackNext');
		expect(search).toContain('addTidalTrackToQueue');
	});

	test('manifest launches the remote route in standalone mode', () => {
		const manifest = JSON.parse(read(resolve(root, 'static/manifest.webmanifest')));
		expect(manifest.display).toBe('standalone');
		expect(manifest.start_url).toBe('/remote');
		expect(manifest.scope).toBe('/');
		expect(manifest.icons.some((icon) => icon.sizes === '192x192')).toBe(true);
	});

	test('manifest ships a 512px install icon present on disk', () => {
		const manifest = JSON.parse(read(resolve(root, 'static/manifest.webmanifest')));
		const icon512 = manifest.icons.find((icon) => icon.sizes === '512x512');
		expect(icon512).toBeDefined();
		expect(existsSync(resolve(root, 'static', icon512.src.replace(/^\//, '')))).toBe(true);
	});

	test('service worker caches only app shell assets', () => {
		const workerPath = resolve(root, 'src/service-worker.ts');
		const worker = read(workerPath);
		expect(worker).toContain("import { build, files, version } from '$service-worker'");
		expect(worker).toContain('event.request.method !==');
		expect(worker).not.toContain('/api/');
	});
});
