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
		// The row's play-button aria-label is a ternary that swaps between
		// "Now playing" and "Play queued track" depending on whether the row
		// is the current track. Assert both label literals appear in source
		// rather than requiring an exact static aria-label= attribute.
		expect(queue).toContain("'Now playing'");
		expect(queue).toContain("'Play queued track'");
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
		// The sheet rewrite collapsed per-row "Next" + "Queue" chips into a
		// single tap-to-play + circular "+" add-to-queue affordance. The
		// queue-now action moved to long-press via the shared track menu.
		expect(search).toContain('playTrackNow');
		expect(search).toContain('addTrackToQueue');
	});

	test('mini search supports library, TIDAL, and playlists segments', () => {
		const searchPath = resolve(root, 'src/lib/components/remote/RemoteMiniSearch.svelte');
		const search = read(searchPath);
		// Modes are now a named union so a fourth segment can be added without
		// editing this test. Verify the union covers all three current values
		// and the corresponding APIs are wired up.
		expect(search).toContain("type SearchMode = 'library' | 'tidal' | 'playlists'");
		expect(search).toContain('api.searchTidal');
		expect(search).toContain('playTidalTrackNow');
		expect(search).toContain('addTidalTrackToQueue');
		expect(search).toContain('api.getPlaylists');
		expect(search).toContain('/remote/playlists/');
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

	// Long-press → action sheet only works if the sheet is actually mounted
	// somewhere in the tree. The layout owns the single mount so every sub-page
	// inherits it; assert here so a future refactor doesn't silently drop it.
	test('remote layout hosts the shared action sheet and bridges', () => {
		const layoutSvelte = resolve(root, 'src/routes/remote/+layout.svelte');
		expect(existsSync(layoutSvelte)).toBe(true);
		const layout = read(layoutSvelte);
		expect(layout).toContain("RemoteActionSheet from '$lib/components/remote/RemoteActionSheet.svelte'");
		expect(layout).toContain('<RemoteActionSheet');
		expect(layout).toContain('installMediaSessionBridge');
		expect(layout).toContain('installWakeLock');
		expect(layout).toContain('installSilentMediaLoop');
	});

	test('remote layout opts out of SSR so browser-only APIs are safe', () => {
		const layoutTs = resolve(root, 'src/routes/remote/+layout.ts');
		expect(existsSync(layoutTs)).toBe(true);
		const layout = read(layoutTs);
		expect(layout).toMatch(/export const ssr = false/);
	});

	// Each sub-route renders inside RemotePageShell, which owns the back
	// button. Smoke-assert presence + no inline font styles (the rest of the
	// repo lint catches font styles project-wide; this is a cheap belt).
	const subRoutes = [
		'src/routes/remote/library/+page.svelte',
		'src/routes/remote/artists/[id]/+page.svelte',
		'src/routes/remote/albums/[id]/+page.svelte',
		'src/routes/remote/playlists/[id]/+page.svelte',
		'src/routes/remote/tidal/artists/[id]/+page.svelte',
		'src/routes/remote/tidal/albums/[id]/+page.svelte'
	];
	for (const rel of subRoutes) {
		test(`sub-route exists and uses RemotePageShell: ${rel}`, () => {
			const path = resolve(root, rel);
			expect(existsSync(path)).toBe(true);
			const src = read(path);
			// Library is a top-level browse and may render its own header
			// instead of the shell; the rest must use the shared shell.
			if (!rel.endsWith('library/+page.svelte')) {
				expect(src).toContain("RemotePageShell from '$lib/components/remote/RemotePageShell.svelte'");
				expect(src).toContain('<RemotePageShell');
			}
			expect(src).not.toMatch(/\sstyle="font-/);
		});
	}
});
