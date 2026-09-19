<script lang="ts">
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { api, type TidalDiscographyTrack } from '$lib/api/client';
	import {
		playTidalTracksNow,
		shuffleTidalTracksNow,
		startTidalSongRadio,
		saveTidalAlbumToLibrary,
	} from '$lib/stores/player';
	import { goBack } from '$lib/navigation/back';
	import TidalTrackRow from '$lib/components/TidalTrackRow.svelte';
	import DetailHero from '$lib/components/ui/DetailHero.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { firstArtworkUrl } from '$lib/utils/artwork';
	import { formatTotalDuration } from '$lib/utils/format';
	import { tidalDiscographyTrackToPlayable } from '$lib/utils/track';

	let tidalAlbumId = $derived(Number(page.params.id));

	let tracks = $state<TidalDiscographyTrack[]>([]);
	let loading = $state(true);
	let error = $state<string | null>(null);
	let loadSeq = 0;

	async function load(id: number) {
		const seq = ++loadSeq;
		loading = true;
		error = null;
		try {
			const res = await api.getTidalAlbumTracks(id);
			if (seq !== loadSeq) return;
			tracks = res.tracks;
		} catch (err) {
			if (seq !== loadSeq) return;
			error = `Couldn't load album from TIDAL: ${err}`;
		} finally {
			if (seq === loadSeq) loading = false;
		}
	}

	$effect(() => {
		const id = tidalAlbumId;
		void load(id);
	});

	let header = $derived(() => {
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

	async function playLoadedAlbum(startIndex = 0) {
		await playTidalTracksNow(
			tracks.map((track) => tidalDiscographyTrackToPlayable(track)),
			header()?.title ?? 'album',
			{ startIndex },
		);
	}

	async function shuffleLoadedAlbum() {
		await shuffleTidalTracksNow(
			tracks.map((track) => tidalDiscographyTrackToPlayable(track)),
			header()?.title ?? 'album',
		);
	}

	let radioPending = $state(false);
	async function radioFromAlbum() {
		const first = tracks[0];
		if (radioPending || !first) return;
		radioPending = true;
		try {
			await startTidalSongRadio(tidalDiscographyTrackToPlayable(first));
		} finally {
			radioPending = false;
		}
	}

	let savePending = $state(false);
	async function saveToLibrary() {
		if (savePending) return;
		savePending = true;
		try {
			// Import brings every track into the library and returns the new
			// local album id, so land the user on the now-library album page.
			const localId = await saveTidalAlbumToLibrary(tidalAlbumId);
			if (localId != null) await goto(`/albums/${localId}`);
		} finally {
			savePending = false;
		}
	}

</script>

<div class="page">
	<button class="back-link" type="button" onclick={() => goBack('/search')}>Back</button>
	{#if loading}
		<p class="status" role="status" aria-live="polite">Loading from TIDAL…</p>
	{:else if error}
		<p class="status error">{error}</p>
	{:else if !header()}
		<p class="status" role="status" aria-live="polite">Album not found.</p>
	{:else}
		{@const h = header()!}

		<DetailHero
			eyebrow="Album · TIDAL preview"
			title={h.title}
			artwork={h.artwork_url}
			backdrop={h.artwork_url}
			fallbackText={h.title.slice(0, 1)}
			variant="immersive"
		>
			{#snippet meta()}
						{#if h.artist_tidal_id != null}
							<a
								class="hero-link"
								href="/tidal/artists/{h.artist_tidal_id}"
								oncontextmenu={(e) => {
									e.preventDefault();
									e.stopPropagation();
									openContextMenu(e, buildArtistMenu({
										tidal_id: h.artist_tidal_id,
										name: h.artist_name,
									}, { isLocal: false }), h.artist_name);
								}}
							>{h.artist_name}</a>
						{:else}
							<span class="hero-link">{h.artist_name}</span>
						{/if}
						<span class="dot">·</span>
						<span>{h.track_count} songs</span>
						<span class="dot">·</span>
						<span>{formatTotalDuration(h.total_ms)}</span>
			{/snippet}
			{#snippet actions()}
						<button class="play-all-btn" onclick={() => void playLoadedAlbum()}>▶ Play All</button>
						<button class="action-btn" onclick={() => void shuffleLoadedAlbum()}>⤮ Shuffle</button>
						<button class="action-btn" disabled={radioPending} onclick={() => void radioFromAlbum()}>◉ Radio</button>
						<button class="save-btn" disabled={savePending} onclick={() => void saveToLibrary()}>
							{savePending ? 'Saving…' : '＋ Save to library'}
						</button>
			{/snippet}
		</DetailHero>

		<section class="track-table">
			<div class="track-header">
				<span class="col-num">#</span>
				<span class="col-title">Title</span>
				<span class="col-duration"><svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true"><circle cx="12" cy="12" r="9" stroke="currentColor" stroke-width="2" fill="none"/><path d="M12 7v5l3 2" stroke="currentColor" stroke-width="2" fill="none" stroke-linecap="round"/></svg></span>
				<span></span>
			</div>
			<ol class="track-list">
				{#each tracks as track, idx (track.tidal_id)}
					<TidalTrackRow
						track={tidalDiscographyTrackToPlayable(track)}
						variant="indexed"
						index={idx}
						showAlbum={false}
						onRowClick={() => void playLoadedAlbum(idx)}
					/>
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

	.page > .back-link {
		align-self: flex-start;
		margin-bottom: var(--space-3);
	}

	.status {
		padding: var(--space-7) var(--space-6);
		text-align: center;
		color: var(--text-secondary);
	}
	.status.error { color: var(--state-error); }

	.hero-link { color: var(--text-primary); font-weight: var(--font-weight-bold); }
	.dot { opacity: 0.5; }

	.play-all-btn {
		background: var(--accent);
		color: #fff;
		border: none;
		border-radius: 20px;
		padding: 8px 22px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: opacity 0.15s;
	}
	.play-all-btn:hover { opacity: 0.85; }

	.action-btn {
		background: var(--bg-surface);
		color: var(--text-secondary);
		border: 1px solid var(--border-subtle);
		border-radius: 20px;
		padding: 8px 18px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: color 0.15s, background 0.15s, border-color 0.15s;
	}
	.action-btn:hover { color: var(--text-primary); background: var(--bg-hover); }
	.action-btn:disabled { cursor: progress; opacity: 0.7; }

	.save-btn {
		background: var(--accent-soft);
		color: var(--accent-strong);
		border: 1px solid var(--accent-line);
		border-radius: 20px;
		padding: 8px 18px;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: background 0.15s, color 0.15s;
	}
	.save-btn:hover { background: var(--accent); color: #fff; }
	.save-btn:disabled { cursor: progress; opacity: 0.85; }

	.track-table {
		padding: var(--space-5) var(--space-6) 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.track-header {
		display: grid;
		grid-template-columns: 40px 1fr 64px auto;
		align-items: center;
		gap: var(--gap);
		padding: var(--space-2) var(--space-4) var(--space-3);
		border-bottom: 1px solid var(--border-subtle);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		font-weight: var(--font-weight-semibold);
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
</style>
