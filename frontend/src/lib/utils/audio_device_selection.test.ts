import { describe, expect, test } from 'vitest';
import { audioDeviceSelectionValue } from './audio_device_selection';

describe('saved output device selection', () => {
	const devices = [
		{ id: 'Wasapi:endpoint-a', name: 'Realtek Digital Output' },
		{ id: 'Wasapi:endpoint-b', name: 'Speakers' },
	];

	test('shows the exact endpoint and resolves an existing unique saved name', () => {
		expect(audioDeviceSelectionValue('Wasapi:endpoint-a', devices)).toBe('Wasapi:endpoint-a');
		expect(audioDeviceSelectionValue('Realtek Digital Output', devices)).toBe('Wasapi:endpoint-a');
	});

	test('preserves missing or ambiguous preferences so the route can show them as unavailable', () => {
		expect(audioDeviceSelectionValue('Disconnected DAC', devices)).toBe('Disconnected DAC');
		expect(audioDeviceSelectionValue('Speakers', [...devices, { id: 'Wasapi:endpoint-c', name: 'Speakers' }])).toBe('Speakers');
	});

	test('retains explicit system default preferences', () => {
		expect(audioDeviceSelectionValue(null, devices)).toBe('__default__');
		expect(audioDeviceSelectionValue('default', devices)).toBe('__default__');
	});
});
