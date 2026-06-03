import routeRegistryData from './registry-data.json';

export type AppRouteId =
	| 'home'
	| 'library'
	| 'search'
	| 'videos'
	| 'genres'
	| 'charts'
	| 'moods'
	| 'playlists'
	| 'discover'
	| 'automix'
	| 'dj'
	| 'analytics'
	| 'duplicates'
	| 'settings';

export type AppRouteZone = 'Atlas' | 'Signals' | 'System';

export interface AppRoute {
	id: AppRouteId;
	path: string;
	label: string;
	zone: AppRouteZone;
	icon: string;
}

export type AppRouteRegistry = {
	readonly [Id in AppRouteId]: Omit<AppRoute, 'id'>;
};

export const APP_ROUTES = routeRegistryData as AppRouteRegistry;

export const APP_ROUTE_IDS = Object.keys(APP_ROUTES) as AppRouteId[];

export const ROUTE_ZONES = ['Atlas', 'Signals', 'System'] as const satisfies readonly AppRouteZone[];

export function appRoute(id: AppRouteId): AppRoute {
	return { id, ...APP_ROUTES[id] };
}

export function isAppRouteId(value: string): value is AppRouteId {
	return Object.hasOwn(APP_ROUTES, value);
}
