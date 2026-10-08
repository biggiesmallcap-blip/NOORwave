import { existsSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const component = readFileSync(join(here, 'TidalEditorialPage.svelte'), 'utf8');
const routesRoot = join(here, '..', '..', '..', 'routes');

function routeSource(route: string): string {
	return readFileSync(join(routesRoot, route, '+page.svelte'), 'utf8');
}

describe('TIDAL editorial page routes', () => {
	test('loads modules through the whitelisted generic TIDAL page API', () => {
		expect(component).toContain('api.getTidalPage(requestedPagePath)');
		expect(component).toContain('TidalDiscoverShelves');
		expect(component).toContain('PageHeader');
		expect(component).toContain("viewState === 'disconnected'");
		expect(component).toContain('e instanceof ApiError && e.status === 503');
		expect(component).toContain('let loadSeq = 0;');
		expect(component).toContain('let pendingPagePath = $state<string | null>(null);');
		expect(component).toContain('let loadedPagePath = $state<string | null>(null);');
		expect(component).toContain('onDestroy(() => {');
		expect(component).toContain('void load(currentPagePath);');
		expect(component).toContain('const requestedPagePath = targetPagePath ?? pagePath;');
		expect(component).toContain('if (inFlight && pendingPagePath === requestedPagePath) return;');
		expect(component).toContain('const seq = ++loadSeq;');
		expect(component).toContain('if (seq !== loadSeq || requestedPagePath !== pagePath) return;');
		expect(component).toContain('if (seq === loadSeq) {');
		expect(component).toContain('pendingPagePath = null;');
		expect(component).toContain('onclick={() => void load()}');
		expect(component).not.toContain('$:');
	});

	test('wires non-colliding editorial pages to documented TIDAL paths', () => {
		// Explore was never linked and TIDAL returns no modules for it.
		expect(existsSync(join(routesRoot, 'explore', '+page.svelte'))).toBe(false);
		expect(routeSource('hires')).toContain('pagePath="hires"');
		expect(routeSource('new-releases')).toContain('pagePath="new-releases"');
	});

	test('does not replace existing genres or videos workflows', () => {
		expect(routeSource('genres')).toContain('GenreGalaxy');
		expect(routeSource('videos')).toContain('GuideRow');
		expect(routeSource('genres')).not.toContain('/tidal/genres');
		expect(routeSource('videos/editorial')).toContain('embedded');
	});

	test('wires colliding TIDAL editorial pages under the tidal namespace', () => {
		// TIDAL genres had no entry point besides the removed galaxy dock link.
		expect(existsSync(join(routesRoot, 'tidal', 'genres', '+page.svelte'))).toBe(false);
		expect(routeSource('videos/editorial')).toContain('pagePath="videos"');
		expect(routeSource('tidal/videos')).toContain("goto('/videos/editorial', { replaceState: true })");
	});
});
