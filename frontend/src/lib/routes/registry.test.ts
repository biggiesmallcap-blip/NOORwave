import { describe, expect, test } from 'vitest';

import {
	APP_ROUTES,
	type AppRouteId,
	ROUTE_ZONES,
	appRoute,
	isAppRouteId,
} from './registry';
import {
	MOBILE_MORE_ROUTE_IDS,
	MOBILE_TAB_ROUTE_IDS,
	NAVIGATION_ZONES,
	SMOKE_PHASE_ROUTE_IDS,
} from './navigation';

describe('route registry', () => {
	test('keeps app routes keyed by typed ids', () => {
		const ids = Object.keys(APP_ROUTES) as AppRouteId[];

		expect(ids).toEqual([
			'home',
			'library',
			'search',
			'videos',
			'genres',
			'playlists',
			'discover',
			'automix',
			'analytics',
			'duplicates',
			'settings',
		]);
		expect(appRoute('discover').path).toBe('/discoverspace');
		expect(APP_ROUTES.genres.label).toBe('Genre Galaxy');
		expect(ROUTE_ZONES).toEqual(['Atlas', 'Signals', 'System']);
	});

	test('recognizes route ids without accepting random strings', () => {
		expect(isAppRouteId('library')).toBe(true);
		expect(isAppRouteId('discoverspace')).toBe(false);
	});
});

describe('navigation route groups', () => {
	test('matches the current desktop and mobile navigation order', () => {
		expect(NAVIGATION_ZONES.map((zone) => zone.label)).toEqual(['Atlas', 'Signals', 'System']);
		expect(NAVIGATION_ZONES.flatMap((zone) => zone.items.map((item) => item.id))).toEqual(
			Object.keys(APP_ROUTES)
		);
		expect(MOBILE_TAB_ROUTE_IDS).toEqual(['home', 'library', 'genres', 'discover']);
		expect(MOBILE_MORE_ROUTE_IDS).toEqual([
			'playlists',
			'automix',
			'analytics',
			'duplicates',
			'settings',
		]);
	});

	test('keeps smoke phases expressed in route ids', () => {
		expect(SMOKE_PHASE_ROUTE_IDS.shell).toEqual(['home', 'library', 'search', 'settings']);
		expect(SMOKE_PHASE_ROUTE_IDS.performance).toEqual([
			'home',
			'library',
			'search',
			'analytics',
			'discover',
			'videos',
		]);
	});
});
