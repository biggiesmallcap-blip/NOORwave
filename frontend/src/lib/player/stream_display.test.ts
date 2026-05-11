import { describe, expect, test } from 'vitest';
import { formatPlayerStreamDetail, formatResolutionShort } from './stream_display';

describe('formatPlayerStreamDetail', () => {
	test('shows confirmed lossless quality without using the output device rate', () => {
		const label = formatPlayerStreamDetail({
			stream: {
				audio_quality: 'LOSSLESS',
				sample_rate: null,
				bit_depth: null,
			},
			runtime: {
				sample_rate: 96_000,
			},
			exclusiveEngaged: false,
		});

		expect(label).toBe('Lossless \u00b7 16-bit');
	});

	test('uses confirmed hi-res stream sample rate and bit depth when the manifest reports them', () => {
		const label = formatPlayerStreamDetail({
			stream: {
				audio_quality: 'HI_RES_LOSSLESS',
				sample_rate: 192_000,
				bit_depth: 24,
			},
			runtime: {
				sample_rate: 96_000,
			},
			exclusiveEngaged: true,
		});

		expect(label).toBe('Hi-Res Lossless \u00b7 192 kHz \u00b7 24-bit \u00b7 Excl');
	});

	test('shows the lossy high bitrate when no resolution is reported', () => {
		const label = formatPlayerStreamDetail({
			stream: {
				audio_quality: 'HIGH',
				sample_rate: null,
				bit_depth: null,
			},
			exclusiveEngaged: true,
		});

		expect(label).toBe('High \u00b7 320 kbps AAC \u00b7 Excl');
	});

	test('uses bitrate for the short high quality badge', () => {
		const label = formatResolutionShort({
			audio_quality: 'HIGH',
			sample_rate: null,
			bit_depth: null,
		});

		expect(label).toBe('320 kbps');
	});
});
