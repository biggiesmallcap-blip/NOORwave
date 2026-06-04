<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, type TidalDiscographyTrack, type TidalPlayable } from '$lib/api/client';
	import { playTidalTracksNow, shuffleTidalTracksNow } from '$lib/stores/player';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import RemoteActionBar from '$lib/components/remote/RemoteActionBar.svelte';
	import RemotePageShell from '$lib/components/remote/RemotePageShell.svelte';
	import RemoteTrackRow from '$lib/components/remote/RemoteTrackRow.svelte';
	import { firstArtworkUrl } from '$lib/utils/artwork';
	import { formatTotalDuration } from '$lib/utils/format';
	import { hapticTap } from '$lib/remote/haptics';

	let tidalAlbumId = $derived(Number(page.params.id));

	let tracks = $state<TidalDiscographyTrack[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

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

	async function load() {
		loading = true;
		error = null;
		try {
			const res = await api.getTidalAlbumTracks(tidalAlbumId);
			tracks = res.tracks;
		} catch (err) {
			error = `Couldn't load album from TIDAL: ${err}`;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		tidalAlbumId;
		void load();
	});

	let header = $derived.by(() => {
		const first = tracks[0];
		if (!first) return null;
		const totalMs = tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0);
		return {
			title: first.album_title ?? 'Album',
			artist_name: first.artist_name ?? 'Unknown artist',
			artist_tidal_id: first.artist_tidal_id ?? null,
			artwork_url: firstArtworkUrl(tracks),
			track_count: tracks.length,
			total_ms: totalMs
		};
	});

	function goArtist() {
		if (!header?.artist_tidal_id) return;
		hapticTap();
		void goto(`/remote/tidal/artists/${header.artist_tidal_id}`);
	}
</script>

<svelte:head>
	<title>{header?.title ?? 'Album'} — NOOR Remote</title>
</svelte:head>

<RemotePageShell title={header?.title ?? ''}>
	{#if loading}
		<p class="remote-status" role="status" aria-live="polite">Loading from TIDAL…</p>
	{:else if error}
		<p class="remote-status remote-status-error">{error}</p>
	{:else if !header}
		<p class="remote-status">Album not found.</p>
	{:else}
		<section class="remote-album-hero">
			<div class="remote-album-art">
				<ArtworkImage
					className="remote-album-artwork"
					src={header.artwork_url}
					size={640}
					fallbackText={(header.title ?? 'A').slice(0, 1)}
					decorative={true}
				/>
			</div>
			<span class="remote-tidal-badge">TIDAL preview</span>
			<h2>{header.title}</h2>
			{#if header.artist_tidal_id}
				<button type="button" class="remote-album-artist" onclick={goArtist}>
					{header.artist_name}
				</button>
			{:else}
				<span class="remote-album-artist-static">{header.artist_name}</span>
			{/if}
			<small>{header.track_count} tracks · {formatTotalDuration(header.total_ms)}</small>
		</section>

		<RemoteActionBar
			disabled={tracks.length === 0}
			onPlay={() => playTidalTracksNow(tracks.map(toPlayable), header?.title ?? 'album')}
			onShuffle={() => shuffleTidalTracksNow(tracks.map(toPlayable), header?.title ?? 'album')}
		/>

		<section class="remote-section">
			<div class="remote-track-list">
				{#each tracks as track, i (track.tidal_id)}
					<RemoteTrackRow
						variant="tidal"
						{track}
						index={track.track_number ?? i + 1}
					/>
				{/each}
			</div>
		</section>
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

	.remote-album-hero {
		display: grid;
		gap: 6px;
		justify-items: center;
		padding: 4px 0;
		text-align: center;
	}

	.remote-album-art {
		width: 180px;
		height: 180px;
		border-radius: 12px;
		overflow: hidden;
		background: var(--surface-1);
		display: grid;
		place-items: center;
		color: var(--text-muted);
		box-shadow: 0 22px 44px rgba(0, 0, 0, 0.4);
	}

	.remote-album-art :global(.remote-album-artwork) {
		width: 100%;
		height: 100%;
	}

	.remote-album-art :global(img.remote-album-artwork) {
		object-fit: cover;
		display: block;
	}

	.remote-album-art :global(.remote-album-artwork.fallback) {
		display: grid;
		place-items: center;
	}

	.remote-album-art :global(.remote-album-artwork.fallback span) {
		font-size: var(--font-size-4xl);
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

	.remote-album-hero h2 {
		margin: 4px 0 0;
		font-size: var(--font-size-lg);
	}

	.remote-album-artist,
	.remote-album-artist-static {
		display: inline-block;
		max-width: 100%;
		padding: 2px 8px;
		border-radius: 6px;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-album-artist:active {
		background: var(--surface-1);
	}

	.remote-album-hero small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-section {
		display: grid;
		gap: 8px;
	}

	.remote-track-list {
		display: grid;
		gap: 2px;
	}
</style>
