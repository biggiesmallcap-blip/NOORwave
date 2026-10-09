import type {
	ArtistReleaseFilter,
	ArtistReleaseFilterStatus,
	ArtistReleaseFilterStatuses,
	TidalArtistReleasePage,
	TidalDiscographyAlbum,
} from '$lib/api/client';
import type { DiscographySection } from './artist_discography';

const FILTERS: Record<Exclude<DiscographySection, 'tracks'>, ArtistReleaseFilter[]> = {
	albums: ['ALBUMS'],
	singles: ['EPSANDSINGLES'],
	compilations: ['COMPILATIONS'],
};

export type ReleaseSection = Exclude<DiscographySection, 'tracks'>;

export interface ReleaseFilterProgress {
	filter: ArtistReleaseFilter;
	status: ArtistReleaseFilterStatus;
	nextOffset: number;
}

export interface ReleaseLoadState {
	albums: TidalDiscographyAlbum[];
	filters: ReleaseFilterProgress[];
}

const UNKNOWN_STATUS: ArtistReleaseFilterStatus = { failed: true, has_more: null };

export function hasFailedPreviewFilter(
	section: ReleaseSection,
	statuses?: ArtistReleaseFilterStatuses,
): boolean {
	return FILTERS[section].some((filter) => statuses?.[filter]?.failed ?? true);
}

export function failedPreviewReleaseLinks(statuses?: ArtistReleaseFilterStatuses) {
	const links: { section: ReleaseSection; label: string; path: string }[] = [
		{ section: 'albums', label: 'Browse albums →', path: '/discography/albums' },
		{ section: 'singles', label: 'Browse singles and EPs →', path: '/discography/singles' },
		{ section: 'compilations', label: 'Browse compilations →', path: '/discography/compilations' },
	];
	return links.filter((link) => hasFailedPreviewFilter(link.section, statuses));
}

export function previewReleaseState(
	section: ReleaseSection,
	albums: TidalDiscographyAlbum[],
	statuses?: ArtistReleaseFilterStatuses,
): ReleaseLoadState {
	return {
		albums,
		filters: FILTERS[section].map((filter) => {
			const status = statuses?.[filter] ?? UNKNOWN_STATUS;
			return {
				filter,
				status: { ...status },
				nextOffset: status.failed ? 0 : 50,
			};
		}),
	};
}

export function releaseSectionComplete(state: ReleaseLoadState): boolean {
	return state.filters.every(({ status }) => !status.failed && status.has_more === false);
}

function mergeUniqueAlbums(
	current: TidalDiscographyAlbum[],
	additional: TidalDiscographyAlbum[],
): TidalDiscographyAlbum[] {
	const seen = new Set(current.map((album) => album.tidal_id));
	return [...current, ...additional.filter((album) => {
		if (seen.has(album.tidal_id)) return false;
		seen.add(album.tidal_id);
		return true;
	})];
}

export async function continueReleaseSection(
	initial: ReleaseLoadState,
	fetchPage: (filter: ArtistReleaseFilter, offset: number) => Promise<TidalArtistReleasePage>,
	onProgress: (state: ReleaseLoadState) => void = () => {},
	isCurrent: () => boolean = () => true,
): Promise<ReleaseLoadState> {
	let state: ReleaseLoadState = {
		albums: [...initial.albums],
		filters: initial.filters.map((progress) => ({ ...progress, status: { ...progress.status } })),
	};
	for (const filter of state.filters.map((progress) => progress.filter)) {
		while (isCurrent()) {
			const progress = state.filters.find((item) => item.filter === filter)!;
			if (!progress.status.failed && progress.status.has_more === false) break;
			let page: TidalArtistReleasePage;
			try {
				page = await fetchPage(filter, progress.nextOffset);
			} catch {
				page = { albums: [], status: UNKNOWN_STATUS };
			}
			if (!isCurrent()) return state;
			const succeeded = !page.status.failed && typeof page.status.has_more === 'boolean'
				&& (page.albums.length > 0 || page.status.has_more === false);
			state = {
				albums: succeeded ? mergeUniqueAlbums(state.albums, page.albums) : state.albums,
				filters: state.filters.map((item) => item.filter === filter ? {
					...item,
					status: succeeded ? { failed: false, has_more: page.status.has_more } : { ...UNKNOWN_STATUS },
					nextOffset: succeeded ? item.nextOffset + 50 : item.nextOffset,
				} : item),
			};
			onProgress(state);
			if (!succeeded || page.status.has_more === false) break;
		}
	}
	return state;
}
