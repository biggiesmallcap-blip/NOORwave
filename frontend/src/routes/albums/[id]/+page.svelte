<script lang="ts">
	import { page } from '$app/state';
	import { api, type Track } from '$lib/api/client';
	import {
		playAlbum,
		shuffleAlbum,
		startAlbumRadio,
		currentTrack,
		isPlaying,
		togglePlayback
	} from '$lib/stores/player';
	import TrackRow from '$lib/components/TrackRow.svelte';

	let albumId = $derived(Number(page.params.id));

	let tracks = $state<Track[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

	let artistTracks = $state<Track[]>([]);
	let moreLoading = $state(false);
	let moreLoaded = $state(false);

	async function load() {
		loading = true;
		error = null;
		try {
			const res = await api.getAlbumTracks(albumId);
			tracks = res.tracks;
		} catch (err) {
			error = `Failed to load album: ${err}`;
		} finally {
			loading = false;
		}
	}

	$effect(() => {
		albumId;
		void load();
		artistTracks = [];
		moreLoaded = false;
		moreLoading = false;
	});

	$effect(() => {
		const artistId = tracks[0]?.artist_id;
		if (artistId != null && !moreLoaded && !moreLoading) {
			void loadMore(artistId);
		}
	});

	async function loadMore(artistId: number) {
		moreLoading = true;
		try {
			const res = await api.getArtistTracks(artistId);
			artistTracks = res.tracks;
			moreLoaded = true;
		} catch (err) {
			console.error('Failed to load artist tracks', err);
		} finally {
			moreLoading = false;
		}
	}

	let header = $derived(() => {
		const first = tracks[0];
		if (!first) return null;
		const totalMs = tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0);
		return {
			title: first.album_title ?? 'Unknown album',
			artist_name: first.artist_name ?? 'Unknown artist',
			artist_id: first.artist_id,
			artwork_url: first.artwork_url,
			track_count: tracks.length,
			total_ms: totalMs
		};
	});

	let otherAlbums = $derived.by(() => {
		const map = new Map<
			number,
			{ id: number; title: string; artwork_url: string | null; count: number }
		>();
		for (const t of artistTracks) {
			if (t.album_id == null || t.album_id === albumId) continue;
			const existing = map.get(t.album_id);
			if (existing) {
				existing.count += 1;
			} else {
				map.set(t.album_id, {
					id: t.album_id,
					title: t.album_title ?? 'Album',
					artwork_url: t.artwork_url,
					count: 1
				});
			}
		}
		return Array.from(map.values()).sort((a, b) => b.count - a.count).slice(0, 8);
	});

	function formatTotalDuration(ms: number): string {
		const totalSeconds = Math.round(ms / 1000);
		const minutes = Math.floor(totalSeconds / 60);
		if (minutes < 60) return `${minutes} min`;
		const hours = Math.floor(minutes / 60);
		const remaining = minutes % 60;
		return remaining ? `${hours} hr ${remaining} min` : `${hours} hr`;
	}

	function onRowClick(track: Track) {
		void playAlbum(albumId, track.id);
	}

	function onHeroPlay() {
		const current = $currentTrack;
		if (current && tracks.some((t) => t.id === current.id)) {
			void togglePlayback();
		} else {
			void playAlbum(albumId);
		}
	}

	let isAlbumPlaying = $derived(
		$isPlaying && tracks.some((t) => t.id === $currentTrack?.id)
	);
</script>

<div class="album-page">
	{#if loading}
		<p class="status">Loading album…</p>
	{:else if error}
		<p class="status error">{error}</p>
	{:else if !header()}
		<p class="status">Album not found.</p>
	{:else}
		{@const h = header()!}

		<header class="hero">
			{#if h.artwork_url}
				<div class="hero-backdrop" style="background-image: url({h.artwork_url});"></div>
			{/if}
			<div class="hero-veil"></div>

			<div class="hero-body">
				<div class="hero-art-wrap">
					{#if h.artwork_url}
						<img class="hero-art" src={h.artwork_url} alt="" />
					{:else}
						<div class="hero-art placeholder">♫</div>
					{/if}
				</div>
				<div class="hero-info">
					<p class="eyebrow">Album</p>
					<h1 class="hero-title">{h.title}</h1>
					<p class="hero-sub">
						<a href="/artists/{h.artist_id}" class="hero-link">{h.artist_name}</a>
						<span class="dot">·</span>
						<span>{h.track_count} {h.track_count === 1 ? 'song' : 'songs'}</span>
						<span class="dot">·</span>
						<span class="hero-duration">{formatTotalDuration(h.total_ms)}</span>
					</p>
				</div>
			</div>
		</header>

		<div class="actions-bar">
			<button
				class="play-fab"
				aria-label={isAlbumPlaying ? 'Pause' : 'Play album'}
				onclick={onHeroPlay}
			>
				{#if isAlbumPlaying}
					<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1" fill="currentColor"/><rect x="14" y="5" width="4" height="14" rx="1" fill="currentColor"/></svg>
				{:else}
					<svg viewBox="0 0 24 24" width="24" height="24" aria-hidden="true"><path d="M8 5.5v13a1 1 0 001.5.87l11-6.5a1 1 0 000-1.74l-11-6.5A1 1 0 008 5.5z" fill="currentColor"/></svg>
				{/if}
			</button>

			<button class="ghost-btn" aria-label="Shuffle" onclick={() => void shuffleAlbum(albumId)}>
				<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><path d="M16 3h5v5M4 20l17-17M21 16v5h-5M4 4l5 5m6 6l6 6" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>
			</button>

			<button
				class="ghost-btn"
				aria-label="Album radio"
				onclick={() => void startAlbumRadio(albumId)}
			>
				<svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true"><circle cx="12" cy="12" r="3" fill="currentColor"/><path d="M8.5 8.5a5 5 0 000 7M15.5 8.5a5 5 0 010 7M5.5 5.5a9 9 0 000 13M18.5 5.5a9 9 0 010 13" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg>
			</button>

			<span class="actions-spacer"></span>

			<span class="actions-hint">Click a song to start the album from there</span>
		</div>

		<p class="actions-microcopy">
			<strong>Shuffle</strong> plays this album in random order.
			<strong>Radio</strong> finds similar tracks across your library and Tidal.
		</p>

		<section class="track-table">
			<div class="track-header">
				<span class="col-num">#</span>
				<span class="col-title">Title</span>
				<span class="col-plays">Plays</span>
				<span class="col-duration"><svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2" fill="none"/><path d="M12 7v5l3 2" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg></span>
			</div>
			<ol class="track-list">
				{#each tracks as track, idx (track.id)}
					<TrackRow
						{track}
						variant="indexed"
						index={idx}
						isCurrent={$currentTrack?.id === track.id}
						isPlaying={$isPlaying}
						showAlbum={false}
						showPlayCount={true}
						onRowClick={() => onRowClick(track)}
						menuOptions={{ hideAlbumActions: true }}
					/>
				{/each}
			</ol>
		</section>

		<p class="footnote">{h.artist_name}</p>

		{#if otherAlbums.length > 0}
			<section class="more-section">
				<div class="more-head">
					<h2 class="more-title">More by {h.artist_name}</h2>
					{#if h.artist_id != null}
						<a class="show-all" href="/artists/{h.artist_id}">Show all</a>
					{/if}
				</div>
				<div class="album-grid">
					{#each otherAlbums as album (album.id)}
						<a class="album-card" href="/albums/{album.id}">
							<div class="album-card-art-wrap">
								{#if album.artwork_url}
									<img class="album-card-art" src={album.artwork_url} alt="" />
								{:else}
									<div class="album-card-art placeholder">♫</div>
								{/if}
							</div>
							<p class="album-card-title">{album.title}</p>
							<p class="album-card-sub">
								{album.count} {album.count === 1 ? 'track' : 'tracks'}
							</p>
						</a>
					{/each}
				</div>
			</section>
		{/if}
	{/if}
</div>

<style>
	.album-page {
		padding: 0 0 80px;
		display: flex;
		flex-direction: column;
	}

	.status {
		padding: 48px 28px;
		text-align: center;
		color: var(--text-secondary);
	}
	.status.error { color: var(--danger, #f87171); }

	.hero {
		position: relative;
		padding: 40px 32px 28px;
		display: flex;
		min-height: 340px;
		overflow: hidden;
		isolation: isolate;
	}

	.hero-backdrop {
		position: absolute;
		inset: -60px;
		background-size: cover;
		background-position: center;
		filter: blur(60px) saturate(1.6);
		transform: scale(1.2);
		z-index: -2;
		opacity: 0.7;
	}

	.hero-veil {
		position: absolute;
		inset: 0;
		background:
			linear-gradient(180deg, rgba(0,0,0,0.08) 0%, rgba(0,0,0,0.42) 68%, var(--bg-base) 100%);
		z-index: -1;
	}

	.hero-body {
		display: grid;
		grid-template-columns: 232px 1fr;
		gap: 28px;
		align-items: end;
		width: 100%;
		max-width: 1400px;
	}

	.hero-art-wrap {
		width: 232px;
		height: 232px;
		border-radius: 8px;
		overflow: hidden;
		box-shadow: 0 28px 70px -14px rgba(0, 0, 0, 0.7);
		background: var(--bg-surface);
	}

	.hero-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.hero-art.placeholder {
		display: grid;
		place-items: center;
		font-size: 3rem;
		color: var(--text-tertiary);
	}

	.hero-info {
		display: flex;
		flex-direction: column;
		gap: 10px;
		min-width: 0;
	}

	.eyebrow {
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.14em;
		color: var(--text-primary);
		margin: 0;
		font-weight: 600;
	}

	.hero-title {
		font-family: var(--font-display);
		font-size: clamp(2.4rem, 5vw, 4.4rem);
		line-height: 1.02;
		letter-spacing: -0.02em;
		margin: 0;
		color: var(--text-primary);
		word-wrap: break-word;
	}

	.hero-sub {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
		color: var(--text-secondary);
		margin: 4px 0 0;
		font-size: 0.88rem;
	}

	.hero-link {
		color: var(--text-primary);
		font-weight: 700;
		text-decoration: none;
	}
	.hero-link:hover { text-decoration: underline; }
	.dot { opacity: 0.5; }
	.hero-duration { color: var(--text-tertiary); }

	.actions-bar {
		display: flex;
		align-items: center;
		gap: 14px;
		padding: 18px 32px 4px;
	}

	.play-fab {
		all: unset;
		width: 56px;
		height: 56px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent);
		color: #fff;
		cursor: pointer;
		transition: transform var(--motion-fast), background var(--motion-fast), box-shadow var(--motion-fast);
		box-shadow: 0 8px 24px -8px var(--accent-glow);
	}

	.play-fab:hover {
		transform: scale(1.06);
		background: var(--accent-strong);
	}

	.play-fab:active { transform: scale(0.98); }

	.ghost-btn {
		all: unset;
		width: 40px;
		height: 40px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		color: var(--text-secondary);
		cursor: pointer;
		transition: color var(--motion-fast), background var(--motion-fast);
	}

	.ghost-btn:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.actions-spacer { flex: 1; }

	.actions-hint {
		color: var(--text-tertiary);
		font-size: 0.78rem;
	}

	.actions-microcopy {
		margin: 0;
		padding: 0 32px 8px;
		color: var(--text-tertiary);
		font-size: 0.78rem;
		line-height: 1.4;
	}

	.actions-microcopy strong {
		color: var(--text-secondary);
		font-weight: 600;
	}

	.track-table {
		padding: 8px 32px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.track-header {
		display: grid;
		grid-template-columns: 40px 1fr 80px auto 64px;
		align-items: center;
		gap: 14px;
		padding: 6px 16px 10px;
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		font-size: 0.74rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-weight: 600;
	}

	.col-num { text-align: center; }
	.col-plays { text-align: right; }
	.col-duration { display: grid; place-items: center; }

	.track-list {
		list-style: none;
		margin: 0;
		padding: 6px 0 0;
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	.footnote {
		padding: 22px 32px 4px;
		color: var(--text-tertiary);
		font-size: 0.78rem;
		margin: 0;
	}

	.more-section {
		padding: 28px 32px 0;
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.more-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
	}

	.more-title {
		font-family: var(--font-display);
		font-size: 1.4rem;
		margin: 0;
		letter-spacing: -0.01em;
	}

	.show-all {
		color: var(--text-secondary);
		font-size: 0.78rem;
		text-transform: uppercase;
		letter-spacing: 0.1em;
		text-decoration: none;
		font-weight: 600;
	}
	.show-all:hover { color: var(--text-primary); text-decoration: underline; }

	.album-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(170px, 1fr));
		gap: 22px;
	}

	.album-card {
		display: flex;
		flex-direction: column;
		gap: 4px;
		padding: 14px;
		border-radius: 10px;
		text-decoration: none;
		color: inherit;
		transition: background var(--motion-fast);
	}

	.album-card:hover {
		background: var(--bg-hover);
	}

	.album-card-art-wrap {
		width: 100%;
		aspect-ratio: 1/1;
		border-radius: 6px;
		overflow: hidden;
		box-shadow: 0 10px 24px -12px rgba(0, 0, 0, 0.6);
		background: var(--bg-surface);
		margin-bottom: 6px;
	}

	.album-card-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.album-card-art.placeholder {
		display: grid;
		place-items: center;
		font-size: 2rem;
		color: var(--text-tertiary);
	}

	.album-card-title {
		margin: 6px 0 0;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.album-card-sub {
		margin: 0;
		font-size: 0.8rem;
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	@media (max-width: 720px) {
		.hero { padding: 24px 20px 20px; min-height: auto; }
		.hero-body { grid-template-columns: 1fr; gap: 18px; }
		.hero-art-wrap { width: 180px; height: 180px; }
		.hero-title { font-size: 2rem; }
		.actions-bar { padding: 12px 20px; }
		.track-table { padding: 8px 12px 0; }
		.track-header { grid-template-columns: 36px 1fr auto 56px; }
		.col-plays { display: none; }
		.more-section { padding: 24px 20px 0; }
	}
</style>
