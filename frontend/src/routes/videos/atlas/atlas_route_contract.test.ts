import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, test } from 'vitest';

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(join(here, rel), 'utf8');
const page = read('+page.svelte');
const sectionLayout = read('../+layout.svelte');
const atlasDir = '../../../lib/video/atlas/';
const view = read(`${atlasDir}AtlasView.svelte`);
const sceneCard = read(`${atlasDir}AtlasSceneCard.svelte`);
const artistCard = read(`${atlasDir}AtlasArtistCard.svelte`);
const routeCard = read(`${atlasDir}AtlasRouteCard.svelte`);
const miniMap = read(`${atlasDir}AtlasMiniMap.svelte`);
const renderer = read(`${atlasDir}renderer.ts`);
const data = read(`${atlasDir}atlas_data.ts`);

describe('atlas route contract', () => {
	test('loads the map, polls while it is being built, and shows errors plainly', () => {
		expect(page).toContain('const POLL_MS = 10_000;');
		expect(page).toContain('await loadAtlas(true)');
		expect(page).toContain('if (building) timer = setTimeout(() => void load(), POLL_MS);');
		expect(page).toContain('The map is being drawn');
		expect(page).toContain('<ErrorState title="Could not load the Atlas"');
		expect(page).toContain('parseAtlasParams(page.url.searchParams)');
		expect(page).toContain('<AtlasView {loaded} {focus} rebuilding={building} />');
		expect(data).toContain('api.getVideoAtlas()');
		expect(data).toContain('api.getVideoAtlasNames()');
	});

	test('the section search field finds artists on the Atlas instead of searching TIDAL', () => {
		expect(sectionLayout).toContain("let onAtlas = $derived(page.url.pathname === '/videos/atlas');");
		expect(sectionLayout).toContain('let filtersInPlace = $derived(onLiked || onArchive || onAtlas);');
		expect(sectionLayout).toContain('let searchingTidal = $derived(query.length > 0 && !filtersInPlace);');
		// The field's aria-label is the placeholder, so the Atlas finds it by label.
		expect(sectionLayout).toContain(': onAtlas ? ATLAS_SEARCH_LABEL');
		expect(sectionLayout).toContain('ariaLabel={placeholder}');
		expect(view).toContain('$videoSectionQuery.trim()');
		expect(view).toContain("target.getAttribute('aria-label') === ATLAS_SEARCH_LABEL");
	});

	test('cards fetch through the contract calls with stale-response guards', () => {
		expect(view).toContain('api.getVideoAtlasScene(scene.id)');
		expect(view).toContain('if (seq === sceneSeq) sceneCard = result;');
		expect(view).toContain('api.getVideoAtlasArtist(id)');
		expect(view).toContain('if (seq === artistSeq) artistCard = result;');
		expect(view).toContain('api.probeVideoAtlasArtist(id)');
		expect(view).toContain('api.getVideoAtlasRoute(data.ids[routeFrom], data.ids[routeTo])');
		expect(view).toContain('if (seq !== routeSeq) return;');
	});

	test('keyboard: arrows pan, +/- zoom, F fits, R routes, Esc closes', () => {
		for (const key of ["case 'ArrowLeft':", "case 'ArrowUp':", "case '+':", "case '-':", "case 'f':", "case 'r':", "case 'Escape':"]) {
			expect(view).toContain(key);
		}
		expect(view).toContain("window.addEventListener('keydown', onKeydown, { capture: true });");
		expect(view).toContain("window.removeEventListener('keydown', onKeydown, { capture: true });");
		expect(view).toContain("canvasEl.addEventListener('wheel', onWheel, { passive: false });");
	});

	test('scene card hands off to On Air and the Archive and plays its sample', () => {
		expect(sceneCard).toContain('sceneChannelUrl(scene.id)');
		expect(sceneCard).toContain('{#if scene.channel_no != null}');
		expect(sceneCard).toContain('sceneArchiveUrl(scene.id)');
		expect(sceneCard).toContain('playAtlasQueue(video, card?.sample ?? [video], scene.name)');
		expect(sceneCard).toContain('Seen {formatCount(scene.seen_videos)} of {formatCount(scene.videos)}');
	});

	test('every artist and video reference carries the shared context menu', () => {
		for (const source of [sceneCard, artistCard, routeCard]) {
			expect(source).toContain('buildArtistMenu({ tidal_id:');
			expect(source).toContain('buildVideoMenu(video)');
		}
		expect(view).toContain('oncontextmenu={onContextMenu}');
		expect(view).toContain('buildArtistMenu({ tidal_id: id, name })');
	});

	test('artwork goes through the TIDAL-aware image with fallbacks', () => {
		for (const source of [sceneCard, artistCard, routeCard]) {
			expect(source).toContain('<ArtworkImage src={video.artwork_url} size={320}');
			expect(source).not.toContain('<img');
		}
	});

	test('route plays its stops in order; probes report honestly', () => {
		expect(routeCard).toContain('routeQueue(route.stops)');
		expect(routeCard).toContain('Play route');
		expect(routeCard).toContain('No path within six hops');
		expect(artistCard).toContain('Send a probe');
		expect(view).toContain('probeMessage(name, result.queued, result.reason)');
	});

	test('reduced motion turns off flights and pulses', () => {
		expect(view).toContain('reduceMotion.subscribe((reduce) => renderer?.setReducedMotion(reduce))');
		expect(renderer).toContain('if (this.reducedMotion) {');
		expect(renderer).toContain('if (this.mini || this.reducedMotion || this.pulseTimer || this.destroyed) return;');
	});

	test('the mini map deep-links into the Atlas with its trail', () => {
		expect(miniMap).toContain('atlasUrl({ artist: artistId, trail:');
		expect(miniMap).toContain("new AtlasRenderer(canvasEl, { mini: true })");
		expect(miniMap).toContain('artistId: number | null;');
	});

	test('runes only', () => {
		for (const source of [page, view, sceneCard, artistCard, routeCard, miniMap]) {
			expect(source).not.toContain('$:');
			expect(source).not.toContain('export let ');
		}
	});
});
