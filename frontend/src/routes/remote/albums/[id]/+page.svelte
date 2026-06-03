<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { api, type Track } from '$lib/api/client';
	import {
		playAlbum,
		shuffleAlbum,
		startSongRadio
	} from '$lib/stores/player';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import RemoteActionBar from '$lib/components/remote/RemoteActionBar.svelte';
	import RemotePageShell from '$lib/components/remote/RemotePageShell.svelte';
	import RemoteTrackRow from '$lib/components/remote/RemoteTrackRow.svelte';
	import { hapticTap } from '$lib/remote/haptics';
	import { formatTotalDuration } from '$lib/utils/format';

	let albumId = $derived(Number(page.params.id));

	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	async function load() {
		loading = true;
		error = null;
		try {
			const res = await api.getAlbumTracks(albumId);
			tracks = res.tracks;
		} catch (err) {
			error = `Couldn't load album: ${err}`;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		albumId;
		void load();
	});

	let header = $derived.by(() => {
		const first = tracks[0];
		if (!first) return null;
		const totalMs = tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0);
		return {
			title: first.album_title ?? 'Album',
			artist_name: first.artist_name ?? 'Unknown artist',
			artist_id: first.artist_id ?? null,
			artwork_url: first.artwork_url,
			track_count: tracks.length,
			total_ms: totalMs
		};
	});

	function goArtist() {
		if (!header?.artist_id || header.artist_id <= 0) return;
		hapticTap();
		void goto(`/remote/artists/${header.artist_id}`);
	}

	function onRadio() {
		if (tracks.length === 0) return;
		void startSongRadio(tracks[0].id);
	}
</script>

<svelte:head>
	<title>{header?.title ?? 'Album'} — NOOR Remote</title>
</svelte:head>

<RemotePageShell title={header?.title ?? ''}>
	{#if loading}
		<p class="remote-status" role="status" aria-live="polite">Loading…</p>
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
			<h2>{header.title}</h2>
			{#if header.artist_id && header.artist_id > 0}
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
			onPlay={() => playAlbum(albumId)}
			onShuffle={() => shuffleAlbum(albumId)}
			onRadio={onRadio}
		/>

		<section class="remote-section">
			<div class="remote-track-list">
				{#each tracks as track, i (track.id)}
					<RemoteTrackRow
						{track}
						albumIdForPlay={albumId}
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

	.remote-album-hero h2 {
		margin: 8px 0 0;
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
