import { writable } from 'svelte/store';
import { api, type AudioFeaturesStats } from '$lib/api/client';

interface AudioAnalysisState {
	isRunning: boolean;
	analyzed: number;
	total: number;
	mode: string;
	stats: AudioFeaturesStats | null;
}

export const audioAnalysis = writable<AudioAnalysisState>({
	isRunning: false,
	analyzed: 0,
	total: 0,
	mode: '',
	stats: null,
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
		const response = await api.getAudioFeaturesStats();
		audioAnalysis.update((s) => ({ ...s, stats: response.stats }));
	} catch (e) {
		console.error('Failed to load audio stats:', e);
	}
}

export async function startAnalysis(mode: 'preview' | 'local', localPath?: string) {
	try {
		audioAnalysis.update((s) => ({ ...s, isRunning: true, mode }));
		await api.startAudioAnalysis(mode, localPath);
	} catch (e) {
		console.error('Failed to start analysis:', e);
		audioAnalysis.update((s) => ({ ...s, isRunning: false }));
	}
}

export async function stopAnalysis() {
	audioAnalysis.update((s) => ({ ...s, isRunning: false }));
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
