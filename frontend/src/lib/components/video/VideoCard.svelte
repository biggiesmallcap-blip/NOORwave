<script lang="ts">
	import type { TidalSearchVideo, TidalVideoMix, TidalVideoMixItem } from '$lib/api/client';
	import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatDuration } from '$lib/stores/library';
	import { buildVideoMenu, buildVideoMixMenu, isVideoMix } from '$lib/player/video_menu';

	type VideoLike = TidalSearchVideo | TidalVideoMix | TidalVideoMixItem;

	let { video, onSelect }: { video: VideoLike; onSelect?: (video: VideoLike) => void } = $props();

	let lazyArtwork = $state<string | null>(null);
	let poster = $derived(('artwork_url' in video ? video.artwork_url : null) ?? lazyArtwork);
	let title = $derived(video.title);
	let subtitle = $derived(
		isVideoMix(video) ? (video.description ?? 'Video mix') : (video.artist_name ?? 'TIDAL video')
	);
	let duration = $derived(!isVideoMix(video) && video.duration_ms ? formatDuration(video.duration_ms) : null);

	function select() {
		onSelect?.(video);
	}

	function menu(event: MouseEvent) {
		openContextMenu(
			event,
			isVideoMix(video) ? buildVideoMixMenu(video) : buildVideoMenu(video),
			title
		);
	}
</script>

<button
	type="button"
	class="video-card glass-tile"
	onclick={select}
	oncontextmenu={menu}
	aria-label={isVideoMix(video) ? `Open ${title}` : `Play ${title}`}
	use:lazyTidalArt={{
		enabled: !poster && !isVideoMix(video),
		query: { artist: !isVideoMix(video) ? video.artist_name : null, title },
		onResolve: (url) => (lazyArtwork = url),
	}}
>
	<div class="poster-wrap">
		{#if poster}
			<img class="poster" src={poster} alt="" loading="lazy" />
		{:else}
			<div class="poster placeholder">▶</div>
		{/if}
		<div class="play-overlay" aria-hidden="true">{isVideoMix(video) ? '↗' : '▶'}</div>
		{#if duration}
			<span class="duration">{duration}</span>
		{/if}
	</div>
	<div class="meta">
		<span class="title">{title}</span>
		<span class="subtitle">{subtitle}</span>
	</div>
</button>

<style>
	.video-card {
		width: 100%;
		display: grid;
		gap: 10px;
		padding: 10px;
		text-align: left;
		transition: transform 0.18s ease, border-color 0.18s ease, background 0.18s ease;
	}

	.video-card:hover {
		transform: translateY(-3px);
		border-color: var(--border-strong);
		background: var(--bg-hover);
	}

	.poster-wrap {
		position: relative;
		aspect-ratio: 16 / 9;
		border-radius: 7px;
		overflow: hidden;
		background: var(--bg-raised);
	}

	.poster,
	.placeholder {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	.play-overlay {
		position: absolute;
		right: 10px;
		bottom: 10px;
		width: 34px;
		height: 34px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--accent);
		color: white;
		font-size: 0.8rem;
		box-shadow: 0 8px 18px rgba(0, 0, 0, 0.34);
		opacity: 0;
		transform: translateY(4px);
		transition: opacity 0.16s ease, transform 0.16s ease;
	}

	.video-card:hover .play-overlay {
		opacity: 1;
		transform: translateY(0);
	}

	.duration {
		position: absolute;
		left: 8px;
		bottom: 8px;
		padding: 3px 7px;
		border-radius: 999px;
		background: rgba(0, 0, 0, 0.56);
		color: rgba(255, 255, 255, 0.9);
		font-size: 0.68rem;
		font-weight: 700;
	}

	.meta {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.title,
	.subtitle {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.title {
		color: var(--text-primary);
		font-size: 0.86rem;
		font-weight: 700;
	}

	.subtitle {
		color: var(--text-tertiary);
		font-size: 0.74rem;
	}
</style>
