<script lang="ts">
	import { page } from '$app/state';
	import {
		api,
		type TidalDiscographyAlbum,
		type Track
	} from '$lib/api/client';
	import {
		playArtist,
		shuffleArtist,
		startArtistRadio
	} from '$lib/stores/player';
	import RemoteActionBar from '$lib/components/remote/RemoteActionBar.svelte';
	import RemoteAlbumTile from '$lib/components/remote/RemoteAlbumTile.svelte';
	import RemotePageShell from '$lib/components/remote/RemotePageShell.svelte';
	import RemoteTrackRow from '$lib/components/remote/RemoteTrackRow.svelte';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';

	interface ArtistDetail {
		id: number;
		tidal_id: number | null;
		name: string;
		biography: string | null;
		photo_url: string | null;
		track_count: number;
		album_count: number;
	}

	let artistId = $derived(Number(page.params.id));

	let artist = $state<ArtistDetail | null>(null);
	let tracks = $state<Track[]>([]);
	let albums = $state<TidalDiscographyAlbum[]>([]);
	let tidalPictureUrl = $state<string | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let showAllTracks = $state(false);
	// TIDAL's CDN 403s individual covers/portraits unpredictably; instead of a
	// single boolean "failed" we walk a cascade of candidate URLs and advance
	// each time one errors. Initial letter is only shown when every candidate
	// has been tried.
	let portraitSourceIndex = $state(0);

	async function load() {
		loading = true;
		error = null;
		try {
			const [artistRes, tracksRes] = await Promise.all([
				api.getArtist(artistId),
				api.getArtistTracks(artistId)
			]);
			artist = artistRes;
			tracks = tracksRes.tracks;
		} catch (err) {
			error = `Couldn't load artist: ${err}`;
		} finally {
			loading = false;
		}
		// Discography is best-effort; an error here shouldn't blank the page.
		// We also grab the fresh TIDAL `picture_url` which is more reliable than
		// the locally-cached `photo_url` (see desktop /artists/[id] for context).
		try {
			const disco = await api.getArtistDiscography(artistId);
			albums = disco.albums ?? [];
			tidalPictureUrl = disco.picture_url ?? null;
		} catch {
			albums = [];
		}
	}

	$effect(() => {
		artistId;
		void load();
	});

	// Walk a cascade of candidate URLs so a 403 on one (e.g. a stale TIDAL
	// photo_url that the CDN now AccessDenies at 640) advances to the next
	// candidate rather than dead-ending at the initial-letter fallback. We
	// also dedupe so the same URL isn't tried twice.
	let portraitSources = $derived.by(() => {
		const raw = [tidalPictureUrl, artist?.photo_url, tracks[0]?.artwork_url];
		const seen = new Set<string>();
		const out: string[] = [];
		for (const src of raw) {
			if (!src || typeof src !== 'string') continue;
			const trimmed = src.trim();
			if (!trimmed || seen.has(trimmed)) continue;
			seen.add(trimmed);
			out.push(trimmed);
		}
		return out;
	});
	$effect(() => {
		// Reset the cascade pointer whenever the underlying data set changes
		// (artist navigation, refetch). Without this we'd carry a stale index
		// when navigating between artists.
		portraitSources;
		portraitSourceIndex = 0;
	});
	let currentPortraitSource = $derived(
		portraitSourceIndex < portraitSources.length ? portraitSources[portraitSourceIndex] : null
	);
	let portrait = $derived(upscaleTidalArtwork(currentPortraitSource, 640));
	let backdrop = $derived(upscaleTidalArtwork(currentPortraitSource));

	function onPortraitError() {
		// Advance once per failed URL; when we exhaust the list, the template
		// falls through to the initial-letter placeholder.
		portraitSourceIndex = Math.min(portraitSourceIndex + 1, portraitSources.length);
	}
	let displayTracks = $derived(showAllTracks ? tracks : tracks.slice(0, 10));
	let countsLine = $derived.by(() => {
		if (!artist) return '';
		const parts: string[] = [];
		if (artist.track_count) {
			parts.push(`${artist.track_count} ${artist.track_count === 1 ? 'track' : 'tracks'}`);
		}
		if (artist.album_count) {
			parts.push(`${artist.album_count} ${artist.album_count === 1 ? 'album' : 'albums'}`);
		}
		return parts.join(' · ');
	});

	function albumHref(album: TidalDiscographyAlbum): string {
		if (album.local_id) return `/remote/albums/${album.local_id}`;
		return `/remote/tidal/albums/${album.tidal_id}`;
	}
</script>

<svelte:head>
	<title>{artist?.name ?? 'Artist'} — NOOR Remote</title>
</svelte:head>

<RemotePageShell title={artist?.name ?? ''}>
	{#if loading}
		<p class="remote-status" role="status" aria-live="polite">Loading…</p>
	{:else if error}
		<p class="remote-status remote-status-error">{error}</p>
	{:else if artist}
		<section class="remote-artist-hero">
			<div class="remote-artist-portrait">
				{#if portrait}
					<img src={portrait} alt="" onerror={onPortraitError} />
				{:else}
					<span aria-hidden="true">{(artist?.name ?? 'A').slice(0, 1)}</span>
				{/if}
			</div>
			<h2>{artist.name}</h2>
			{#if countsLine}
				<small>{countsLine}</small>
			{/if}
		</section>

		<RemoteActionBar
			disabled={tracks.length === 0}
			onPlay={() => playArtist(artistId)}
			onShuffle={() => shuffleArtist(artistId)}
			onRadio={() => startArtistRadio(artistId)}
		/>

		{#if tracks.length > 0}
			<section class="remote-section">
				<header>
					<h3>Top tracks</h3>
					{#if tracks.length > 10}
						<button
							type="button"
							class="remote-section-toggle"
							onclick={() => {
								showAllTracks = !showAllTracks;
							}}
						>
							{showAllTracks ? 'Show less' : `Show all (${tracks.length})`}
						</button>
					{/if}
				</header>
				<div class="remote-track-list">
					{#each displayTracks as track (track.id)}
						<RemoteTrackRow {track} artistIdForPlay={artistId} />
					{/each}
				</div>
			</section>
		{/if}

		{#if albums.length > 0}
			<section class="remote-section">
				<header>
					<h3>Albums</h3>
				</header>
				<div class="remote-rail">
					{#each albums as album (album.tidal_id)}
						<RemoteAlbumTile
							title={album.title}
							artworkUrl={album.artwork_url}
							year={album.release_date?.slice(0, 4) ?? null}
							releaseType={album.release_type}
							href={albumHref(album)}
							menuArtistName={album.artist_name}
							albumForMenu={{
								local_id: album.local_id ?? null,
								tidal_id: album.tidal_id,
								title: album.title,
								artist_id: artist.id,
								artist_name: album.artist_name ?? artist.name ?? null,
								in_library: !!album.local_id
							}}
						/>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</RemotePageShell>

<style>
	.remote-status {
		margin: 24px 0 0;
		text-align: center;
		color: var(--text-muted);
	}

	.remote-status-error {
		color: var(--state-error);
	}

	.remote-artist-hero {
		display: grid;
		gap: 8px;
		justify-items: center;
		padding: 4px 0 4px;
		text-align: center;
	}

	.remote-artist-portrait {
		width: 140px;
		height: 140px;
		border-radius: 999px;
		overflow: hidden;
		background: var(--surface-1);
		display: grid;
		place-items: center;
		color: var(--text-muted);
		box-shadow: 0 18px 36px rgba(0, 0, 0, 0.35);
	}

	.remote-artist-portrait img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.remote-artist-hero h2 {
		margin: 4px 0 0;
		font-size: var(--font-size-lg);
	}

	.remote-artist-hero small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-section {
		display: grid;
		gap: 8px;
	}

	.remote-section header {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 8px;
	}

	.remote-section header h3 {
		margin: 0;
		font-size: var(--font-size-sm);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-muted);
	}

	.remote-section-toggle {
		background: transparent;
		color: var(--accent);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.remote-track-list {
		display: grid;
		gap: 2px;
	}

	.remote-rail {
		display: flex;
		gap: 6px;
		overflow-x: auto;
		-webkit-overflow-scrolling: touch;
		padding-bottom: 4px;
		scroll-snap-type: x proximity;
	}

	.remote-rail :global(.remote-album-tile) {
		scroll-snap-align: start;
	}
</style>
