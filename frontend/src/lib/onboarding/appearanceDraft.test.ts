import { afterEach, describe, expect, it, vi } from 'vitest';
import { changeAppearanceDraft, draftAppearance, persistAppearanceDraft, rememberAppearanceDraft, recoverAppearanceDraft } from './appearanceDraft';
vi.mock('$lib/api/client', () => ({ getApiBase: () => 'http://localhost:17600' }));
const original = { palette: 'futuro', theme: 'dark', background: 'standing-wave' } as const;
function storage(seed: Record<string, string> = {}) {
	const values = new Map(Object.entries(seed));
	return { values, getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); }, removeItem: (key: string) => { values.delete(key); } };
}
afterEach(() => vi.unstubAllGlobals());
describe('onboarding appearance drafts', () => {
	it('previews changes without writing saved appearance or mutating the original', () => {
		const saved = storage(); vi.stubGlobal('localStorage', saved);
		const first = draftAppearance(original);
		const next = changeAppearanceDraft(first, { palette: 'clay', theme: 'light' });
		expect(first.values).toEqual(original);
		expect(next.values).toEqual({ ...original, palette: 'clay', theme: 'light' });
		expect(saved.values.size).toBe(0);
	});
	it('commits only choices made explicitly and retains other saved keys', () => {
		const saved = storage({ 'noor-wallpaper': 'aurora', 'noor-theme': 'system' }); vi.stubGlobal('localStorage', saved);
		persistAppearanceDraft(changeAppearanceDraft(draftAppearance(original), { palette: 'clay' }));
		expect(Object.fromEntries(saved.values)).toEqual({ 'noor-palette': 'clay', 'noor-theme': 'system', 'noor-wallpaper': 'aurora' });
	});
	it('restores earlier writes after a partial failure and retains a recoverable draft', () => {
		const saved = storage({ 'noor-palette': 'iris', 'noor-theme': 'dark' });
		vi.stubGlobal('localStorage', { ...saved, setItem: (key: string, value: string) => {
			if (key === 'noor-theme' && value === 'light') throw new Error('Quota exceeded');
			saved.setItem(key, value);
		} });
		vi.stubGlobal('sessionStorage', storage());
		const draft = changeAppearanceDraft(draftAppearance(original), { palette: 'clay', theme: 'light' });
		rememberAppearanceDraft(draft);
		expect(() => persistAppearanceDraft(draft)).toThrow('retained for retry');
		expect(Object.fromEntries(saved.values)).toEqual({ 'noor-palette': 'iris', 'noor-theme': 'dark' });
		expect(recoverAppearanceDraft()).toEqual(draft);
	});
	it('requires recovery storage before completion with unsaved choices', () => {
		vi.stubGlobal('sessionStorage', undefined);
		expect(() => rememberAppearanceDraft(changeAppearanceDraft(draftAppearance(original), { theme: 'light' }))).toThrow('recovery storage');
		expect(() => rememberAppearanceDraft(draftAppearance(original))).not.toThrow();
	});
});
