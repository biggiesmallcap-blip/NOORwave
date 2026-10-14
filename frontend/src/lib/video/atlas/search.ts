// Finding an artist on the map by name: prefix matches first, then word
// starts, then anywhere in the name; bigger artists break ties. Accents and
// case are ignored so "beyonce" finds Beyonce with its accent. Scenes are
// found the same way, and also by channel number and by genre.

import type { AtlasScene } from '$lib/api/video_explore_types';
import type { AtlasData } from './decode';

export interface AtlasSearchHit {
	index: number;
	id: number;
	name: string;
	/** 0 exact, 1 prefix, 2 word start, 3 anywhere. */
	tier: number;
}

/** A scene found by its name, its channel number or one of its genres. */
export interface AtlasSceneHit {
	/** Index into `data.scenes`. */
	index: number;
	id: number;
	name: string;
	channelNo: number | null;
	tier: number;
	/** The genre that matched, when the scene was found by a genre. */
	tag: string | null;
}

const CHANNEL_QUERY = /^(?:ch(?:annel)?\.?\s*)?(\d{1,3})$/;

/** Scenes by channel number ("58", "ch 58"), name, or genre ("house"):
 *  exact first, then prefix, word start and anywhere; bigger scenes
 *  break ties. */
export function searchScenes(scenes: readonly AtlasScene[], query: string, limit = 4): AtlasSceneHit[] {
	const q = foldName(query);
	if (!q) return [];
	const channel = CHANNEL_QUERY.exec(q);
	const wanted = channel ? Number(channel[1]) : null;
	const scored: { hit: AtlasSceneHit; videos: number }[] = [];
	scenes.forEach((scene, index) => {
		let tier = -1;
		let tag: string | null = null;
		if (wanted !== null && scene.channel_no === wanted) tier = 0;
		const name = foldName(scene.name);
		const byName = name === q ? 0 : name.startsWith(q) ? 1 : name.includes(` ${q}`) ? 2 : name.includes(q) ? 3 : -1;
		if (byName >= 0 && (tier < 0 || byName < tier)) tier = byName;
		for (const raw of scene.tags ?? []) {
			const folded = foldName(raw);
			const byTag = folded === q ? 1 : folded.startsWith(q) ? 2 : q.length >= 3 && folded.includes(q) ? 3 : -1;
			if (byTag >= 0 && (tier < 0 || byTag < tier)) {
				tier = byTag;
				tag = raw;
			}
		}
		if (tier < 0) return;
		scored.push({ hit: { index, id: scene.id, name: scene.name, channelNo: scene.channel_no, tier, tag }, videos: scene.videos });
	});
	scored.sort((a, b) => a.hit.tier - b.hit.tier || b.videos - a.videos);
	return scored.slice(0, limit).map((s) => s.hit);
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
		scored.push({ hit: { index: entry.index, id: entry.id, name: entry.name, tier }, tier, rank: data.globalRank[entry.index] });
	}
	scored.sort((a, b) => a.tier - b.tier || a.rank - b.rank);
	return scored.slice(0, limit).map((s) => s.hit);
}
