import { describe, expect, test } from 'vitest';
import type { Genre } from '$lib/api/client';
import { buildGalaxyData } from './galaxyBuilder';
import { ROOT_FAMILY_COLORS } from './galaxy.types';

function genre(id: number, name: string, slug: string, parentId: number | null, children: Genre[] = [], trackCount = 10): Genre {
	return { id, name, slug, parent_id: parentId, children, track_count: trackCount };
}

const taxonomy: Genre[] = [
	genre(1, 'Electronic', 'electronic', null, [
		genre(2, 'House', 'house', 1),
		genre(3, 'Techno', 'techno', 1),
		genre(4, 'Ambient Techno', 'ambient-techno', 1)
	]),
	genre(10, 'Rock', 'rock', null, [genre(11, 'Indie', 'indie', 10)]),
	genre(20, 'Jazz', 'jazz', null, [genre(21, 'Bebop', 'bebop', 20)])
];

describe('buildGalaxyData', () => {
	test('roots keep the family colour, sub-genres get distinct shades', () => {
		const { nodes } = buildGalaxyData(taxonomy, []);
		const byId = new Map(nodes.map((node) => [node.id, node]));
		expect(byId.get(1)?.color).toBe(ROOT_FAMILY_COLORS.electronic.color);
		const childColors = [2, 3, 4].map((id) => byId.get(id)?.color);
		expect(childColors).not.toContain(ROOT_FAMILY_COLORS.electronic.color);
		expect(new Set(childColors).size).toBe(3);
	});
});
