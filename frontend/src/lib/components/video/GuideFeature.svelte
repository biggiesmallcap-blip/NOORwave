<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { TidalSearchVideo } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';

	// The featured row at the top of a video guide (today's spotlight, daily
	// picks): the caller's intro on the left, a mosaic of up to five frames on
	// the right, over a faint wash of the lead frame. A frame plays from that
	// video.
	let {
		label,
		frames,
		busy = false,
		onAir = false,
		rise = null,
		onpick,
		children,
	}: {
		label: string;
		frames: TidalSearchVideo[];
		busy?: boolean;
		onAir?: boolean;
		/** Slot in the page's entrance cascade (`rise-in-shelf` in app.css). */
		rise?: number | null;
		onpick: (video: TidalSearchVideo) => void;
		children: Snippet;
	} = $props();

	let shown = $derived(frames.slice(0, 5));
	let lead = $derived(shown[0]?.artwork_url ?? null);

	function frameLabel(video: TidalSearchVideo): string {
		return video.artist_name ? `${video.title} - ${video.artist_name}` : video.title;
	}

	function frameMenu(event: MouseEvent, video: TidalSearchVideo) {
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

<article
	class="feature"
	class:on-air={onAir}
	class:rise-in-shelf={rise != null}
	style={rise != null ? `--rise-index: ${rise}` : undefined}
	aria-busy={busy}
	aria-label={label}
>
	<div class="wash" aria-hidden="true">
		<ArtworkImage src={lead} size={320} fallbackText="" decorative={true} fadeIn={true} />
	</div>

	<div class="intro">{@render children()}</div>

	<div class="mosaic" data-count={shown.length}>
		{#each shown as video, index (video.tidal_id)}
			<button
				type="button"
				class="frame"
				class:lead={index === 0}
				aria-label={`Play ${frameLabel(video)}`}
				onclick={() => onpick(video)}
				oncontextmenu={(event) => frameMenu(event, video)}
			>
				<ArtworkImage src={video.artwork_url} size={index === 0 ? 640 : 320} fallbackText="" decorative={true} fadeIn={true} />
				<span class="caption" aria-hidden="true">
					<span class="caption-title">{video.title}</span>
					{#if video.artist_name}<span class="caption-artist">{video.artist_name}</span>{/if}
				</span>
			</button>
		{/each}
	</div>
</article>

<style>
	.feature {
		position: relative;
		display: grid;
		grid-template-columns: minmax(0, 0.85fr) minmax(0, 1.15fr);
		gap: clamp(18px, 2.6vw, 36px);
		/* Top-aligned, not centered: the mosaic lands in the same place on
		   every page whatever the height of the intro beside it. */
		align-items: start;
		padding: clamp(14px, 2vw, 22px);
		border-radius: 16px;
		overflow: hidden;
		isolation: isolate;
	}
	.feature[aria-busy='true'] { opacity: 0.7; }
	.feature.on-air { box-shadow: inset 3px 0 0 var(--accent); }

	/* A faint wash of the lead frame, so the row reads as the featured one
	   without a heavy card. */
	.wash {
		position: absolute;
		inset: 0;
		z-index: -1;
		opacity: 0.2;
		filter: blur(56px) saturate(1.3);
		transform: scale(1.25);
		pointer-events: none;
	}
	.wash :global(img) { width: 100%; height: 100%; object-fit: cover; }

	.intro {
		display: grid;
		gap: 10px;
		justify-items: start;
		min-width: 0;
	}

	/* Lead frame over two rows, four small frames beside it. */
	.mosaic {
		display: grid;
		grid-template-columns: 2fr 1fr 1fr;
		gap: 6px;
		min-width: 0;
	}
	.mosaic[data-count='1'] { grid-template-columns: 1fr; }
	.mosaic[data-count='2'],
	.mosaic[data-count='3'] { grid-template-columns: 2fr 1fr; }
	.frame {
		position: relative;
		padding: 0;
		border: 0;
		border-radius: 8px;
		overflow: hidden;
		aspect-ratio: 16 / 9;
		background: var(--bg-raised);
		cursor: pointer;
		transition: transform var(--motion-fast);
	}
	/* The lead takes the height of the two small rows; its image is lifted
	   out of flow so a square source can't stretch them. */
	.frame.lead {
		grid-row: span 2;
		aspect-ratio: auto;
		border-radius: 12px;
	}
	.frame.lead :global(img) {
		position: absolute;
		inset: 0;
	}
	.mosaic[data-count='1'] .frame.lead,
	.mosaic[data-count='2'] .frame.lead {
		grid-row: auto;
		aspect-ratio: 16 / 9;
	}
	.frame :global(img) { width: 100%; height: 100%; object-fit: cover; }
	.frame:hover,
	.frame:focus-visible { transform: translateY(-2px); }
	.caption {
		position: absolute;
		inset: auto 0 0 0;
		display: grid;
		padding: 22px 10px 8px;
		background: linear-gradient(to top, rgba(0, 0, 0, 0.88), rgba(0, 0, 0, 0.55) 60%, transparent);
		color: #fff;
		text-align: left;
		opacity: 0;
		transition: opacity var(--motion-fast);
		pointer-events: none;
	}
	.frame:hover .caption,
	.frame:focus-visible .caption { opacity: 1; }
	.caption-title,
	.caption-artist {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		line-height: var(--line-height-snug);
	}
	.caption-title {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}
	.caption-artist {
		font-size: var(--font-size-xs);
		color: rgba(255, 255, 255, 0.78);
	}
	.frame:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	@media (max-width: 860px) {
		.feature { grid-template-columns: minmax(0, 1fr); }
	}
</style>
