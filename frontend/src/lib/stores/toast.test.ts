import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';
import { get } from 'svelte/store';
import { dismissToast, showToast, showUniqueToast, toasts } from './toast';

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
	for (const toast of get(toasts)) dismissToast(toast.id);
	vi.useRealTimers();
});

describe('unique notifications', () => {
	test('repeated failures reuse the visible toast and extend its lifetime', () => {
		const id = showUniqueToast('Exclusive mode unavailable', 'error', 8000);
		vi.advanceTimersByTime(7000);
		expect(showUniqueToast('Exclusive mode unavailable', 'error', 8000)).toBe(id);
		expect(get(toasts)).toHaveLength(1);
		vi.advanceTimersByTime(7000);
		expect(get(toasts)).toHaveLength(1);
		vi.advanceTimersByTime(1000);
		expect(get(toasts)).toHaveLength(0);
	});

	test('a dismissed failure can notify again', () => {
		const id = showUniqueToast('Exclusive mode unavailable', 'error');
		dismissToast(id);
		expect(showUniqueToast('Exclusive mode unavailable', 'error')).not.toBe(id);
		expect(get(toasts)).toHaveLength(1);
	});

	test('different messages or severity remain separate', () => {
		showUniqueToast('Format rejected', 'error');
		showUniqueToast('Device unavailable', 'error');
		showUniqueToast('Format rejected', 'info');
		expect(get(toasts)).toHaveLength(3);
	});

	test('ordinary notifications retain their existing behavior', () => {
		showToast('Saved', 'success');
		showToast('Saved', 'success');
		expect(get(toasts)).toHaveLength(2);
	});
});
