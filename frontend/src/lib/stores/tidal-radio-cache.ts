import type { TidalMix } from '$lib/api/client';

/// In-memory cache for the home Personal Radio shelf. Mirrors the 6h backend
/// TTL so revisiting Home avoids a skeleton flash + unnecessary network round-trip.

const TTL_MS = 6 * 60 * 60 * 1000;

interface Entry {
	stations: TidalMix[];
	insertedAt: number;
}

let entry: Entry | null = null;

export function getCachedRadioStations(): TidalMix[] | null {
	if (!entry) return null;
	if (Date.now() - entry.insertedAt > TTL_MS) {
		entry = null;
		return null;
	}
	return entry.stations;
}

export function putCachedRadioStations(stations: TidalMix[]): void {
	entry = { stations, insertedAt: Date.now() };
}

export function clearCachedRadioStations(): void {
	entry = null;
}
