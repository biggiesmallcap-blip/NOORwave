import { PALETTES } from '$lib/components/wallpaper/palettes';
import { WALLPAPERS } from '$lib/components/wallpaper/shaders';
import type { AppearanceValues } from '$lib/components/settings/AppearanceFields.svelte';
import { getApiBase } from '$lib/api/client';

export type AppearanceDraft = { values: AppearanceValues; changes: Partial<AppearanceValues> };
const KEYS = { palette: 'noor-palette', theme: 'noor-theme', background: 'noor-wallpaper' } as const;
const recoveryKey = () => 'noor.onboarding.appearance.pending.' + encodeURIComponent(getApiBase());
export function draftAppearance(values: AppearanceValues): AppearanceDraft { return { values: { ...values }, changes: {} }; }
export function changeAppearanceDraft(draft: AppearanceDraft, changes: Partial<AppearanceValues>): AppearanceDraft {
	return { values: { ...draft.values, ...changes }, changes: { ...draft.changes, ...changes } };
}
function valid(field: keyof AppearanceValues, value: unknown): boolean {
	return field === 'theme' ? ['dark', 'light', 'system'].includes(String(value))
		: field === 'palette' ? PALETTES.some((item) => item.id === value) : WALLPAPERS.some((item) => item.id === value);
}
export function recoverAppearanceDraft(): AppearanceDraft | null {
	try {
		if (typeof sessionStorage === 'undefined') return null;
		const saved = JSON.parse(sessionStorage.getItem(recoveryKey()) ?? 'null') as AppearanceDraft | null;
		if (!saved?.values || !saved.changes) return null;
		if (!Object.keys(KEYS).every((field) => valid(field as keyof AppearanceValues, saved.values[field as keyof AppearanceValues]))) return null;
		if (!Object.entries(saved.changes).every(([field, value]) => Object.hasOwn(KEYS, field) && valid(field as keyof AppearanceValues, value))) return null;
		return saved;
	} catch { return null; }
}
export function rememberAppearanceDraft(draft: AppearanceDraft): void {
	if (!Object.keys(draft.changes).length) return;
	if (typeof sessionStorage === 'undefined') throw new Error('Appearance recovery storage is unavailable. Retry or skip these choices.');
	try { sessionStorage.setItem(recoveryKey(), JSON.stringify(draft)); }
	catch { throw new Error('Your appearance choices could not be kept for recovery. Retry or skip these choices.'); }
}
export function clearAppearanceRecovery(): void {
	try { if (typeof sessionStorage !== 'undefined') sessionStorage.removeItem(recoveryKey()); } catch { /* A successful save remains successful. */ }
}
/** Check every write before changing live stores; restore previous keys on partial failure. */
export function persistAppearanceDraft(draft: AppearanceDraft): void {
	const entries = Object.entries(draft.changes) as [keyof AppearanceValues, string][];
	if (!entries.length) return;
	if (typeof localStorage === 'undefined') throw new Error('Appearance could not be saved. Retry or continue with your previous look.');
	const previous = entries.map(([field]) => [KEYS[field], localStorage.getItem(KEYS[field])] as const);
	try {
		for (const [field, value] of entries) {
			if (!valid(field, value)) throw new Error('Invalid appearance choice.');
			localStorage.setItem(KEYS[field], value);
		}
	} catch {
		for (const [key, value] of previous) {
			try { if (value === null) localStorage.removeItem(key); else localStorage.setItem(key, value); } catch { /* Retain recovery for retry if storage is still blocked. */ }
		}
		throw new Error('Appearance could not be saved. Your choices are retained for retry.');
	}
}
