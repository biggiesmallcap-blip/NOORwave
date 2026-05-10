import { readFileSync } from 'node:fs';
import { describe, expect, test } from 'vitest';

describe('genre galaxy UI contract', () => {
	test('keeps summary compact and reserves dense detail for hover or selection', () => {
		const route = readFileSync('src/routes/genres/+page.svelte', 'utf8');
		const galaxy = readFileSync('src/lib/components/Genre/GenreGalaxy.svelte', 'utf8');

		expect(route).toContain('aria-label="Galaxy summary"');
		expect(route).toContain('class="hud-card-title"');
		expect(route).toContain('class="hud-meta-line"');
		expect(galaxy).not.toContain('drawArtistChips(ctx)');
		expect(galaxy).toContain('function drawNebulaVeins');
		expect(galaxy).toContain('drawNebulaVeins(ctx);');
		const drawFrameBody = galaxy.slice(galaxy.indexOf('function drawFrame()'));
		expect(drawFrameBody).not.toContain('drawNebulaVeins(ctx);');
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
});
