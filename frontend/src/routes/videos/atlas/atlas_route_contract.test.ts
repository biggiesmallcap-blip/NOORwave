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
const journeyCard = read(`${atlasDir}AtlasJourneyCard.svelte`);
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
		expect(view).toContain('await api.getVideoAtlasJourney({');
		expect(view).toContain('if (seq !== journeySeq) return;');
	});

	test('keyboard: arrows pan, +/- zoom, F fits, J opens Journeys, Esc closes', () => {
		for (const key of ["case 'ArrowLeft':", "case 'ArrowUp':", "case '+':", "case '-':", "case 'f':", "case 'j':", "case 'Escape':"]) {
			expect(view).toContain(key);
		}
		expect(view).toContain("window.addEventListener('keydown', onKeydown, { capture: true });");
		expect(view).toContain("window.removeEventListener('keydown', onKeydown, { capture: true });");
		expect(view).toContain("canvasEl.addEventListener('wheel', onWheel, { passive: false });");
	});

	test('scene card hands off to On Air and the Archive and plays its sample', () => {
		// Tune in plays right from the card: no stop on the dial first.
		expect(sceneCard).toContain('await tuneChannel({ id: `scene:${scene.id}`, title: scene.name });');
		expect(sceneCard).toContain('{#if scene.channel_no != null}');
		expect(sceneCard).toContain('sceneArchiveUrl(scene.id)');
		expect(sceneCard).toContain('playAtlasQueue(video, card?.sample ?? [video], scene.name)');
		expect(sceneCard).toContain('{sceneSeenLine(scene.seen_videos, scene.lit_artists)}');
	});

	test('every artist and video reference carries the shared context menu', () => {
		for (const source of [sceneCard, artistCard, journeyCard]) {
			expect(source).toContain('buildArtistMenu({ tidal_id:');
			expect(source).toContain('buildVideoMenu(video)');
		}
		expect(view).toContain('oncontextmenu={onContextMenu}');
		expect(view).toContain('buildArtistMenu({ tidal_id: id, name })');
	});

	test('artwork goes through the TIDAL-aware image with fallbacks', () => {
		for (const source of [sceneCard, artistCard, journeyCard]) {
			expect(source).toContain('<ArtworkImage src={video.artwork_url} size={320}');
			expect(source).not.toContain('<img');
		}
	});

	test('a journey plays its stops in order and the map follows it; probes report honestly', () => {
		expect(journeyCard).toContain('journeyQueue(journey.stops)');
		expect(journeyCard).toContain('Play journey');
		expect(journeyCard).toContain('Somewhere new');
		// Collabs are shown on the artist card (and drawn on the map), not chained.
		expect(journeyCard).not.toContain('Collab chain');
		expect(artistCard).toContain('aria-label="Collabs"');
		expect(view).toContain('if (session.sourceLabel !== journeyPlaying.label) {');
		expect(view).toContain('renderer?.flyToArtist(index, compact ? 6 : 7, ox, oy);');
		expect(artistCard).toContain('Send a probe');
		expect(view).toContain('probeMessage(name, result.queued, result.reason)');
	});

	test('reduced motion turns off flights and pulses', () => {
		expect(view).toContain('reduceMotion.subscribe((reduce) => renderer?.setReducedMotion(reduce))');
		expect(renderer).toContain('if (this.reducedMotion) {');
		expect(renderer).toContain('if (this.mini || this.reducedMotion || this.pulseTimer || this.destroyed) return;');
	});

	test('the looks load their own data, Picture and You', () => {
		expect(data).toContain('api.getVideoAtlasPictures()');
		expect(data).toContain('api.getVideoAtlasYou()');
		expect(view).toContain('renderer?.setPictures(next);');
		expect(view).toContain('renderer?.setYou(next);');
		expect(renderer).toContain("if (this.look === 'picture' && !this.mini && k >= THUMB_K)");
	});

	test('the mini map deep-links into the Atlas with its trail', () => {
		expect(miniMap).toContain('atlasUrl({ artist: artistId, trail:');
		expect(miniMap).toContain("new AtlasRenderer(canvasEl, { mini: true })");
		expect(miniMap).toContain('artistId: number | null;');
	});

	test('runes only', () => {
		for (const source of [page, view, sceneCard, artistCard, journeyCard, miniMap]) {
			expect(source).not.toContain('$:');
			expect(source).not.toContain('export let ');
		}
	});
});
