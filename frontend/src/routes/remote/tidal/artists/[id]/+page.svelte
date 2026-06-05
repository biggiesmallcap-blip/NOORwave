<script lang="ts">
	import { page } from '$app/state';
	import {
		api,
		type TidalArtistProfile,
		type TidalDiscographyTrack,
		type TidalPlayable
	} from '$lib/api/client';
	import {
		playTidalTracksNow,
		shuffleTidalTracksNow,
		startTidalSongRadio
	} from '$lib/stores/player';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import RemoteActionBar from '$lib/components/remote/RemoteActionBar.svelte';
	import RemoteAlbumTile from '$lib/components/remote/RemoteAlbumTile.svelte';
	import RemotePageShell from '$lib/components/remote/RemotePageShell.svelte';
	import RemoteTrackRow from '$lib/components/remote/RemoteTrackRow.svelte';

	function toPlayable(t: TidalDiscographyTrack): TidalPlayable {
		return {
			tidal_id: t.tidal_id,
			title: t.title,
			artist_name: t.artist_name ?? null,
			album_title: t.album_title ?? null,
			artwork_url: t.artwork_url ?? null,
			duration_ms: t.duration_ms,
			artist_tidal_id: t.artist_tidal_id ?? null,
			album_tidal_id: t.album_tidal_id ?? null
		};
	}

	let tidalArtistId = $derived(Number(page.params.id));

	let profile = $state<TidalArtistProfile | null>(null);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let loadSeq = 0;

	async function load(id: number) {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		profile = null;
		try {
			const next = await api.getTidalArtistProfile(id);
			if (seq !== loadSeq) return;
			profile = next;
		} catch (e) {
			if (seq !== loadSeq) return;
			error = `Couldn't load artist from TIDAL: ${e}`;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	$effect(() => {
		const id = tidalArtistId;
		void load(id);
	});

	let portraitSources = $derived.by(() => {
		const raw = [
			profile?.picture_url,
			profile?.top_tracks[0]?.artwork_url,
			profile?.albums[0]?.artwork_url
		];
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
	function albumHref(albumTidalId: number, localId: number | null): string {
		if (localId) return `/remote/albums/${localId}`;
		return `/remote/tidal/albums/${albumTidalId}`;
	}
</script>

<svelte:head>
	<title>{profile?.artist_name ?? 'Artist'} — NOOR Remote</title>
</svelte:head>

<RemotePageShell title={profile?.artist_name ?? ''}>
	{#if loading}
		<p class="remote-status" role="status" aria-live="polite">Loading from TIDAL…</p>
	{:else if error}
		<p class="remote-status remote-status-error">{error}</p>
	{:else if profile}
		{@const p = profile}
		<section class="remote-artist-hero">
			<div class="remote-artist-portrait">
				<ArtworkImage
					className="remote-artist-artwork"
					src={portraitSources}
					size={640}
					fallbackText={(p.artist_name ?? 'A').slice(0, 1)}
					decorative={true}
				/>
			</div>
			<span class="remote-tidal-badge">TIDAL preview</span>
			<h2>{p.artist_name ?? 'Artist'}</h2>
			<small>{p.top_tracks.length} top tracks · {p.albums.length} releases</small>
		</section>

		<RemoteActionBar
			disabled={p.top_tracks.length === 0}
			onPlay={() => playTidalTracksNow(p.top_tracks.map(toPlayable), p.artist_name ?? 'artist')}
			onShuffle={() => shuffleTidalTracksNow(p.top_tracks.map(toPlayable), p.artist_name ?? 'artist')}
			onRadio={p.top_tracks[0]
				? () => startTidalSongRadio(toPlayable(p.top_tracks[0]))
				: null}
		/>

		{#if p.top_tracks.length > 0}
			<section class="remote-section">
				<header>
					<h3>Top tracks</h3>
				</header>
				<div class="remote-track-list">
					{#each p.top_tracks as track (track.tidal_id)}
						<RemoteTrackRow variant="tidal" {track} />
					{/each}
				</div>
			</section>
		{/if}

		{#if p.albums.length > 0}
			<section class="remote-section">
				<header>
					<h3>Releases</h3>
				</header>
				<div class="remote-rail">
					{#each p.albums as album (album.tidal_id)}
						<RemoteAlbumTile
							title={album.title}
							artworkUrl={album.artwork_url}
							year={album.release_date?.slice(0, 4) ?? null}
							releaseType={album.release_type}
							href={albumHref(album.tidal_id, album.local_id)}
							menuArtistName={album.artist_name ?? p.artist_name}
							albumForMenu={{
								local_id: album.local_id ?? null,
								tidal_id: album.tidal_id,
								title: album.title,
								artist_name: album.artist_name ?? p.artist_name ?? null,
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
		gap: 6px;
		justify-items: center;
		padding: 4px 0;
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
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
		box-shadow: 0 18px 36px rgba(0, 0, 0, 0.35);
	}

	.remote-artist-portrait :global(.remote-artist-artwork) {
		width: 100%;
		height: 100%;
	}

	.remote-artist-portrait :global(img.remote-artist-artwork) {
		object-fit: cover;
		display: block;
	}

	.remote-artist-portrait :global(.remote-artist-artwork.fallback) {
		display: grid;
		place-items: center;
	}

	.remote-artist-portrait :global(.remote-artist-artwork.fallback span) {
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-semibold);
	}

	.remote-tidal-badge {
		display: inline-block;
		padding: 2px 8px;
		border-radius: 999px;
		background: var(--surface-2);
		color: var(--text-muted);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.06em;
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

	.remote-section header h3 {
		margin: 0;
		font-size: var(--font-size-sm);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-muted);
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
