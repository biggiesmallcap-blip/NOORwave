import { writable } from 'svelte/store';
import { api } from '$lib/api/client';

interface AcrCloudState {
	connected: boolean;
	scanned_today: number;
	daily_limit: number;
	isScanning: boolean;
	scanned: number;
	total: number;
	matches_found: number;
}

export const acrCloud = writable<AcrCloudState>({
	connected: false,
	scanned_today: 0,
	daily_limit: 1000,
	isScanning: false,
	scanned: 0,
	total: 0,
	matches_found: 0,
});

export function handleAcrCloudProgress(data: { scanned: number; total: number; matches_found: number }) {
	acrCloud.update((s) => ({
		...s,
		isScanning: true,
		scanned: data.scanned,
		total: data.total,
		matches_found: data.matches_found,
	}));
}

export function handleAcrCloudComplete(data: { scanned: number; matches_found: number }) {
	acrCloud.update((s) => ({
		...s,
		isScanning: false,
		scanned: data.scanned,
		matches_found: data.matches_found,
	}));
}

export async function loadAcrCloudStatus() {
	try {
		const response = await api.getAcrCloudStatus();
		acrCloud.update((s) => ({
			...s,
			connected: response.connected,
			scanned_today: response.scanned_today,
			daily_limit: response.daily_limit,
		}));
	} catch (e) {
		// not configured
	}
}

export async function configureAcrCloud(accessKey: string, accessSecret: string, region: string) {
	try {
		await api.configureAcrCloud(accessKey, accessSecret, region);
		acrCloud.update((s) => ({ ...s, connected: true }));
		await loadAcrCloudStatus();
	} catch (e) {
		console.error('Failed to configure ACRCloud:', e);
	}
}

export async function deleteAcrCloudConfig() {
	try {
		await api.deleteAcrCloudConfig();
		acrCloud.update((s) => ({ ...s, connected: false, scanned_today: 0 }));
	} catch (e) {
		console.error('Failed to delete ACRCloud config:', e);
	}
}

export async function startAcrCloudScan() {
	try {
		acrCloud.update((s) => ({ ...s, isScanning: true }));
		await api.startAcrCloudScan();
	} catch (e) {
		console.error('Failed to start ACRCloud scan:', e);
		acrCloud.update((s) => ({ ...s, isScanning: false }));
	}
}
