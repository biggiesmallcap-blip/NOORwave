<script lang="ts">
	import type { TidalSearchVideo } from '$lib/api/client';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';

	// One row of a video guide (stations, the Videos tab): optional number,
	// name, meta and count on the left, a filmstrip of what it would play on
	// the right. The strip wraps into a clipped second line, so it shows as
	// many whole frames as fit and never scrolls. A frame plays from that
	// video; the name and the play button play from the top.
	let {
		title,
		titleHint = '',
		meta = '',
		count = null,
		number = null,
		label,
		onAir = false,
		busy = false,
		frames,
		rise = null,
		onplay,
	}: {
		title: string;
		titleHint?: string;
		meta?: string;
		count?: string | null;
		number?: string | null;
		/** What the row is, for assistive tech ("Rock station"). */
		label: string;
		onAir?: boolean;
		busy?: boolean;
		frames: TidalSearchVideo[];
		/** Slot in the page's entrance cascade (`rise-in-shelf` in app.css). */
		rise?: number | null;
		onplay: (startWith?: TidalSearchVideo) => void;
	} = $props();

	function frameLabel(video: TidalSearchVideo): string {
		return video.artist_name ? `${video.title} - ${video.artist_name}` : video.title;
	}

	function frameMenu(event: MouseEvent, video: TidalSearchVideo) {
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

<article
	class="guide-row"
	class:on-air={onAir}
	class:numbered={number != null}
	class:rise-in-shelf={rise != null}
	style={rise != null ? `--rise-index: ${rise}` : undefined}
	aria-busy={busy}
	aria-label={label}
>
	<div class="ident">
		{#if number != null}<span class="number">{number}</span>{/if}
		{#if onAir}<span class="on-air-tag">On air</span>{/if}
		<button type="button" class="name" title={titleHint || undefined} onclick={() => onplay()}>{title}</button>
		{#if meta}<span class="meta">{meta}</span>{/if}
		{#if count}<span class="count">{count}</span>{/if}
	</div>

	<div class="strip">
		{#each frames as video (video.tidal_id)}
			<button
				type="button"
				class="frame"
				aria-label={`Play ${title} from ${frameLabel(video)}`}
				onclick={() => onplay(video)}
				oncontextmenu={(event) => frameMenu(event, video)}
			>
				<ArtworkImage src={video.artwork_url} size={320} fallbackText="" decorative={true} fadeIn={true} />
				<span class="caption" aria-hidden="true">
					<span class="caption-title">{video.title}</span>
					{#if video.artist_name}<span class="caption-artist">{video.artist_name}</span>{/if}
				</span>
			</button>
		{/each}
	</div>

	<button type="button" class="play" aria-label={`Play ${label}`} onclick={() => onplay()}>
		<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M8 5.5v13l11-6.5z" /></svg>
	</button>
</article>

<style>
	.guide-row {
		--frame-h: 76px;
		position: relative;
		display: grid;
		grid-template-columns: 220px minmax(0, 1fr) auto;
		align-items: center;
		gap: 18px;
		padding: 10px 12px;
		border-radius: 12px;
		transition: background var(--motion-fast);
	}
	.guide-row:hover,
	.guide-row:focus-within {
		background: var(--bg-hover);
	}
	.guide-row[aria-busy='true'] {
		opacity: 0.6;
	}
	/* The station playing now: an accent edge down the row's left side. */
	.guide-row.on-air {
		box-shadow: inset 3px 0 0 var(--accent);
	}

	.ident {
		display: grid;
		grid-template-columns: minmax(0, 1fr);
		row-gap: 2px;
		align-items: baseline;
		min-width: 0;
	}
	.numbered .ident {
		grid-template-columns: auto minmax(0, 1fr);
		column-gap: 10px;
	}
	.number {
		grid-row: span 3;
		align-self: start;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
		font-variant-numeric: tabular-nums;
		font-weight: var(--font-weight-semibold);
	}
	.on-air-tag {
		justify-self: start;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 1.2px;
		color: var(--accent);
	}
	.name {
		justify-self: start;
		max-width: 100%;
		padding: 0;
		border: 0;
		background: transparent;
		color: var(--text-primary);
		font: inherit;
		font-weight: var(--font-weight-semibold);
		text-align: left;
		cursor: pointer;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.name:hover { text-decoration: underline; text-underline-offset: 3px; }
	.meta,
	.count {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.numbered .meta,
	.numbered .count {
		grid-column: 2;
	}
	.meta {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}
	.count {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	/* Frames wrap onto a clipped second line: whole frames only, as many
	   as the width allows. */
	.strip {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		height: var(--frame-h);
		overflow: hidden;
		min-width: 0;
	}
	/* Frames on the visible line stretch a little to close the gap at the
	   right edge (the picture covers the extra width). */
	.frame {
		position: relative;
		flex: 1 0 auto;
		height: var(--frame-h);
		aspect-ratio: 16 / 9;
		padding: 0;
		border: 0;
		border-radius: 8px;
		overflow: hidden;
		background: var(--bg-raised);
		cursor: pointer;
		transition: opacity 120ms ease;
	}
	/* At rest the wall of thumbnails sits a little dim and muted, so a page
	   of rows reads calm; the row under the pointer comes up to full color. */
	.frame :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		filter: var(--art-wall-filter);
		transition: filter 160ms ease;
	}
	.guide-row:hover .frame :global(img),
	.guide-row:focus-within .frame :global(img) {
		filter: none;
	}
	/* Hover is instant and in place (the strip clips anything that moves):
	   the other frames dim, this one names its video. */
	.strip:hover .frame { opacity: 0.5; }
	.strip .frame:hover,
	.strip .frame:focus-visible {
		opacity: 1;
	}
	.frame:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
	.caption {
		position: absolute;
		inset: auto 0 0 0;
		display: grid;
		padding: 18px 8px 6px;
		background: linear-gradient(to top, rgba(0, 0, 0, 0.88), rgba(0, 0, 0, 0.55) 60%, transparent);
		color: #fff;
		text-align: left;
		opacity: 0;
		transition: opacity 120ms ease;
		pointer-events: none;
	}
	.frame:hover .caption,
	.frame:focus-visible .caption {
		opacity: 1;
	}
	.caption-title,
	.caption-artist {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		line-height: var(--line-height-snug);
	}
	.caption-title {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}
	.caption-artist {
		font-size: var(--font-size-2xs);
		color: rgba(255, 255, 255, 0.78);
	}

	.play {
		display: grid;
		place-items: center;
		width: 40px;
		height: 40px;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: var(--accent);
		color: #fff;
		cursor: pointer;
		opacity: 0;
		transition: opacity var(--motion-fast);
	}
	.play svg {
		width: 18px;
		height: 18px;
		fill: currentColor;
	}
	.guide-row:hover .play,
	.guide-row:focus-within .play {
		opacity: 1;
	}
	.play:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	@media (max-width: 860px) {
		.guide-row {
			--frame-h: 56px;
			grid-template-columns: minmax(0, 1fr);
			gap: 10px;
		}
		.play { display: none; }
	}
</style>
