<script lang="ts">
	import { page } from '$app/state';
	import { api, type TidalDiscographyTrack } from '$lib/api/client';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { playTidalTrackNow, playTidalAlbum } from '$lib/stores/player';
	import { formatDuration } from '$lib/stores/library';

	function trackAsPlayable(t: TidalDiscographyTrack) {
		return {
			tidal_id: t.tidal_id,
			title: t.title,
			artist_name: t.artist_name ?? null,
			album_title: t.album_title ?? null,
			artwork_url: t.artwork_url,
			duration_ms: t.duration_ms,
			artist_tidal_id: t.artist_tidal_id ?? null,
		};
	}

	let tidalAlbumId = $derived(Number(page.params.id));

	let tracks = $state<TidalDiscographyTrack[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);

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

	let header = $derived(() => {
		const first = tracks[0];
		if (!first) return null;
		const totalMs = tracks.reduce((sum, t) => sum + (t.duration_ms ?? 0), 0);
		return {
			title: first.album_title ?? 'Album',
			artist_name: first.artist_name ?? 'Unknown artist',
			artwork_url: first.artwork_url,
			track_count: tracks.length,
			total_ms: totalMs
		};
	});

	function formatTotalDuration(ms: number): string {
		const minutes = Math.round(ms / 60000);
		if (minutes < 60) return `${minutes} min`;
		const hours = Math.floor(minutes / 60);
		const rem = minutes % 60;
		return rem ? `${hours} hr ${rem} min` : `${hours} hr`;
	}
</script>

<div class="page">
	{#if loading}
		<p class="status">Loading from TIDAL…</p>
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
					<p class="eyebrow">Album · TIDAL preview</p>
					<h1 class="hero-title">{h.title}</h1>
					<p class="hero-sub">
						<span class="hero-link">{h.artist_name}</span>
						<span class="dot">·</span>
						<span>{h.track_count} songs</span>
						<span class="dot">·</span>
						<span>{formatTotalDuration(h.total_ms)}</span>
					</p>
					<div class="hero-actions">
						<button class="play-all-btn" onclick={() => playTidalAlbum(tidalAlbumId)}>▶ Play All</button>
						<span class="not-in-library-badge">Not in your library</span>
					</div>
				</div>
			</div>
		</header>

		<section class="track-table">
			<div class="track-header">
				<span class="col-num">#</span>
				<span class="col-title">Title</span>
				<span class="col-duration"><svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2" fill="none"/><path d="M12 7v5l3 2" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg></span>
				<span></span>
				<span></span>
			</div>
			<ol class="track-list">
				{#each tracks as track, idx (track.tidal_id)}
					<li
						class="track-row"
						ondblclick={() => playTidalTrackNow(trackAsPlayable(track))}
						oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTidalTrackMenu(trackAsPlayable(track))) }}
					>
						<span class="track-index">{track.track_number ?? idx + 1}</span>
						<div class="track-meta">
							<p class="track-title">{track.title}</p>
							<span class="track-artist">{track.artist_name}</span>
						</div>
						<span class="track-duration">{formatDuration(track.duration_ms)}</span>
						<button
							class="row-btn"
							onclick={() => playTidalTrackNow(trackAsPlayable(track))}
							aria-label="Play {track.title}"
						>▶</button>
						<button
							class="row-btn"
							onclick={(e) => { e.stopPropagation(); openContextMenu(e, buildTidalTrackMenu(trackAsPlayable(track))) }}
							aria-label="More options"
						>⋯</button>
					</li>
				{/each}
			</ol>
		</section>
	{/if}
</div>

<style>
	.page {
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
		background: linear-gradient(180deg, rgba(0,0,0,0.08) 0%, rgba(0,0,0,0.42) 68%, var(--bg-base) 100%);
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
		font-size: clamp(2.4rem, 5vw, 4rem);
		line-height: 1.02;
		letter-spacing: -0.02em;
		margin: 0;
		color: var(--text-primary);
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

	.hero-link { color: var(--text-primary); font-weight: 700; }
	.dot { opacity: 0.5; }

	.hero-actions {
		display: flex;
		align-items: center;
		gap: 12px;
		margin-top: 10px;
		flex-wrap: wrap;
	}

	.play-all-btn {
		background: var(--accent);
		color: #fff;
		border: none;
		border-radius: 20px;
		padding: 8px 22px;
		font-size: 0.88rem;
		font-weight: 600;
		cursor: pointer;
		transition: opacity 0.15s;
	}
	.play-all-btn:hover { opacity: 0.85; }

	.not-in-library-badge {
		font-size: 0.75rem;
		color: var(--text-tertiary);
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		border-radius: 12px;
		padding: 3px 10px;
	}

	.track-table {
		padding: 24px 32px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.track-header {
		display: grid;
		grid-template-columns: 40px 1fr 64px 32px 32px;
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
	.col-duration { display: grid; place-items: center; }

	.track-list {
		list-style: none;
		margin: 0;
		padding: 6px 0 0;
		display: flex;
		flex-direction: column;
		gap: 0;
	}

	.track-row {
		display: grid;
		grid-template-columns: 40px 1fr 64px 32px 32px;
		align-items: center;
		gap: 14px;
		padding: 10px 16px;
		border-radius: 6px;
	}

	.track-index {
		text-align: center;
		color: var(--text-secondary);
		font-variant-numeric: tabular-nums;
		font-size: 0.9rem;
	}

	.track-meta {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.track-title {
		margin: 0;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.track-artist {
		color: var(--text-secondary);
		font-size: 0.82rem;
	}

	.track-duration {
		color: var(--text-secondary);
		font-size: 0.82rem;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	.track-row:hover { background: var(--bg-hover); cursor: pointer; }

	.row-btn {
		background: none;
		border: none;
		color: var(--text-tertiary);
		cursor: pointer;
		font-size: 13px;
		padding: 4px;
		border-radius: 4px;
		opacity: 0;
		transition: opacity 0.1s, color 0.1s;
	}
	.track-row:hover .row-btn { opacity: 1; }
	.row-btn:hover { color: var(--text-primary); }
</style>
