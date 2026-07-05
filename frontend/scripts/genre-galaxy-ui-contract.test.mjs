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
		expect(galaxy).toContain('const inActiveFamily = activeFamilyId !== null && node.familyId === activeFamilyId;');
		expect(galaxy).toContain('if (inActiveFamily && node.depth === 1) return 0.86;');
		expect(galaxy).toContain("if (inActiveFamily && node.depth === 2 && zoomLevel !== 'galaxy')");
		expect(galaxy).toContain('function labelUsesChip');
		expect(galaxy).toContain('function clampLabelRect');
		expect(galaxy).toContain('const HOVER_CARD_CURSOR_CLEARANCE_X = 28;');
		expect(galaxy).toContain('const HOVER_CARD_CURSOR_CLEARANCE_Y = 24;');
		expect(galaxy).toContain('function placeHoverCard(');
		expect(galaxy).toContain("hoverCardPosition.align === 'right' ? 'translate(-100%, -100%)' : 'translate(0, -100%)'");
		expect(galaxy).toContain('const labelActivity = activeFamilyLabel && labelUsesChip(node) ? Math.max(activity, 0.82) : activity;');
		expect(galaxy).toContain('if (hoveredNodeId === node.id && !isDragging) continue;');
		expect(galaxy).not.toContain('selectedId === node.id || hoveredNodeId === node.id');
		expect(galaxy).toContain('const { x: chipX, y: chipY } = clampLabelRect(');
		expect(galaxy).toContain('class="hover-card"');
		expect(galaxy).toContain('Top:');
	});

	test('heat and rediscover modes expose real playback actions', () => {
		const route = readFileSync('src/routes/genres/+page.svelte', 'utf8');

		expect(route).toContain('class="mode-actions glass-panel"');
		expect(route).toContain('async function playRediscover');
		expect(route).toContain('async function playHottest');
		expect(route).toContain('async function saveHeatPlaylist');
		// Vibe mode is visual-only: no play action (it duplicated Start mix).
		expect(route).not.toContain('async function playVibe');
		expect(route).toContain("viewMode === 'heat' || viewMode === 'rediscover'");
		// Rediscover must scope to the SAME candidate rule the canvas highlights.
		expect(route).toContain('node.trackCount > 0 && node.listenCount === 0');
		expect(route).toContain('api.createPlaylistFromQueue(name, true)');
		// Play actions must seed the real radio orchestrator (continuous, reasoned
		// station), NOT dump a static replacePlaybackQueue that loops the seed.
		expect(route).toContain('startGenreRadio');
		expect(route).toContain("startGenreRadio(seed, 'mixed'");
		expect(route).toContain("'adventurous', 'Rediscover'");
		expect(route).toContain("'familiar', 'Hottest'");
		const player = readFileSync('src/lib/stores/player.ts', 'utf8');
		expect(player).toContain('export async function startGenreRadio');
		expect(player).toContain('api.startRadioSong({ seed_track_id: seedTrackId, blend');
	});
});
