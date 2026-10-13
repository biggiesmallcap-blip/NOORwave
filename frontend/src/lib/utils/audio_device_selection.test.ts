import { describe, expect, test } from 'vitest';
import { audioDeviceSelectionStatus, audioDeviceSelectionValue, savedOutputLabel } from './audio_device_selection';

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

	test('tells a disconnected output apart from an ambiguous saved name', () => {
		const twins = [...devices, { id: 'Wasapi:endpoint-c', name: 'Speakers' }];
		expect(audioDeviceSelectionStatus('Wasapi:endpoint-a', devices)).toBe('available');
		expect(audioDeviceSelectionStatus('Speakers', devices)).toBe('available');
		expect(audioDeviceSelectionStatus('Speakers', twins)).toBe('ambiguous');
		expect(audioDeviceSelectionStatus('Disconnected DAC', devices)).toBe('missing');
		expect(audioDeviceSelectionStatus(null, devices)).toBe('default');
	});

	test('never shows an endpoint GUID as the saved output name', () => {
		expect(savedOutputLabel('{0.0.0.00000000}.{8e1c2f9a-1111-2222-3333-444455556666}')).toBe('Saved output');
		expect(savedOutputLabel('Disconnected DAC')).toBe('Disconnected DAC');
	});
});
