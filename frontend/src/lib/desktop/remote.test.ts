import { describe, expect, it } from 'vitest';
import { exposurePresentation, parseDesktopRemoteError, type DesktopRemoteState } from './remote';
import type { RemoteStatus } from '$lib/api/remote';

const nativeState: DesktopRemoteState = { configured_host_mode: false, phase: 'failed', last_error: 'bind failed' };

it('parses a plain serialized native error and keeps its authoritative state', () => {
	const parsed = parseDesktopRemoteError(JSON.stringify({ code: 'SERVER_START_FAILED', message: 'disable failed', state: nativeState }));
	expect(parsed).toEqual({ code: 'SERVER_START_FAILED', message: 'disable failed', state: nativeState });
});

describe('effective exposure wording', () => {
	it.each([
		[false, 'disabled', 'Local only'],
		[false, 'unavailable', 'Local only — server unavailable'],
		[true, 'running', 'Available on your network'],
		[true, 'starting', 'Starting network access…'],
		[true, 'unavailable', 'Network access unavailable'],
	] as const)('maps effective=%s state=%s', (effective, state, label) => {
		const status = { effective_host_mode: effective, state } as RemoteStatus;
		expect(exposurePresentation(status).label).toBe(label);
	});
});
