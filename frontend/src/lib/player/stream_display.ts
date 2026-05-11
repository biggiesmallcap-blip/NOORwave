export type PlayerStreamDisplay = {
	audio_quality?: string | null;
	sample_rate?: number | null;
	bit_depth?: number | null;
} | null;

export type PlayerRuntimeDisplay = {
	sample_rate?: number | null;
} | null;

const PART_SEPARATOR = ' \u00b7 ';

function formatSampleRate(sampleRate: number): string {
	const khz = sampleRate / 1000;
	return Number.isInteger(khz) ? `${khz} kHz` : `${khz.toFixed(1)} kHz`;
}

function formatStreamQuality(quality: string | null | undefined): string | null {
	const normalized = quality?.trim().toUpperCase();
	if (!normalized) return null;
	if (normalized === 'HI_RES_LOSSLESS') return 'Hi-Res Lossless';
	if (normalized === 'HI_RES') return 'Hi-Res';
	if (normalized === 'LOSSLESS') return 'Lossless';
	if (normalized === 'HIGH') return 'High';
	if (normalized === 'LOW') return 'Low';
	return normalized.replaceAll('_', ' ');
}

function formatQualityDetail(quality: string | null | undefined): string | null {
	const normalized = quality?.trim().toUpperCase();
	if (normalized === 'HIGH') return '320 kbps AAC';
	return null;
}

function inferStreamBitDepth(quality: string | null | undefined, sampleRate: number | null): number | null {
	if (sampleRate && sampleRate > 48000) return 24;
	if (quality === 'HI_RES_LOSSLESS' || quality === 'HI_RES') return 24;
	if (quality === 'LOSSLESS') return 16;
	return null;
}

export function formatResolutionShort(stream: PlayerStreamDisplay): string {
	if (!stream) return '';
	const bitDepth = stream.bit_depth;
	const sampleRate = stream.sample_rate;
	if (!bitDepth && !sampleRate && stream.audio_quality?.trim().toUpperCase() === 'HIGH') {
		return '320 kbps';
	}
	if (!bitDepth && !sampleRate) return '';
	const khz = sampleRate ? sampleRate / 1000 : null;
	const khzLabel = khz === null ? '' : Number.isInteger(khz) ? `${khz}` : khz.toFixed(1);
	if (bitDepth && sampleRate) return `${bitDepth}/${khzLabel}`;
	if (sampleRate) return `${khzLabel} kHz`;
	return `${bitDepth}-bit`;
}

export function formatPlayerStreamDetail({
	stream,
	exclusiveEngaged,
}: {
	stream: PlayerStreamDisplay;
	runtime?: PlayerRuntimeDisplay;
	exclusiveEngaged: boolean;
}): string {
	const parts: string[] = [];
	const sampleRate = stream?.sample_rate ?? null;
	const bitDepth = stream?.bit_depth ?? inferStreamBitDepth(stream?.audio_quality, sampleRate);
	const quality = formatStreamQuality(stream?.audio_quality);
	const qualityDetail = formatQualityDetail(stream?.audio_quality);

	if (quality) parts.push(quality);
	if (qualityDetail) parts.push(qualityDetail);
	if (sampleRate) parts.push(formatSampleRate(sampleRate));
	if (bitDepth) parts.push(`${bitDepth}-bit`);
	if (exclusiveEngaged) parts.push('Excl');
	return parts.join(PART_SEPARATOR);
}
