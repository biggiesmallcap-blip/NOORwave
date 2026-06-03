import { writable } from 'svelte/store';
import { api, type AudioFeaturesStats } from '$lib/api/client';
import { cachedApi } from '$lib/cache/api_queries';

interface AudioAnalysisState {
	isRunning: boolean;
	analyzed: number;
	total: number;
	mode: string;
	stats: AudioFeaturesStats | null;
	passiveEnabled: boolean;
}

export const audioAnalysis = writable<AudioAnalysisState>({
	isRunning: false,
	analyzed: 0,
	total: 0,
	mode: '',
	stats: null,
	passiveEnabled: true,
});

export function handleAnalysisProgress(data: { analyzed: number; total: number; mode: string }) {
	audioAnalysis.update((s) => ({
		...s,
		isRunning: true,
		analyzed: data.analyzed,
		total: data.total,
		mode: data.mode,
	}));
}

export function handleAnalysisComplete(data: { analyzed: number }) {
	audioAnalysis.update((s) => ({
		...s,
		isRunning: false,
		analyzed: data.analyzed,
	}));
}

export async function loadAudioStats() {
	try {
		const response = await cachedApi.getAudioFeaturesStats();
		audioAnalysis.update((s) => ({ ...s, stats: response.stats }));
	} catch (e) {
		console.error('Failed to load audio stats:', e);
	}
}

export async function syncAnalysisStatus() {
	try {
		const { running, analyzed } = await cachedApi.getAudioAnalysisStatus();
		audioAnalysis.update((s) => ({ ...s, isRunning: running, analyzed }));
	} catch (e) {
		console.error('Failed to sync analysis status:', e);
	}
}

export async function clearAllAnalysis() {
	if (!confirm('Delete all audio analysis data?')) return;
	try {
		await api.resetAudioAnalysis();
		audioAnalysis.update((s) => ({ ...s, analyzed: 0, stats: null }));
	} catch (e) {
		console.error('Failed to reset analysis:', e);
	}
}

export async function loadPassiveDspState() {
	try {
		const { enabled } = await cachedApi.getPassiveDsp();
		audioAnalysis.update((s) => ({ ...s, passiveEnabled: enabled }));
	} catch (e) {
		console.error('Failed to load passive DSP setting:', e);
	}
}

export async function setPassiveDspEnabled(enabled: boolean) {
	audioAnalysis.update((s) => ({ ...s, passiveEnabled: enabled }));
	try {
		await api.setPassiveDsp(enabled);
	} catch (e) {
		console.error('Failed to update passive DSP setting:', e);
		audioAnalysis.update((s) => ({ ...s, passiveEnabled: !enabled }));
	}
}
