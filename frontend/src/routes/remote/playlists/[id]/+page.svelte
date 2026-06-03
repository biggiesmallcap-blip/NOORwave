<script lang="ts">
	import { page } from '$app/state';
	import { api, type Playlist, type Track } from '$lib/api/client';
	import {
		playPlaylist,
		shufflePlaylist,
		startPlaylistRadio
	} from '$lib/stores/player';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import RemoteActionBar from '$lib/components/remote/RemoteActionBar.svelte';
	import RemotePageShell from '$lib/components/remote/RemotePageShell.svelte';
	import RemoteTrackRow from '$lib/components/remote/RemoteTrackRow.svelte';

	let playlistId = $derived(Number(page.params.id));

	let playlist = $state<Playlist | null>(null);
	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	async function load() {
		loading = true;
		error = null;
		try {
			const [listsRes, tracksRes] = await Promise.all([
				api.getPlaylists(),
				api.getPlaylistTracks(playlistId)
			]);
			playlist = listsRes.playlists.find((p) => p.id === playlistId) ?? null;
			tracks = tracksRes.tracks;
		} catch (err) {
			error = `Couldn't load playlist: ${err}`;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		playlistId;
		void load();
	});

	let totalMs = $derived(tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0));
	let countLine = $derived.by(() => {
		const minutes = Math.round(totalMs / 60000);
		const total =
			minutes < 60
				? `${minutes} min`
				: (() => {
						const h = Math.floor(minutes / 60);
						const r = minutes % 60;
						return r ? `${h} hr ${r} min` : `${h} hr`;
					})();
		return `${tracks.length} tracks · ${total}`;
	});
</script>

<svelte:head>
	<title>{playlist?.name ?? 'Playlist'} — NOOR Remote</title>
</svelte:head>

<RemotePageShell title={playlist?.name ?? ''}>
	{#if loading}
		<p class="remote-status" role="status" aria-live="polite">Loading…</p>
	{:else if error}
		<p class="remote-status remote-status-error">{error}</p>
	{:else}
		<section class="remote-playlist-hero">
			<div class="remote-playlist-art">
				<ArtworkImage
					className="remote-playlist-artwork"
					src={tracks[0]?.artwork_url ?? null}
					size={640}
					fallbackText={playlist?.name?.charAt(0) ?? 'P'}
					decorative={true}
				/>
			</div>
			<h2>{playlist?.name ?? 'Playlist'}</h2>
			{#if playlist?.description}
				<p class="remote-playlist-desc">{playlist.description}</p>
			{/if}
			<small>{countLine}</small>
		</section>

		<RemoteActionBar
			disabled={tracks.length === 0}
			onPlay={() => playPlaylist(playlistId)}
			onShuffle={() => shufflePlaylist(tracks)}
			onRadio={() => startPlaylistRadio(tracks)}
		/>

		<section class="remote-section">
			<div class="remote-track-list">
				{#each tracks as track (track.id)}
					<RemoteTrackRow {track} />
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

	.remote-playlist-hero {
		display: grid;
		gap: 6px;
		justify-items: center;
		padding: 4px 0;
		text-align: center;
	}

	.remote-playlist-art {
		width: 180px;
		height: 180px;
		border-radius: 12px;
		overflow: hidden;
		background: linear-gradient(135deg, var(--surface-2), var(--surface-1));
		display: grid;
		place-items: center;
		color: var(--text-muted);
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-semibold);
		box-shadow: 0 22px 44px rgba(0, 0, 0, 0.4);
	}

	.remote-playlist-art :global(.remote-playlist-artwork) {
		width: 100%;
		height: 100%;
	}

	.remote-playlist-art :global(img.remote-playlist-artwork) {
		object-fit: cover;
		display: block;
	}

	.remote-playlist-art :global(.remote-playlist-artwork.fallback) {
		display: grid;
		place-items: center;
	}

	.remote-playlist-art :global(.remote-playlist-artwork.fallback span) {
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-semibold);
	}

	.remote-playlist-hero h2 {
		margin: 8px 0 0;
		font-size: var(--font-size-lg);
	}

	.remote-playlist-desc {
		margin: 0;
		max-width: 32ch;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	.remote-playlist-hero small {
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
