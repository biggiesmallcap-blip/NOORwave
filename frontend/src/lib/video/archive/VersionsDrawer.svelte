<script lang="ts">
	import { onMount } from 'svelte';
	import { api } from '$lib/api/client';
	import type {
		SongVersionsResponse,
		VideoConcert,
		VideoVersion,
	} from '$lib/api/video_explore_types';
	import QuickViewPopup from '$lib/components/ui/QuickViewPopup.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import ErrorState from '$lib/components/ui/ErrorState.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import Skeleton from '$lib/components/ui/Skeleton.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildVideoMenu } from '$lib/player/video_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { videoSession } from '$lib/stores/video_session';
	import { CUT_LABELS } from './lenses';
	import { orderVersions, runtimeLabel } from './versions';
	import { playVersion } from './play';
	import ConcertSetlist from './ConcertSetlist.svelte';

	// Every cut of one song, in the Liked version list's look: best first, what
	// tells each one apart, its cut and year, runtime on the right. A concert
	// link opens that show's setlist in place, with Play concert.

	interface SongSummary {
		song_key: string;
		title: string;
		artist_id: number | null;
		artist_name: string | null;
		artwork_url: string | null;
		cut_count: number;
	}

	let {
		song,
		concertId = null,
		onclose,
	}: {
		song: SongSummary;
		/** Open straight on this concert's setlist. */
		concertId?: number | null;
		onclose: () => void;
	} = $props();

	let data = $state<SongVersionsResponse | null>(null);
	let error = $state<unknown>(null);
	let loading = $state(true);
	let concert = $state<VideoConcert | null>(null);
	let concertLoading = $state<number | null>(null);
	let concertError = $state<unknown>(null);
	let concertAsked: number | null = null;

	let versions = $derived(data ? orderVersions(data.versions) : []);
	let playingId = $derived($videoSession.current?.tidal_id ?? null);
	let title = $derived(data?.title ?? song.title);
	let artistName = $derived(data?.artist_name ?? song.artist_name);
	let artistId = $derived(data?.artist_id ?? song.artist_id);

	async function load() {
		loading = true;
		error = null;
		try {
			data = await api.getVideoSongVersions(song.song_key);
		} catch (reason) {
			error = reason;
		} finally {
			loading = false;
		}
	}

	async function openConcert(id: number) {
		concertAsked = id;
		concertLoading = id;
		concertError = null;
		try {
			concert = await api.getVideoConcert(id);
		} catch (reason) {
			concertError = reason;
		} finally {
			concertLoading = null;
		}
	}

	function versionMenu(event: MouseEvent, version: VideoVersion) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildVideoMenu(version.video), version.label || version.video.title);
	}

	function artistMenu(event: MouseEvent) {
		if (artistId == null) return;
		event.preventDefault();
		event.stopPropagation();
		const name = artistName ?? 'Artist';
		openContextMenu(event, buildArtistMenu({ tidal_id: artistId, name }, { isLocal: false }), name);
	}

	onMount(() => {
		void load();
		if (concertId != null) void openConcert(concertId);
	});
</script>

<QuickViewPopup title={`Versions of ${title}`} onClose={onclose} busy={loading && data !== null}>
	{#snippet hero(requestClose)}
		<div class="hero">
			<div class="hero-art">
				<ArtworkImage className="hero-img" src={versions[0]?.video.artwork_url ?? song.artwork_url} size={320} fallbackText="VID" decorative={true} />
			</div>
			<div class="hero-copy">
				<span class="t-label">{data ? versions.length : song.cut_count} cuts</span>
				<h2 class="song-title">{title}</h2>
				<p class="t-meta">
					{#if artistName && artistId != null}
						<a href={`/tidal/artists/${artistId}`} oncontextmenu={artistMenu} onclick={requestClose}>{artistName}</a>
					{:else if artistName}
						{artistName}
					{/if}
					{#if data?.song_year}&middot; {data.song_year}{/if}
				</p>
			</div>
		</div>
	{/snippet}

	{#snippet children()}
		{#if concert || concertLoading !== null || concertError}
			<button type="button" class="back" onclick={() => { concert = null; concertError = null; concertLoading = null; }}>
				<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M15 6l-6 6 6 6" /></svg>
				All cuts
			</button>
			{#if concert}
				<ConcertSetlist {concert} onnavigate={onclose} />
			{:else if concertError}
				<ErrorState title="Could not load this concert" error={concertError} onretry={() => { if (concertAsked !== null) void openConcert(concertAsked); }} />
			{:else}
				<Skeleton rows={6} label="Loading the setlist" />
			{/if}
		{:else if loading && !data}
			<Skeleton rows={5} label="Loading versions" />
		{:else if error}
			<ErrorState title="Could not load the versions" error={error} onretry={() => void load()} />
		{:else if versions.length === 0}
			<EmptyState title="No other cuts" copy="This song has one video in the catalog so far." />
		{:else}
			<ul class="versions">
				{#each versions as version (version.video.tidal_id)}
					<li class="version" class:playing={playingId === version.video.tidal_id}>
						<button
							type="button"
							class="version-play"
							onclick={() => data && void playVersion(version, versions, data.title)}
							oncontextmenu={(event) => versionMenu(event, version)}
							aria-label={`Play ${version.label || version.video.title}`}
						>
							<span class="thumb">
								<ArtworkImage className="thumb-img" src={version.video.artwork_url} size={160} fallbackText="VID" decorative={true} />
							</span>
							<span class="version-main">
								<span class="version-label">{version.label || version.video.title}</span>
								<span class="version-meta">
									{#if version.best}<span class="best">Best</span>{/if}
									<span class="cut">{CUT_LABELS[version.cut] ?? version.cut}</span>
									{#if version.cut_year}<span class="year">{version.cut_year}</span>{/if}
								</span>
							</span>
							<span class="time">{runtimeLabel(version.video.duration_ms)}</span>
						</button>
						{#if version.concert}
							{@const show = version.concert}
							<button type="button" class="concert-link" onclick={() => void openConcert(show.id)}>
								From {show.label}
								<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" aria-hidden="true"><path d="M9 6l6 6-6 6" /></svg>
							</button>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	{/snippet}
</QuickViewPopup>

<style>
	.hero {
		display: grid;
		grid-template-columns: 176px minmax(0, 1fr);
		align-items: end;
		gap: var(--space-4);
	}

	.hero-art {
		position: relative;
		aspect-ratio: 16 / 9;
		border-radius: var(--radius-sm);
		overflow: hidden;
		background: var(--bg-raised);
	}

	.hero-art :global(.hero-img) {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.hero-art :global(.hero-img.fallback) {
		display: grid;
		place-items: center;
	}

	.hero-copy {
		display: grid;
		gap: 4px;
		min-width: 0;
	}

	.song-title {
		margin: 0;
		font-family: var(--font-display);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		text-wrap: balance;
		display: -webkit-box;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.hero-copy p {
		margin: 0;
	}

	.hero-copy a {
		color: var(--text-secondary);
		font-weight: var(--font-weight-semibold);
		text-decoration: none;
	}

	.hero-copy a:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.versions {
		display: grid;
		gap: var(--row-gap);
		margin: 0;
		padding: 0;
		list-style: none;
	}

	.version {
		display: grid;
		border-radius: var(--radius-sm);
		transition: background var(--motion-fast);
	}

	.version:hover,
	.version:focus-within {
		background: var(--bg-hover);
	}

	.version.playing {
		box-shadow: inset 3px 0 0 var(--accent);
	}

	.version.playing .version-label {
		color: var(--accent-strong);
	}

	.version-play {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr) 64px;
		align-items: center;
		gap: var(--space-3);
		width: 100%;
		padding: 6px 8px;
		border: 0;
		border-radius: var(--radius-sm);
		background: transparent;
		color: var(--text-secondary);
		text-align: left;
		cursor: pointer;
	}

	.version-play:focus-visible,
	.concert-link:focus-visible,
	.back:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.thumb {
		position: relative;
		width: 96px;
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

	.version-main {
		display: grid;
		gap: 3px;
		min-width: 0;
	}

	/* Wraps rather than ellipsing, like the Liked list: the label is the only
	   thing that says which cut this is. */
	.version-label {
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
		overflow-wrap: anywhere;
	}

	.version-meta {
		display: flex;
		align-items: center;
		gap: 6px;
		flex-wrap: wrap;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
	}

	.cut,
	.best {
		padding: 2px 7px;
		border-radius: 999px;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	.cut {
		background: var(--bg-surface);
		color: var(--text-secondary);
	}

	.best {
		background: var(--accent-soft);
		color: var(--accent-strong);
	}

	.time {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		text-align: right;
	}

	.concert-link {
		justify-self: start;
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin: -4px 0 6px calc(96px + var(--space-3) + 8px);
		padding: 2px 0;
		border: 0;
		background: none;
		color: var(--accent-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.concert-link:hover {
		text-decoration: underline;
	}

	.back {
		display: inline-flex;
		align-items: center;
		gap: 4px;
		margin-bottom: var(--space-3);
		padding: 4px 10px 4px 6px;
		border: 0;
		border-radius: 999px;
		background: var(--bg-surface);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
	}

	.back:hover {
		color: var(--text-primary);
	}

	@media (max-width: 560px) {
		.hero {
			grid-template-columns: 112px minmax(0, 1fr);
		}

		.version-play {
			grid-template-columns: 72px minmax(0, 1fr) 48px;
		}

		.thumb {
			width: 72px;
		}

		.concert-link {
			margin-left: calc(72px + var(--space-3) + 8px);
		}
	}
</style>
