import type { VideoCut } from '$lib/api/video_explore_types';
import { formatRuntime } from './clock';

// The broadcast credit, music-TV style: ARTIST, "Song", then year, cut and
// runtime in mono, and one why chip. It is in for the first seconds of a
// video and back for the last ones with what is up next.

export const LOWER_THIRD_IN_S = 6;
export const LOWER_THIRD_OUTRO_S = 10;

export type LowerThirdPhase = 'intro' | 'outro' | 'hidden';

/** Which credit shows at `positionS` of a video `durationS` long that this
 *  session started watching at `startedAtS` (a tune-in lands mid-video, and
 *  the credit greets you there, not at 0:00). The intro wins while it runs;
 *  the outro covers the last ten seconds. */
export function lowerThirdPhase(positionS: number, durationS: number, startedAtS = 0): LowerThirdPhase {
	if (!Number.isFinite(positionS) || positionS < 0) return 'hidden';
	const since = positionS - Math.max(0, startedAtS);
	if (since >= -0.5 && since < LOWER_THIRD_IN_S) return 'intro';
	if (Number.isFinite(durationS) && durationS > 0) {
		const remaining = durationS - positionS;
		if (remaining > 0 && remaining <= LOWER_THIRD_OUTRO_S) return 'outro';
	}
	return 'hidden';
}

/** Fields a credit can use. The channel endpoints send TidalSearchVideo plus
 *  `why`; `cut` and a year ride along when the server has them. */
export interface CreditSource {
	title: string;
	artist_name?: string | null;
	duration_ms?: number | null;
	type?: string | null;
	why?: string | null;
	cut?: string | null;
	song_year?: number | null;
	release_year?: number | null;
	year?: number | null;
}

export interface Credit {
	artist: string;
	song: string;
	/** The bracketed part of the title ("Live at Croke Park"), when any. */
	tag: string | null;
	year: number | null;
	cut: string | null;
	runtime: string;
	why: string | null;
}

const CUT_LABELS: Record<VideoCut, string> = {
	official: 'Official video',
	live: 'Live',
	session: 'Session',
	lyric: 'Lyric video',
	visualizer: 'Visualizer',
	remix: 'Remix',
	dance: 'Dance',
	interview: 'Interview',
	trailer: 'Trailer',
	clip: 'Clip',
	longform: 'Long form',
};

// A small fallback for when the server sends no cut: the obvious markers in
// the languages the catalog is full of. The server's classifier is the real one.
const CUT_MARKERS: [VideoCut, RegExp][] = [
	['live', /\b(live|en vivo|ao vivo|en directo|en concierto|in concert|unplugged)\b/i],
	['session', /\b(session|acoustic|ac.stico|stripped|tiny desk)\b/i],
	['lyric', /\b(lyric|lyrics|letra|paroles|testo)\b/i],
	['visualizer', /\b(visuali[sz]er|official audio)\b/i],
	['remix', /\bremix\b/i],
	['official', /\b(official (music )?video|video oficial|clip officiel|clipe oficial|videoclip)\b/i],
];

export function cutLabel(source: Pick<CreditSource, 'cut' | 'title' | 'type'>): string | null {
	const cut = source.cut as VideoCut | null | undefined;
	if (cut && cut in CUT_LABELS) return CUT_LABELS[cut];
	if (source.type && /^live$/i.test(source.type.trim())) return CUT_LABELS.live;
	for (const [kind, pattern] of CUT_MARKERS) if (pattern.test(source.title)) return CUT_LABELS[kind];
	return null;
}

/** "Swear It Again (Live at Croke Park)" -> song and tag. Only a trailing
 *  bracket is a tag; a title that is all bracket keeps itself. */
export function splitTitle(title: string): { song: string; tag: string | null } {
	const clean = title.trim();
	const match = /^(.*?\S)\s*[([]([^()[\]]+)[)\]]\s*$/.exec(clean);
	if (!match) return { song: clean, tag: null };
	return { song: match[1].trim(), tag: match[2].trim() || null };
}

function plausibleYear(value: unknown): number | null {
	return typeof value === 'number' && Number.isInteger(value) && value >= 1900 && value <= 2100 ? value : null;
}

export function creditFor(source: CreditSource, why?: string | null): Credit {
	const { song, tag } = splitTitle(source.title);
	return {
		artist: source.artist_name?.trim() || 'Unknown artist',
		song,
		tag,
		year: plausibleYear(source.song_year) ?? plausibleYear(source.release_year) ?? plausibleYear(source.year),
		cut: cutLabel(source),
		runtime: formatRuntime(source.duration_ms != null ? source.duration_ms / 1000 : null),
		why: (why ?? source.why)?.trim() || null,
	};
}
