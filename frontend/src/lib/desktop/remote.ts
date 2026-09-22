import type { RemoteStatus } from '$lib/api/remote';

export interface DesktopRemoteState {
	configured_host_mode: boolean;
	phase: 'idle' | 'restarting' | 'recovering' | 'failed';
	last_error: string | null;
}

export interface DesktopRemoteError {
	code: string;
	message: string;
	state: DesktopRemoteState;
}

function isState(value: unknown): value is DesktopRemoteState {
	if (!value || typeof value !== 'object') return false;
	const state = value as Partial<DesktopRemoteState>;
	return typeof state.configured_host_mode === 'boolean'
		&& ['idle', 'restarting', 'recovering', 'failed'].includes(state.phase ?? '')
		&& (state.last_error === null || typeof state.last_error === 'string');
}

export function parseDesktopRemoteError(value: unknown): DesktopRemoteError | null {
	if (typeof value === 'string') {
		try { return parseDesktopRemoteError(JSON.parse(value)); } catch { return null; }
	}
	if (value instanceof Error) return parseDesktopRemoteError(value.message);
	if (!value || typeof value !== 'object') return null;
	const error = value as Partial<DesktopRemoteError>;
	return typeof error.code === 'string' && typeof error.message === 'string' && isState(error.state)
		? error as DesktopRemoteError
		: null;
}

export function exposurePresentation(status: RemoteStatus): { label: string; online: boolean } {
	if (!status.effective_host_mode) {
		return { label: status.state === 'unavailable' ? 'Local only — server unavailable' : 'Local only', online: false };
	}
	if (status.state === 'running') return { label: 'Available on your network', online: true };
	if (status.state === 'starting') return { label: 'Starting network access…', online: false };
	return { label: 'Network access unavailable', online: false };
}
