import { describe, expect, it } from 'vitest';
import { WALLPAPERS, wallpaperById } from './shaders';
import { VALID } from '$lib/stores/wallpaper';

describe('wallpaper shader registry', () => {
	it('lists every WALLPAPERS id in the store VALID set', () => {
		// Drift here is silent in the running app: a WALLPAPERS id missing from VALID
		// makes the store reject the user's saved choice and fall back to DEFAULT.
		const validSet = new Set<string>(VALID);
		for (const option of WALLPAPERS) {
			expect(validSet.has(option.id), `missing from VALID: ${option.id}`).toBe(true);
		}
	});

	it('has no VALID id that is absent from WALLPAPERS', () => {
		const known = new Set(WALLPAPERS.map((o) => o.id));
		for (const id of VALID) {
			expect(known.has(id), `VALID id has no WALLPAPERS entry: ${id}`).toBe(true);
		}
	});

	it('uses unique, kebab-case ids and resolves each one', () => {
		const seen = new Set<string>();
		for (const option of WALLPAPERS) {
			expect(seen.has(option.id), `duplicate id: ${option.id}`).toBe(false);
			seen.add(option.id);
			expect(option.id).toMatch(/^[a-z][a-z0-9-]*$/);
			expect(wallpaperById(option.id).id).toBe(option.id);
		}
	});

	it('gives every non-none wallpaper a shader body and labels', () => {
		for (const option of WALLPAPERS) {
			expect(option.label.length).toBeGreaterThan(0);
			expect(option.sublabel.length).toBeGreaterThan(0);
			if (option.id === 'none') {
				expect(option.shader).toBeNull();
			} else {
				expect(typeof option.shader).toBe('string');
				expect((option.shader ?? '').includes('void main')).toBe(true);
			}
		}
	});
});
