import { get } from 'svelte/store';
import { beforeEach, expect, it } from 'vitest';
import {
	exclusiveStatus,
	markExclusiveNotEngaged,
	setExclusiveEngaged,
	setExclusiveFailed,
	setExclusiveReleased,
} from './exclusive_status';

beforeEach(() => {
	exclusiveStatus.set({ engaged: false, failureReason: null, device: null, transportFormat: null });
});

it('keeps a reported failure through runtime polls until a real event replaces it', () => {
	setExclusiveFailed('DAC', 'Device in use');
	markExclusiveNotEngaged('DAC');
	expect(get(exclusiveStatus).failureReason).toBe('Device in use');

	setExclusiveReleased('DAC');
	expect(get(exclusiveStatus).failureReason).toBeNull();
});

it('marks an engaged output as not engaged without inventing a failure', () => {
	setExclusiveEngaged('DAC', 'PCM 24-bit');
	markExclusiveNotEngaged('DAC');
	expect(get(exclusiveStatus)).toMatchObject({ engaged: false, failureReason: null, transportFormat: null });
});
