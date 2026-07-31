<script lang="ts">
	import { type Track, type TidalDiscographyAlbum, type TidalDiscographyTrack } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import TrackRow from '$lib/components/TrackRow.svelte';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import SearchField from '$lib/search/ui/SearchField.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import { goBack } from '$lib/navigation/back';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { playAlbum, playArtist, playTidalAlbum } from '$lib/stores/player';
	import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';
	import {
		buildPopularTrackItems,
		discographySectionFor,
		popularTrackItemKey,
		sortTidalAlbumsByReleaseDate,
		type PopularTrackItem,
	} from './artist_discography';

	type Section = 'tracks' | 'albums' | 'singles' | 'compilations';

	// Same dual-source split as ArtistDetail: a library artist is keyed by local
	// id (rich local rows), a non-library artist by TIDAL id (sourced from the
	// TIDAL profile endpoint, which returns the same discography depth).
	type ArtistSource =
		| { kind: 'local'; artistId: number }
		| { kind: 'tidal'; tidalArtistId: number };

	let { source, section }: { source: ArtistSource; section: Section } = $props();

	const SECTION_LABELS: Record<Section, string> = {
		tracks: 'Top tracks',
		albums: 'Albums',
		singles: 'Singles and EPs',
		compilations: 'Compilations',
	};

	let artistId = $derived(source.kind === 'local' ? source.artistId : 0);

	let artist = $state<{ id: number; tidal_id: number | null; name: string } | null>(null);
	let tracks = $state<Track[]>([]);
	let tidalTracks = $state<TidalDiscographyTrack[]>([]);
	let tidalAlbums = $state<TidalDiscographyAlbum[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let query = $state('');
	let loadSeq = 0;

	let activeTidalArtistId = $derived(
		source.kind === 'tidal' ? source.tidalArtistId : (artist?.tidal_id ?? null)
	);
	let artistHref = $derived(
		source.kind === 'tidal' ? `/tidal/artists/${source.tidalArtistId}` : `/artists/${artistId}`
	);

	async function loadLocal(id: number) {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const [artistRes, tracksRes, discographyRes] = await Promise.allSettled([
				cachedApi.getArtist(id),
				cachedApi.getArtistTracks(id),
				cachedApi.getArtistDiscography(id),
			]);
			if (seq !== loadSeq) return;
			artist = artistRes.status === 'fulfilled' ? artistRes.value : null;
			tracks = tracksRes.status === 'fulfilled' ? tracksRes.value.tracks : [];
			if (discographyRes.status === 'fulfilled') {
				tidalTracks = discographyRes.value.top_tracks ?? [];
				tidalAlbums = discographyRes.value.albums ?? [];
			} else {
				tidalTracks = [];
				tidalAlbums = [];
			}
			if (artistRes.status !== 'fulfilled' && tracksRes.status !== 'fulfilled') {
				error = `Failed to load artist: ${tracksRes.reason}`;
			}
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	async function loadTidal(tidalId: number) {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const res = await cachedApi.getTidalArtistProfile(tidalId);
			if (seq !== loadSeq) return;
			artist = { id: 0, tidal_id: tidalId, name: res.artist_name ?? 'Artist' };
			tracks = [];
			tidalTracks = res.top_tracks ?? [];
			tidalAlbums = res.albums ?? [];
		} catch (e) {
			if (seq !== loadSeq) return;
			error = String(e);
			tidalTracks = [];
			tidalAlbums = [];
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	$effect(() => {
		if (source.kind === 'local') {
			void loadLocal(source.artistId);
		} else {
			void loadTidal(source.tidalArtistId);
		}
	});

	// Bucketing, ordering, and Top-tracks merging come from the shared
	// artist_discography helper - the private copies this component used to
	// carry had drifted from ArtistDetail's (LIVE releases were bucketed
	// differently between the artist page and this see-all page).
	let popularItems = $derived.by<PopularTrackItem[]>(() =>
		buildPopularTrackItems(tracks, tidalTracks)
	);

	let albumsForSection = $derived(
		sortTidalAlbumsByReleaseDate(
			tidalAlbums.filter((album) => discographySectionFor(album) === section)
		)
	);

	function matches(text: string | null | undefined): boolean {
		const normalized = query.trim().toLowerCase();
		if (!normalized) return true;
		return (text ?? '').toLowerCase().includes(normalized);
	}

	let visibleTracks = $derived(
		popularItems.filter((item) =>
			matches(item.track.title) || matches(item.track.album_title)
		)
	);
	let visibleAlbums = $derived(
		albumsForSection.filter((album) => matches(album.title) || matches(album.artist_name))
	);

	function itemKey(item: PopularTrackItem): string {
		return popularTrackItemKey(item);
	}

	function artistTrackPlayable(track: TidalDiscographyTrack) {
		return tidalDiscographyTrackToPlayable(track, { artistTidalId: activeTidalArtistId });
	}

	function albumHref(album: TidalDiscographyAlbum): string {
		return album.local_id != null ? `/albums/${album.local_id}` : `/tidal/albums/${album.tidal_id}`;
	}

	function albumMenu(album: TidalDiscographyAlbum) {
		return buildAlbumMenu(
			{
				local_id: album.local_id,
				tidal_id: album.tidal_id,
				title: album.title,
				artist_name: album.artist_name,
				in_library: album.in_library,
			},
			{ isLocal: album.in_library && album.local_id != null },
		);
	}

	function playAlbumFromCard(album: TidalDiscographyAlbum, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		if (album.local_id != null) void playAlbum(album.local_id);
		else void playTidalAlbum(album.tidal_id);
	}
</script>

<div class="discography-page">
	<button class="back-link" type="button" onclick={() => goBack(artistHref)}>Back</button>

	{#if loading}
		<Skeleton rows={5} label="Loading discography" />
	{:else if error}
		<EmptyState title="Discography could not load" copy={error}>
			{#snippet actions()}
				<a class="empty-action" href={artistHref}>Back to artist</a>
			{/snippet}
		</EmptyState>
	{:else}
		<header class="page-head">
			<p class="eyebrow">{artist?.name ?? 'Artist'}</p>
			<h1>{SECTION_LABELS[section]}</h1>
			<p>{section === 'tracks' ? visibleTracks.length : visibleAlbums.length} results</p>
		</header>

		<div class="filter-bar">
			<SearchField
				bind:value={query}
				variant="page"
				size="sm"
				fill
				placeholder={`Search ${SECTION_LABELS[section].toLowerCase()}...`}
			/>
		</div>

		{#if section === 'tracks'}
			{#if visibleTracks.length > 0}
				<ol class="track-list">
					{#each visibleTracks as item, idx (itemKey(item))}
						{#if item.kind === 'local'}
							<TrackRow
								track={item.track}
								variant="numbered"
								index={idx}
								isCurrent={false}
								isPlaying={false}
								showArtist={false}
								onRowClick={() => void playArtist(artistId, item.track.id)}
								menuOptions={{ hideArtistActions: true }}
							/>
						{:else}
							<TidalTrackRow
								track={artistTrackPlayable(item.track)}
								variant="numbered"
								index={idx}
								showArtist={false}
							/>
						{/if}
					{/each}
				</ol>
			{:else}
				<p class="empty-copy">No tracks match this search.</p>
			{/if}
		{:else if visibleAlbums.length > 0}
			<div class="album-grid">
				{#each visibleAlbums as album (album.tidal_id)}
					<a
						class="album-card"
						href={albumHref(album)}
						oncontextmenu={(event) => {
							event.preventDefault();
							event.stopPropagation();
							openContextMenu(event, albumMenu(album), album.title);
						}}
					>
						<div class="album-art">
							<ArtworkImage
								src={album.artwork_url}
								size={320}
								tint={true}
								fallbackText={album.title.slice(0, 1)}
								decorative={true}
							/>
							<button
								class="play-overlay"
								type="button"
								aria-label="Play {album.title}"
								onclick={(event) => playAlbumFromCard(album, event)}
							><svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><path d="M5 3l8 5-8 5V3z" fill="currentColor" /></svg></button>
						</div>
						<p class="album-title">{album.title}</p>
						<p class="album-sub">
							{#if album.release_date}{album.release_date.slice(0, 4)} / {/if}{album.number_of_tracks ?? 0} tracks
						</p>
					</a>
				{/each}
			</div>
		{:else}
			<p class="empty-copy">No releases match this search.</p>
		{/if}
	{/if}
</div>

<style>
	.discography-page {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		padding: var(--space-5);
	}

	.back-link {
		display: inline-flex;
		margin-bottom: var(--space-4);
		color: var(--text-secondary);
		text-decoration: none;
		font-size: var(--font-size-sm);
	}

	.back-link:hover {
		color: var(--text-primary);
	}

	.page-head {
		display: grid;
		gap: var(--space-1);
		margin-bottom: var(--space-4);
	}

	.eyebrow {
		margin: 0;
		color: var(--accent);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	h1 {
		margin: 0;
		font-size: var(--font-size-3xl);
		line-height: var(--line-height-tight);
	}

	.page-head p {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.filter-bar {
		margin-bottom: var(--space-4);
		max-width: min(420px, 100%);
	}

	.track-list {
		list-style: none;
		display: grid;
		gap: var(--space-1);
		margin: 0;
		padding: 0;
	}

	.album-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(180px, 100%), 1fr));
		gap: var(--space-4);
	}

	.album-card {
		display: grid;
		gap: var(--space-2);
		color: inherit;
		text-decoration: none;
		transition: transform var(--motion-base);
	}

	.album-card:hover {
		transform: translateY(-4px);
	}

	.album-art {
		position: relative;
		aspect-ratio: 1 / 1;
		overflow: hidden;
		border-radius: var(--radius-md);
		background: var(--bg-raised);
		box-shadow: 0 2px 8px rgba(0, 0, 0, 0.22);
		transition: box-shadow var(--motion-base);
	}

	.album-card:hover .album-art {
		box-shadow: 0 12px 26px -6px rgba(0, 0, 0, 0.5);
	}

	.album-art :global(img),
	.album-art :global(.fallback) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	/* Circular accent play button in the artwork's bottom-right, revealed on hover. */
	.play-overlay {
		position: absolute;
		inset: auto 8px 8px auto;
		display: grid;
		place-items: center;
		width: 40px;
		height: 40px;
		border: 0;
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
		box-shadow: 0 6px 16px -4px rgba(0, 0, 0, 0.55);
		opacity: 0;
		cursor: pointer;
		transition: opacity var(--motion-base), transform var(--motion-base), filter var(--motion-fast);
		transform: translateY(6px);
	}

	.play-overlay svg {
		margin-left: 1px;
	}

	.play-overlay:hover {
		transform: translateY(0) scale(1.06);
		filter: brightness(1.08);
	}

	.album-card:hover .play-overlay,
	.album-card:focus-within .play-overlay {
		opacity: 1;
		transform: translateY(0);
	}

	.album-title,
	.album-sub,
	.empty-copy {
		margin: 0;
	}

	.album-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.album-sub,
	.empty-copy {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}
</style>
