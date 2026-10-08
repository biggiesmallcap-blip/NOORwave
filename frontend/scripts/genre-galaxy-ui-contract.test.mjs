import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('genre galaxy UI contract', () => {
	test('keeps summary compact and reserves dense detail for hover or selection', () => {
		const route = readFileSync('src/routes/genres/+page.svelte', 'utf8');
		const galaxy = readFileSync('src/lib/components/Genre/GenreGalaxy.svelte', 'utf8');

		expect(route).toContain('cachedApi.getGenreGalaxySnapshot(90)');
		expect(route).toContain('aria-label="Galaxy summary"');
		expect(route).toContain('class="hud-card-title"');
		expect(route).toContain('class="hud-meta-line"');
		expect(galaxy).not.toContain('drawArtistChips(ctx)');
		// Nebula veins were removed as visual noise. The living-sky feel now
		// comes from camera-tracking parallax star layers, and node bodies blit
		// from cached sprites instead of allocating gradients per frame.
		expect(galaxy).not.toContain('drawNebulaVeins');
		expect(galaxy).toContain('function drawParallaxStars');
		expect(galaxy).toContain('function getNodeSprite');
		const drawFrameBody = galaxy.slice(galaxy.indexOf('function drawFrame()'));
		expect(drawFrameBody).toContain('drawParallaxStars(ctx);');
		expect(galaxy).toContain("import { labelAlpha, labelPriority, placeLabels, type LabelRect } from './galaxyLabels';");
		expect(galaxy).toContain('const accepted = placeLabels(candidates, { width, height });');
		expect(galaxy).not.toContain('function clampLabelRect');
		expect(galaxy).not.toContain('function labelAlphaForNode');
		expect(galaxy).not.toContain('Iowan Old Style');
		expect(galaxy).toContain('const HOVER_CARD_CURSOR_CLEARANCE_X = 28;');
		expect(galaxy).toContain('const HOVER_CARD_CURSOR_CLEARANCE_Y = 24;');
		expect(galaxy).toContain('function placeHoverCard(');
		expect(galaxy).toContain("hoverCardPosition.align === 'right' ? 'translate(-100%, -100%)' : 'translate(0, -100%)'");
		expect(galaxy).toContain('if (hoveredNodeId === node.id && !isDragging) continue;');
		expect(galaxy).not.toContain('selectedId === node.id || hoveredNodeId === node.id');
		expect(galaxy).toContain('class="hover-card"');
		expect(galaxy).toContain('Top:');
		expect(route).toContain('class="hud-stat"');
		const panel = readFileSync('src/lib/components/Genre/GenrePanel.svelte', 'utf8');
		// The panel opens under the view tabs, not on top of them.
		expect(panel).toContain('top: 84px;');
		expect(galaxy).not.toContain('class="mix-pill"');
		expect(galaxy).not.toContain('mixPillPosition');
		expect(galaxy).toContain('class="hover-hint"');
		expect(route).not.toContain('onMix={(id) => void handleMix(id)}');
	});

	test('heat and rediscover modes expose real playback actions', () => {
		const route = readFileSync('src/routes/genres/+page.svelte', 'utf8');

		// Mode actions live in the dock instead of a third floating bar.
		expect(route).toContain('class="dock-actions"');
		expect(route).not.toContain('mode-actions');
		expect(route).toContain('async function playRediscover');
		expect(route).toContain('async function playHottest');
		expect(route).toContain('async function saveHeatPlaylist');
		// Vibe mode is visual-only: no play action (it duplicated Start mix).
		expect(route).not.toContain('async function playVibe');
		expect(route).toContain("viewMode === 'heat' || viewMode === 'rediscover'");
		// Rediscover must scope to the SAME candidate rule the canvas highlights.
		expect(route).toContain('node.trackCount > 0 && node.listenCount === 0');
		expect(route).toContain('api.createPlaylistFromQueue(name, true)');
		// Core play (Start mix + heat/rediscover) plays LOCAL genre tracks,
		// shuffled and bounded - the whole point of the galaxy. Radio is opt-in.
		const playback = readFileSync('src/lib/components/Genre/genrePlayback.ts', 'utf8');
		expect(playback).toContain('export function sampleGenreQueue');
		expect(route).toContain("from '$lib/components/Genre/genrePlayback'");
		expect(route).toContain("playTracksInContext(ids, undefined, { shuffle: true })");
		expect(route).toContain('async function handleRadio');
		expect(route).toContain("startGenreRadio(seed, 'mixed', label)");
		const player = readFileSync('src/lib/stores/player.ts', 'utf8');
		expect(player).toContain('export async function startGenreRadio');
		expect(player).toContain('api.startRadioSong({ seed_track_id: seedTrackId, blend');
	});

	test('canvas colours come from the palette theme, not hard-coded navy', () => {
		const route = readFileSync('src/routes/genres/+page.svelte', 'utf8');
		const galaxy = readFileSync('src/lib/components/Genre/GenreGalaxy.svelte', 'utf8');

		expect(route).toContain('let galaxyTheme = $derived(buildGalaxyTheme($palette));');
		expect(route).toContain('theme={galaxyTheme}');
		expect(galaxy).toContain('fill.addColorStop(0, theme.sky[0]);');
		expect(galaxy).not.toContain('rgba(18, 20, 38, 0.99)');
		expect(galaxy).not.toContain("'#4a4d5e'");
		expect(galaxy).not.toContain("'#3a3d4e'");
		expect(galaxy).toContain('theme.starTints[star.tintIndex]');
		// The map is always night, so the whole route uses the dark token set.
		expect(route).toContain('data-theme="dark"');
		expect(route).toContain("applyPaletteTheme(routeEl, $palette, 'dark')");
		expect(route).not.toContain('rgba(8, 10, 18, 0.92)');
		expect(route).not.toContain('#0d0e15');
		expect(galaxy).not.toContain('rgba(10, 10, 18, 0.92)');
		expect(galaxy).not.toContain('rgba(13, 15, 24, 0.96)');
	});

	test('planets are flat matte discs', () => {
		const galaxy = readFileSync('src/lib/components/Genre/GenreGalaxy.svelte', 'utf8');

		expect(galaxy).not.toContain('BODY_GLOW_FACTOR');
		expect(galaxy).toContain('sctx.createLinearGradient(0, 0, 0, radius * 2)');
	});

	test('genres open as their own page and the galaxy restores focus on return', () => {
		const route = readFileSync('src/routes/genres/+page.svelte', 'utf8');
		const galaxy = readFileSync('src/lib/components/Genre/GenreGalaxy.svelte', 'utf8');
		const panel = readFileSync('src/lib/components/Genre/GenrePanel.svelte', 'utf8');

		expect(route).not.toContain('GenreInterior');
		expect(route).not.toContain('interiorOpen');
		expect(route).toContain('void goto(`/genres/${id}`);');
		expect(route).toContain("page.url.searchParams.get('focus')");
		expect(galaxy).toContain('onOpenGenre(node.id);');
		expect(panel).toContain('Open genre page');
		expect(panel).not.toContain('Open interior');
	});
});
