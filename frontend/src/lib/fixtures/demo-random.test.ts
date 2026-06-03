import { describe, expect, test } from 'vitest';
import { dateNDaysAgo, gaussian, mulberry32 } from './demo-random';
import { generateDemoKpis } from './demo-kpis';
import { generateDemoRidgeline } from './demo-ridgeline';
import { generateDemoSonicField } from './demo-sonic-field';
import { generateDemoTempo } from './demo-tempo';

function todayMinus(days: number): string {
	const d = new Date();
	d.setHours(0, 0, 0, 0);
	d.setDate(d.getDate() - days);
	return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

describe('demo random fixture helpers', () => {
	test('keeps seeded random output repeatable', () => {
		const first = mulberry32(42);
		const second = mulberry32(42);
		const different = mulberry32(43);

		expect([first(), first(), first()]).toEqual([second(), second(), second()]);
		expect(mulberry32(42)()).not.toBe(different());
	});

	test('generates gaussian values from an injected random source', () => {
		const rand = mulberry32(42);
		expect(gaussian(rand, 10, 2)).toBeCloseTo(7.4303236847419605, 12);
	});

	test('formats local ISO dates at midnight', () => {
		expect(dateNDaysAgo(0)).toBe(todayMinus(0));
		expect(dateNDaysAgo(7)).toBe(todayMinus(7));
	});

	test('keeps analytics demo generators deterministic for the same seed', () => {
		expect(generateDemoTempo('house-techno', 7, 7)).toEqual(generateDemoTempo('house-techno', 7, 7));
		expect(generateDemoSonicField('club', 7)).toEqual(generateDemoSonicField('club', 7));
		expect(generateDemoRidgeline('routine', 7, 7)).toEqual(generateDemoRidgeline('routine', 7, 7));
		expect(generateDemoKpis('casual', 7, 7)).toEqual(generateDemoKpis('casual', 7, 7));
	});
});
