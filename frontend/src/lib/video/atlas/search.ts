// Finding an artist on the map by name: prefix matches first, then word
// starts, then anywhere in the name; bigger artists break ties. Accents and
// case are ignored so "beyonce" finds Beyonce with its accent.

import type { AtlasData } from './decode';

export interface AtlasSearchHit {
	index: number;
	id: number;
	name: string;
}

export function foldName(value: string): string {
	return value
		.normalize('NFKD')
		.replace(/[\u0300-\u036f]/g, '')
		.toLowerCase()
		.replace(/\s+/g, ' ')
		.trim();
}

export interface NameIndex {
	entries: { index: number; id: number; name: string; folded: string }[];
}

export function buildNameIndex(data: AtlasData, names: Map<number, string>): NameIndex {
	const entries: NameIndex['entries'] = [];
	for (let i = 0; i < data.count; i++) {
		const name = names.get(data.ids[i]);
		if (name) entries.push({ index: i, id: data.ids[i], name, folded: foldName(name) });
	}
	return { entries };
}

export function searchArtists(index: NameIndex, data: AtlasData, query: string, limit = 8): AtlasSearchHit[] {
	const q = foldName(query);
	if (!q) return [];
	const scored: { hit: AtlasSearchHit; tier: number; rank: number }[] = [];
	for (const entry of index.entries) {
		let tier = -1;
		if (entry.folded === q) tier = 0;
		else if (entry.folded.startsWith(q)) tier = 1;
		else if (entry.folded.includes(` ${q}`)) tier = 2;
		else if (entry.folded.includes(q)) tier = 3;
		if (tier < 0) continue;
		scored.push({ hit: { index: entry.index, id: entry.id, name: entry.name }, tier, rank: data.globalRank[entry.index] });
	}
	scored.sort((a, b) => a.tier - b.tier || a.rank - b.rank);
	return scored.slice(0, limit).map((s) => s.hit);
}
