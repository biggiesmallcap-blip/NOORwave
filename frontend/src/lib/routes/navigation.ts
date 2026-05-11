import navigationData from './navigation-data.json';
import { appRoute, appRoutePath, type AppRoute, type AppRouteId, type AppRouteZone } from './registry';

export type SmokePhase = 'shell' | 'links' | 'menus' | 'artist-album' | 'styling' | 'performance';

export interface NavigationZone {
	label: AppRouteZone;
	items: AppRoute[];
}

interface NavigationZoneData {
	label: AppRouteZone;
	routeIds: AppRouteId[];
}

interface NavigationData {
	navigationZones: NavigationZoneData[];
	mobileTabRouteIds: AppRouteId[];
	mobileMoreRouteIds: AppRouteId[];
	smokePhaseRouteIds: Record<SmokePhase, AppRouteId[]>;
}

const typedNavigationData = navigationData as NavigationData;

export const NAVIGATION_ZONES: NavigationZone[] = typedNavigationData.navigationZones.map((zone) => ({
	label: zone.label,
	items: zone.routeIds.map(appRoute),
}));

export const MOBILE_TAB_ROUTE_IDS = typedNavigationData.mobileTabRouteIds;
export const MOBILE_MORE_ROUTE_IDS = typedNavigationData.mobileMoreRouteIds;
export const MOBILE_TAB_ROUTES = MOBILE_TAB_ROUTE_IDS.map(appRoute);
export const MOBILE_MORE_ROUTES = MOBILE_MORE_ROUTE_IDS.map(appRoute);

export const SMOKE_PHASE_ROUTE_IDS = typedNavigationData.smokePhaseRouteIds;

export function routePathsForIds(ids: readonly AppRouteId[]): string[] {
	return ids.map(appRoutePath);
}
