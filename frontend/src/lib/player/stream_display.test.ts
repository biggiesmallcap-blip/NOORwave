import { describe, expect, test } from 'vitest';
import { formatPlayerStreamDetail, formatPlayerStreamTooltip, formatResolutionShort } from './stream_display';

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

		expect(label).toBe('16-bit');
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

		expect(label).toBe('96 kHz output \u00b7 Excl');
	});

	test('keeps matching source and output rates compact', () => {
		expect(formatPlayerStreamDetail({
			stream: { sample_rate: 96_000, bit_depth: 24 },
			runtime: { sample_rate: 96_000 },
			exclusiveEngaged: true,
		})).toBe('96 kHz \u00b7 24-bit \u00b7 Excl');
	});

	test('identifies shared output resampling without claiming exclusive mode', () => {
		expect(formatPlayerStreamDetail({
			stream: { sample_rate: 44_100, bit_depth: 16 },
			runtime: { sample_rate: 48_000 },
			exclusiveEngaged: false,
		})).toBe('48 kHz output');
	});

	test('shows fixed 96 kHz output for a 44.1 kHz source', () => {
		expect(formatPlayerStreamDetail({
			stream: { sample_rate: 44_100, bit_depth: 16 },
			runtime: { sample_rate: 96_000 },
			exclusiveEngaged: true,
		})).toBe('96 kHz output \u00b7 Excl');
	});

	test('keeps source resolution in the tooltip', () => {
		expect(formatPlayerStreamTooltip({ sample_rate: 44_100, bit_depth: 16 }, '96 kHz output \u00b7 Excl'))
			.toBe('96 kHz output \u00b7 Excl \u00b7 44.1 kHz / 16-bit source');
		expect(formatPlayerStreamTooltip(null, '96 kHz output')).toBe('96 kHz output');
		// Matching rates: the label already names the source.
		expect(formatPlayerStreamTooltip({ sample_rate: 96_000, bit_depth: 24 }, '96 kHz · 24-bit · Excl'))
			.toBe('96 kHz · 24-bit · Excl');
	});

	test.each([undefined, 0, -1, NaN, Infinity])('does not invent an output rate for %s', (sampleRate) => {
		expect(formatPlayerStreamDetail({
			stream: { sample_rate: 192_000, bit_depth: 24 },
			runtime: { sample_rate: sampleRate },
			exclusiveEngaged: true,
		})).toBe('192 kHz \u00b7 24-bit \u00b7 Excl');
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

		expect(label).toBe('320 kbps AAC \u00b7 Excl');
	});

	test('marks exclusive playback as DJ processed when the DJ engine is on', () => {
		const label = formatPlayerStreamDetail({
			stream: {
				audio_quality: 'HI_RES_LOSSLESS',
				sample_rate: 96_000,
				bit_depth: 24,
			},
			runtime: {
				sample_rate: 96_000,
				dj_engine_enabled: true,
			},
			exclusiveEngaged: true,
		});

		expect(label).toBe('96 kHz \u00b7 24-bit \u00b7 Excl DJ');
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
