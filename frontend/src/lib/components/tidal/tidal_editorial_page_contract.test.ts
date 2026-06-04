import { readFileSync } from 'node:fs';
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
		expect(component).toContain('api.getTidalPage(pagePath)');
		expect(component).toContain('TidalDiscoverShelves');
		expect(component).toContain('PageHeader');
		expect(component).toContain("viewState === 'disconnected'");
		expect(component).toContain('e instanceof ApiError && e.status === 503');
		expect(component).not.toContain('$:');
	});

	test('wires non-colliding editorial pages to documented TIDAL paths', () => {
		expect(routeSource('explore')).toContain('pagePath="explore"');
		expect(routeSource('hires')).toContain('pagePath="hires"');
		expect(routeSource('new-releases')).toContain('pagePath="new-releases"');
	});

	test('does not replace existing genres or videos workflows', () => {
		expect(routeSource('genres')).toContain('GenreGalaxy');
		expect(routeSource('videos')).toContain('VideoPlayer');
	});
});
