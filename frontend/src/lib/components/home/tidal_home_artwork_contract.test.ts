import { describe, expect, test } from 'vitest';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));

function source(path: string): string {
	return readFileSync(join(here, path), 'utf8');
}

const mixes = source('YourMixesShelf.svelte');
const radio = source('PersonalRadioShelf.svelte');

describe('TIDAL home artwork contracts', () => {
	test('routes mix artwork through ArtworkImage fallback handling', () => {
		expect(mixes).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(mixes).toContain('<ArtworkImage');
		expect(mixes).toContain('className="art"');
		expect(mixes).toContain('src={mix.image_url}');
		expect(mixes).toContain('size={320}');
		expect(mixes).toContain('fallbackText="MIX"');
		expect(mixes).toContain('decorative={true}');
		expect(mixes).toContain(':global(.art)');
		expect(mixes).not.toContain("style=\"background-image: url('{mix.image_url}')\"");
	});

	test('mix shelf instant-paints from the persisted query and revalidates safely', () => {
		// Seeds synchronously from the persisted snapshot (no skeleton when warm).
		expect(mixes).toContain('cachedApi.tidalMixesQuery()');
		expect(mixes).toContain('getSnapshot().data?.mixes');
		// The subscription is the sole writer of state (no manual loadSeq race guard;
		// the query cache de-dupes in-flight fetches).
		expect(mixes).toContain('mixesQuery.subscribe(');
		// A transient 503 keeps cached mixes; the connect prompt only shows with none.
		expect(mixes).toContain('s.error.status === 503');
		expect(mixes).toContain("if (mixes.length === 0) viewState = 'disconnected';");
		// The in-memory-only cache that was wiped on restart is gone.
		expect(mixes).not.toContain('tidal-mixes-cache');
		expect(mixes).not.toContain('putCachedMixes');
	});

	test('routes radio artwork through ArtworkImage fallback handling', () => {
		expect(radio).toContain("import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';");
		expect(radio).toContain('<ArtworkImage');
		expect(radio).toContain('className="art"');
		expect(radio).toContain('src={station.image_url}');
		expect(radio).toContain('size={320}');
		expect(radio).toContain('fallbackText="RAD"');
		expect(radio).toContain('decorative={true}');
		expect(radio).toContain(':global(.art)');
		expect(radio).not.toContain("style=\"background-image: url('{station.image_url}')\"");
	});

	test('radio shelf instant-paints from the persisted query and revalidates safely', () => {
		expect(radio).toContain('cachedApi.tidalRadioStationsQuery()');
		expect(radio).toContain('getSnapshot().data?.stations');
		expect(radio).toContain('stationsQuery.subscribe(');
		expect(radio).toContain('s.error.status === 503');
		expect(radio).toContain("if (stations.length === 0) viewState = 'disconnected';");
		expect(radio).not.toContain('tidal-radio-cache');
		expect(radio).not.toContain('putCachedRadioStations');
	});
});
