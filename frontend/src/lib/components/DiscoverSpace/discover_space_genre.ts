// Genre families: the coarse grouping Sound Space uses for its angle sectors,
// the Genre lens colours and the nebula labels. Matching is by substring on
// the lowercased genre name, most specific family first ("pop punk" is punk,
// "indie pop" is indie, "trip-hop" is electronic, not hip-hop).

import type { DiscoverTrackNode } from './discover_space_types';

export interface GenreFamily {
	key: string;
	label: string;
	color: string;
	rgb: [number, number, number];
}

const FAMILIES: (GenreFamily & { match: string[] })[] = [
	{ key: 'punk', label: 'Punk', color: 'hsl(350,85%,58%)', rgb: [242, 56, 88], match: ['pop punk', 'poppunk', 'punk', 'emo', 'hardcore'] },
	{ key: 'metal', label: 'Metal', color: 'hsl(0,65%,48%)', rgb: [202, 43, 43], match: ['metal'] },
	{ key: 'hip-hop', label: 'Hip-Hop', color: 'hsl(38,75%,55%)', rgb: [226, 158, 55], match: ['hip-hop', 'hip hop', 'hiphop', 'rap', 'trap', 'grime'] },
	{ key: 'electronic', label: 'Electronic', color: 'hsl(270,70%,62%)', rgb: [158, 94, 230], match: ['electronic', 'electro', 'edm', 'techno', 'house', 'trance', 'dubstep', 'drum and bass', 'drum & bass', 'breakbeat', 'idm', 'synth', 'trip-hop', 'downtempo', 'chillwave'] },
	{ key: 'latin', label: 'Latin', color: 'hsl(12,80%,58%)', rgb: [237, 108, 66], match: ['latin', 'reggaeton', 'salsa', 'bossa', 'samba', 'cumbia'] },
	{ key: 'reggae', label: 'Reggae', color: 'hsl(130,50%,48%)', rgb: [61, 183, 86], match: ['reggae', 'dub', 'ska', 'dancehall'] },
	{ key: 'dance', label: 'Dance', color: 'hsl(300,60%,60%)', rgb: [214, 92, 214], match: ['dance', 'disco'] },
	{ key: 'ambient', label: 'Ambient', color: 'hsl(188,60%,60%)', rgb: [92, 201, 214], match: ['ambient', 'new age', 'drone'] },
	{ key: 'indie', label: 'Indie', color: 'hsl(175,55%,50%)', rgb: [57, 198, 184], match: ['indie', 'alternative', 'alt-', 'lo-fi', 'shoegaze'] },
	{ key: 'rock', label: 'Rock', color: 'hsl(212,75%,60%)', rgb: [76, 147, 229], match: ['rock', 'grunge', 'psychedelic'] },
	{ key: 'soul', label: 'Soul & R&B', color: 'hsl(25,75%,58%)', rgb: [228, 133, 68], match: ['soul', 'r&b', 'rnb', 'funk', 'gospel', 'motown'] },
	{ key: 'jazz', label: 'Jazz', color: 'hsl(44,65%,52%)', rgb: [218, 172, 48], match: ['jazz', 'swing', 'bebop'] },
	{ key: 'blues', label: 'Blues', color: 'hsl(225,25%,58%)', rgb: [121, 136, 175], match: ['blues'] },
	{ key: 'folk', label: 'Folk & Country', color: 'hsl(60,45%,50%)', rgb: [185, 185, 70], match: ['country', 'folk', 'americana', 'bluegrass', 'singer-songwriter'] },
	{ key: 'classical', label: 'Classical & Score', color: 'hsl(230,40%,70%)', rgb: [156, 166, 214], match: ['classical', 'orchestral', 'baroque', 'opera', 'soundtrack', 'score'] },
	{ key: 'experimental', label: 'Experimental', color: 'hsl(190,50%,60%)', rgb: [102, 189, 204], match: ['experimental', 'avant', 'noise'] },
	{ key: 'pop', label: 'Pop', color: 'hsl(330,75%,65%)', rgb: [237, 108, 172], match: ['pop'] },
	{ key: 'world', label: 'World', color: 'hsl(165,45%,52%)', rgb: [77, 189, 160], match: ['world', 'afro', 'celtic', 'flamenco'] },
];

/** Fill for nodes with no data on the active lens: neutral, never a fake value. */
export const UNKNOWN_FILL = 'hsl(230,8%,46%)';

export function genreFamily(genre: string | null | undefined): GenreFamily | null {
	if (!genre) return null;
	const g = genre.toLowerCase();
	for (const family of FAMILIES) {
		if (family.match.some((m) => g.includes(m))) return family;
	}
	return null;
}

export function nodeFamily(node: Pick<DiscoverTrackNode, 'genres' | 'topGenre'>): GenreFamily | null {
	return genreFamily(node.genres[0] ?? node.topGenre ?? null);
}

/** Families in display order, for the Genre lens legend. */
export function allFamilies(): GenreFamily[] {
	return FAMILIES.map(({ key, label, color, rgb }) => ({ key, label, color, rgb }));
}
