<script lang="ts">
	import type { ExploreVideo, VideoConcert } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { videoSession } from '$lib/stores/video_session';
	import { concertLength, runtimeLabel } from './versions';
	import { playConcert } from './play';

	// A reconstructed concert: its setlist in order and Play concert, which
	// queues the whole show and runs it through. A row plays from that song
	// with the rest of the show still queued around it.

	let { concert, onnavigate }: { concert: VideoConcert; onnavigate?: () => void } = $props();

	let playingId = $derived($videoSession.current?.tidal_id ?? null);

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}

	function artistMenu(event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		const name = concert.artist_name ?? 'Artist';
		openContextMenu(event, buildArtistMenu({ tidal_id: concert.artist_id, name }, { isLocal: false }), name);
	}
</script>

<div class="concert">
	<div class="concert-head">
		<div class="concert-copy">
			<span class="t-label">Concert{concert.year ? `, ${concert.year}` : ''}</span>
			<h3 class="concert-title">{concert.label}</h3>
			<p class="t-meta">
				{#if concert.artist_name}
					<a href={`/tidal/artists/${concert.artist_id}`} oncontextmenu={artistMenu} onclick={() => onnavigate?.()}>{concert.artist_name}</a>
					&middot;
				{/if}
				{concert.video_count} {concert.video_count === 1 ? 'song' : 'songs'}
				{#if concert.total_s > 0}&middot; {concertLength(concert.total_s)}{/if}
			</p>
		</div>
		<button type="button" class="btn btn-primary" onclick={() => void playConcert(concert)}>
			<svg viewBox="0 0 24 24" width="14" height="14" aria-hidden="true"><path d="M7 4.5v15l12.5-7.5z" fill="currentColor" /></svg>
			Play concert
		</button>
	</div>

	<ol class="setlist">
		{#each concert.videos as video, index (video.tidal_id)}
			<li>
				<button
					type="button"
					class="song"
					class:playing={playingId === video.tidal_id}
					onclick={() => void playConcert(concert, video.tidal_id)}
					oncontextmenu={(event) => videoMenu(event, video)}
					aria-label={`Play ${video.title} from the concert`}
				>
					<span class="n">{index + 1}</span>
					<span class="thumb">
						<ArtworkImage className="thumb-img" src={video.artwork_url} size={160} fallbackText="VID" decorative={true} />
					</span>
					<span class="song-title">{video.title}</span>
					<span class="time">{runtimeLabel(video.duration_ms)}</span>
				</button>
			</li>
		{/each}
	</ol>
</div>

<style>
	.concert {
		display: grid;
		gap: var(--space-4);
	}

	.concert-head {
		display: flex;
		align-items: flex-end;
		justify-content: space-between;
		gap: var(--space-4);
		flex-wrap: wrap;
	}

	.concert-copy {
		display: grid;
		gap: 4px;
		min-width: 0;
	}

	.concert-title {
		margin: 0;
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		text-wrap: balance;
	}

	.concert-copy p {
		margin: 0;
		white-space: normal;
	}

	.concert-copy a {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
	}

	.concert-copy a:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.btn-primary {
		display: inline-flex;
		align-items: center;
		gap: 8px;
	}

	.setlist {
		display: grid;
		gap: var(--row-gap);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.song {
		display: grid;
		grid-template-columns: 28px 80px minmax(0, 1fr) 56px;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		min-height: 52px;
		padding: 4px 8px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.song:hover,
	.song:focus-visible {
		background: var(--bg-hover);
	}

	.song:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.song.playing {
		box-shadow: inset 3px 0 0 var(--accent);
	}

	.song.playing .song-title,
	.song.playing .n {
		color: var(--accent-strong);
	}

	.n,
	.time {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.n {
		text-align: right;
	}

	.time {
		text-align: right;
	}

	.thumb {
		position: relative;
		width: 80px;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-xs);
		overflow: hidden;
		background: var(--bg-raised);
	}

	.thumb :global(.thumb-img) {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
		font-size: var(--font-size-2xs);
	}

	.thumb :global(.thumb-img.fallback) {
		display: grid;
		place-items: center;
	}

	.song-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
</style>
