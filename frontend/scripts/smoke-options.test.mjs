import { readFileSync } from 'node:fs';

import { describe, expect, test } from 'vitest';

import {
	DEFAULT_PHASES,
	PHASE_ROUTES,
	parseSmokeOptions,
	redactSensitiveText,
	routesForPhase,
} from './smoke-options.mjs';

describe('parseSmokeOptions', () => {
	test('defaults to a safe full smoke run', () => {
		const options = parseSmokeOptions([]);

		expect(options.phase).toBe('full');
		expect(options.frontend).toBe('http://localhost:5173');
		expect(options.backend).toBe('http://localhost:3334');
		expect(options.viewport).toEqual({ width: 1920, height: 1080 });
		expect(options.headless).toBe(true);
		expect(options.keepOpen).toBe(false);
		expect(options.destructive).toBe(false);
		expect(options.shotsSuffix).toBe('');
	});

	test('parses explicit flags without enabling destructive mode by accident', () => {
		const options = parseSmokeOptions([
			'--phase',
			'menus',
			'--viewport',
			'390x844',
			'--artist',
			'42',
			'--album',
			'77',
			'--track',
			'12',
			'--playlist',
			'9',
			'--query',
			'Cocteau Twins',
			'--shots-suffix',
			'mobile',
			'--headed',
			'--keep-open',
		]);

		expect(options.phase).toBe('menus');
		expect(options.viewport).toEqual({ width: 390, height: 844 });
		expect(options.artistId).toBe(42);
		expect(options.albumId).toBe(77);
		expect(options.trackId).toBe(12);
		expect(options.playlistId).toBe(9);
		expect(options.query).toBe('Cocteau Twins');
		expect(options.shotsSuffix).toBe('mobile');
		expect(options.headless).toBe(false);
		expect(options.keepOpen).toBe(true);
		expect(options.destructive).toBe(false);
	});

	test('rejects unknown phases and malformed viewports', () => {
		expect(() => parseSmokeOptions(['--phase', 'delete-library'])).toThrow(/phase/);
		expect(() => parseSmokeOptions(['--viewport', 'large'])).toThrow(/viewport/);
	});
});

describe('routesForPhase', () => {
	test('full includes every phase route once in phase order', () => {
		const routes = routesForPhase('full');
		const expected = DEFAULT_PHASES.flatMap((phase) => PHASE_ROUTES[phase]);

		expect(routes).toEqual([...new Set(expected)]);
	});

	test('single phases are scoped', () => {
		expect(routesForPhase('shell')).toEqual(PHASE_ROUTES.shell);
		expect(routesForPhase('menus')).toEqual(PHASE_ROUTES.menus);
	});
});

describe('redactSensitiveText', () => {
	test('redacts setup tokens, bearer tokens, and local storage auth values', () => {
		const text = [
			'token=abc123',
			'Authorization: Bearer secret-value',
			'noor_api_token":"stored-token',
		].join(' ');

		expect(redactSensitiveText(text)).not.toContain('abc123');
		expect(redactSensitiveText(text)).not.toContain('secret-value');
		expect(redactSensitiveText(text)).not.toContain('stored-token');
		expect(redactSensitiveText(text)).toContain('[redacted]');
	});
});

describe('smoke runner source contracts', () => {
	test('waits for app readiness and route-specific shells before probing', () => {
		const source = readFileSync('scripts/smoke.mjs', 'utf8');

		expect(source).toContain('waitForAppReady');
		expect(source).toContain('routeShellSelector');
		expect(source).toContain('.genres-route');
		expect(source).toContain("page.waitForSelector('input'");
		expect(source).toContain('waitForTrackLikeRow');
		expect(source).toContain("dispatchEvent('contextmenu'");
		expect(source).toContain('waitForRouteSettled');
		expect(source).toContain('Loading genres');
	});
});
