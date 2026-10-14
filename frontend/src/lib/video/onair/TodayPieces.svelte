<script lang="ts" module>
	import type { VideoTodayResponse } from '$lib/api/video_explore_types';
	// One day's pieces, kept across visits to the dial.
	let cached: VideoTodayResponse | null = null;
</script>

<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api/client';
	import type { ExploreVideo, TodayPiece } from '$lib/api/video_explore_types';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { atlasUrl } from '$lib/video/atlas/links';
	import { formatRuntime, playAtlasQueue } from '$lib/video/atlas/play';
	import { noteVideoLaunch } from '$lib/video/launch';
	import { tuneChannel } from './tune';

	// Today, under the scenes: three short pieces, one artist each (the day's
	// Spotlight, someone new next to what you watch, someone from your
	// archive), written from what is known and nothing made up. A picture,
	// the name, a few plain sentences, the start of a biography with where it
	// is from, and three videos to start with.

	let today = $state<VideoTodayResponse | null>(cached);
	// Pieces that arrive during this visit rise in; kept ones are just there.
	const rise = !cached;
	let failed = $state(false);

	let dateLine = $derived.by(() => {
		if (!today) return '';
		const date = new Date(`${today.day}T12:00:00`);
		return Number.isNaN(date.getTime())
			? ''
			: date.toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long' });
	});

	onMount(() => {
		const day = new Date().toLocaleDateString('en-CA');
		if (cached && cached.day === day) return;
		api
			.getVideoToday()
			.then((result) => {
				cached = result;
				today = result;
			})
			.catch(() => {
				failed = true;
			});
	});

	function playFrom(piece: TodayPiece, video: ExploreVideo, el?: Element | null) {
		noteVideoLaunch(el);
		void playAtlasQueue(video, piece.videos, `${piece.name}: start here`);
	}

	function lead(piece: TodayPiece, el: Element | null) {
		noteVideoLaunch(el);
		if (piece.kind === 'today') void tuneChannel({ id: 'spotlight', title: 'Spotlight' });
		else if (piece.videos[0]) void playAtlasQueue(piece.videos[0], piece.videos, `${piece.name}: start here`);
	}

	function artistMenu(event: MouseEvent, piece: TodayPiece) {
		event.preventDefault();
		openContextMenu(event, buildArtistMenu({ tidal_id: piece.artist_id, name: piece.name, in_library: false }, { isLocal: false }), piece.name);
	}

	function videoMenu(event: MouseEvent, video: ExploreVideo) {
		event.preventDefault();
		openContextMenu(event, buildVideoMenu(video), video.title);
	}
</script>

{#if today && today.pieces.length > 0}
	<section class="today" class:rise-in-shelf={rise} aria-labelledby="today-label" style="--rise-index: 3">
		<header class="opener">
			<h3 id="today-label">Today</h3>
			<p>{dateLine}{dateLine ? '. ' : ''}Three artists, new each day.</p>
		</header>

		<div class="pieces" data-count={today.pieces.length}>
			{#each today.pieces as piece, index (piece.kind + piece.artist_id)}
				{@const hero = piece.videos[0]?.artwork_url ?? piece.photo_url}
				<article class="piece" class:lead={index === 0}>
					<button
						type="button"
						class="picture"
						onclick={(event) => lead(piece, event.currentTarget)}
						aria-label={piece.kind === 'today' ? `Watch ${piece.name} on Spotlight` : `Play ${piece.name}`}
					>
						<ArtworkImage src={hero} size={640} fallbackText={piece.name.slice(0, 1)} tint decorative fadeIn />
						<span class="play" aria-hidden="true"><svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z" /></svg></span>
					</button>

					<div class="words">
						<p class="kicker">{piece.kicker}</p>
						<h4 class="name">
							<a href={`/tidal/artists/${piece.artist_id}`} oncontextmenu={(event) => artistMenu(event, piece)}>{piece.name}</a>
						</h4>
						{#each piece.paragraphs as paragraph, p (p)}
							<p class="para" class:first={p === 0}>{paragraph}</p>
						{/each}
						{#if piece.bio}
							<blockquote class="bio">
								<p>{piece.bio.text}</p>
								<footer>Biography from {piece.bio.source}</footer>
							</blockquote>
						{/if}

						{#if piece.videos.length > 0}
							<p class="start-label">Start here</p>
							<ul class="videos">
								{#each piece.videos as video (video.tidal_id)}
									<li>
										<button type="button" class="video" onclick={(event) => playFrom(piece, video, event.currentTarget.querySelector('.thumb'))} oncontextmenu={(event) => videoMenu(event, video)}>
											<span class="thumb"><ArtworkImage src={video.artwork_url} size={320} fallbackText="" decorative fadeIn /></span>
											<span class="video-title">{video.title}</span>
											<span class="runtime">{formatRuntime(video.duration_ms)}</span>
										</button>
									</li>
								{/each}
							</ul>
						{/if}

						<div class="actions">
							<button type="button" class="btn btn-primary" onclick={(event) => lead(piece, event.currentTarget.closest('.piece')?.querySelector('.picture') ?? null)}>
								{piece.kind === 'today' ? 'Watch Spotlight' : 'Play their videos'}
							</button>
							<a class="btn btn-ghost" href={atlasUrl({ artist: piece.artist_id })}>On the Atlas</a>
						</div>
					</div>
				</article>
			{/each}
		</div>
	</section>
{:else if failed}
	<!-- Today is extra: if it cannot load, the dial stands without it. -->
{/if}

<style>
	.today {
		container-type: inline-size;
		display: grid;
		gap: var(--space-5);
		margin-top: var(--space-7);
	}
	.opener {
		display: grid;
		gap: 6px;
		padding: 0 12px;
	}
	.opener h3 {
		margin: 0;
		color: var(--text-primary);
		font-family: var(--font-display);
		font-size: var(--font-size-3xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-tight);
		letter-spacing: -0.01em;
	}
	.opener p {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	/* The lead piece across the page, picture beside words; the others in a
	   pair below it, picture over words. */
	.pieces {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-7) clamp(28px, 3vw, 56px);
		padding: 0 12px;
	}
	.piece {
		display: grid;
		gap: var(--space-4);
		align-content: start;
		min-width: 0;
	}
	.piece.lead {
		grid-column: 1 / -1;
		grid-template-columns: minmax(0, 1.15fr) minmax(0, 1fr);
		gap: clamp(24px, 3vw, 48px);
		align-items: start;
	}
	.pieces[data-count='1'] .piece.lead,
	.pieces[data-count='2'] .piece:not(.lead) {
		grid-column: 1 / -1;
	}

	.picture {
		position: relative;
		display: block;
		width: 100%;
		aspect-ratio: 16 / 9;
		padding: 0;
		border: 0;
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--bg-raised);
		cursor: pointer;
	}
	.picture :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		transition: transform var(--motion-slow);
	}
	.picture:hover :global(img) {
		transform: scale(1.02);
	}
	.play {
		position: absolute;
		left: 16px;
		bottom: 16px;
		display: grid;
		place-items: center;
		width: 44px;
		height: 44px;
		border-radius: 50%;
		background: rgba(0, 0, 0, 0.55);
		color: #fff;
		opacity: 0;
		transition: opacity var(--motion-base);
	}
	.play svg {
		width: 18px;
		height: 18px;
		fill: currentColor;
	}
	.picture:hover .play,
	.picture:focus-visible .play {
		opacity: 1;
	}
	.picture:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 3px;
	}

	.words {
		display: grid;
		gap: 10px;
		min-width: 0;
		max-width: 62ch;
	}
	.kicker {
		margin: 0;
		color: var(--accent-strong);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}
	.name {
		margin: 0 0 4px;
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-style: italic;
		font-weight: var(--font-weight-medium);
		line-height: var(--line-height-tight);
	}
	.lead .name {
		font-size: var(--font-size-3xl);
	}
	.name a {
		color: var(--text-primary);
		text-decoration: none;
	}
	.name a:hover {
		text-decoration: underline;
		text-decoration-thickness: 1px;
		text-underline-offset: 5px;
	}
	.para {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}
	.para.first {
		color: var(--text-primary);
	}
	.bio {
		margin: 4px 0 0;
		padding-left: 14px;
		border-left: 2px solid var(--border-subtle);
	}
	.bio p {
		margin: 0;
		color: var(--text-secondary);
		font-family: var(--font-display);
		font-size: var(--font-size-md);
		font-style: italic;
		line-height: var(--line-height-normal);
	}
	.bio footer {
		margin-top: 6px;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
	}

	.start-label {
		margin: 6px 0 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-label);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.12em;
		text-transform: uppercase;
	}
	.videos {
		display: grid;
		gap: 2px;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.video {
		display: grid;
		grid-template-columns: 72px minmax(0, 1fr) auto;
		align-items: center;
		gap: 12px;
		width: 100%;
		padding: 5px 6px 5px 0;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-fast);
	}
	.video:hover {
		background: var(--bg-hover);
	}
	.video:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}
	.thumb {
		width: 72px;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-xs);
		overflow: hidden;
		background: var(--bg-raised);
	}
	.thumb :global(img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}
	.video-title {
		overflow: hidden;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		text-overflow: ellipsis;
		white-space: nowrap;
	}
	.runtime {
		color: var(--text-tertiary);
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
		margin-top: 6px;
	}

	@container (max-width: 860px) {
		.pieces {
			grid-template-columns: minmax(0, 1fr);
		}
		.piece.lead {
			grid-template-columns: minmax(0, 1fr);
		}
	}
	@media (prefers-reduced-motion: reduce) {
		.picture :global(img) { transition: none; }
	}
</style>
