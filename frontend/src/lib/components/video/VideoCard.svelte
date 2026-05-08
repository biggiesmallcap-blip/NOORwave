<script lang="ts">
	import type { TidalSearchVideo, TidalVideoMix, TidalVideoMixItem } from '$lib/api/client';
	import { lazyTidalArt } from '$lib/actions/lazy-tidal-art';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { formatTrackDuration } from '$lib/utils/format';
	import { buildVideoMenu, buildVideoMixMenu, isVideoMix } from '$lib/player/video_menu';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';

	type VideoLike = TidalSearchVideo | TidalVideoMix | TidalVideoMixItem;

	let { video, onSelect }: { video: VideoLike; onSelect?: (video: VideoLike) => void } = $props();

	let lazyArtwork = $state<string | null>(null);
	let poster = $derived(('artwork_url' in video ? video.artwork_url : null) ?? lazyArtwork);
	let title = $derived(video.title);
	let subtitle = $derived(
		isVideoMix(video) ? (video.description ?? 'Video mix') : (video.artist_name ?? 'TIDAL video')
	);
	let duration = $derived(!isVideoMix(video) && video.duration_ms ? formatTrackDuration(video.duration_ms) : null);

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
		<PlayOverlay
			position="corner"
			size="sm"
			label={isVideoMix(video) ? `Open mix ${title}` : `Play ${title}`}
		/>
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

	.video-card:hover :global(.play-overlay),
	.video-card:focus-visible :global(.play-overlay) {
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
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
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
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-bold);
	}

	.subtitle {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}
</style>
