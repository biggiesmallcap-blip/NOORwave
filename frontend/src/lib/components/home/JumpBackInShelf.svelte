<script lang="ts">
	// Home's first row: the last tracks you played, newest first, one card per
	// track. The same recent-listens source as Search's idle "Jump back in", with
	// a longer window so the rail fills wide windows. Hidden when there is no
	// history.
	import { onMount } from 'svelte';
	import type { ListenHistoryEntry } from '$lib/api/client';
	import { cachedApi } from '$lib/cache/api_queries';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu } from '$lib/player/track_menu';
	import { playTrackNow } from '$lib/stores/player';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import MediaRail from '$lib/components/ui/MediaRail.svelte';
	import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';

	let { index = 0 }: { index?: number } = $props();

	// Enough distinct tracks to fill the rail on wide windows; it scrolls.
	const LIMIT = 16;
	let entries = $state<ListenHistoryEntry[]>([]);

	onMount(() => {
		const recent = cachedApi.getRecentListens(50);
		void recent
			.then((res) => {
				const seen = new Set<number>();
				entries = res.listens.filter((entry) => !seen.has(entry.track_id) && seen.add(entry.track_id)).slice(0, LIMIT);
			})
			.catch((error) => console.warn('[home] recent listens failed; "Jump back in" hidden', error));
	});

	function onContextMenu(event: MouseEvent, entry: ListenHistoryEntry) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildTrackMenu({
				id: entry.track_id,
				title: entry.track_title,
				artist_id: null,
				artist_name: entry.artist_name,
				album_id: null,
				album_title: entry.album_title,
			}),
			entry.track_title,
		);
	}
</script>

{#snippet card(entry: ListenHistoryEntry)}
	<button
		type="button"
		class="jump-card"
		title={`${entry.track_title}${entry.artist_name ? ` - ${entry.artist_name}` : ''}`}
		onclick={() => void playTrackNow(entry.track_id)}
		oncontextmenu={(event) => onContextMenu(event, entry)}
	>
		<div class="art">
			<ArtworkImage
				className="jump-art"
				src={entry.artwork_url}
				alt={entry.track_title}
				size={320}
				tint
				fallbackText={(entry.track_title.trim()[0] ?? 'N').toUpperCase()}
			/>
			<PlayOverlay position="corner" size="sm" />
		</div>
		<p class="t-row-title title">{entry.track_title}</p>
		{#if entry.artist_name}<p class="t-meta sub">{entry.artist_name}</p>{/if}
	</button>
{/snippet}

{#if entries.length > 0}
	<section class="discovery-section rise-in-shelf" data-section="jump-back-in" style={`--rise-index: ${index}`}>
		<SectionHeader title="Jump back in" variant="charts" level={2} />
		<MediaRail items={entries} {card} getKey={(entry) => entry.track_id} gap={14} fluid stagger />
	</section>
{/if}

<style>
	.jump-card {
		all: unset;
		display: grid;
		gap: 4px;
		width: 100%;
		cursor: pointer;
	}

	.art {
		position: relative;
		overflow: hidden;
		aspect-ratio: 1;
		border-radius: var(--radius-md);
		background: var(--bg-raised);
	}

	.art :global(.jump-art) {
		display: block;
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.art :global(.jump-art.fallback) {
		display: flex;
		align-items: center;
		justify-content: center;
		font-size: var(--font-size-4xl);
		font-weight: var(--font-weight-semibold);
	}

	.jump-card:hover :global(.play-overlay),
	.jump-card:focus-visible :global(.play-overlay) {
		opacity: 1;
		transform: none;
	}

	.jump-card:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 4px;
		border-radius: var(--radius-md);
	}

	.title,
	.sub {
		margin: 0;
		overflow: hidden;
		white-space: nowrap;
		text-overflow: ellipsis;
	}

	.title {
		margin-top: 6px;
	}
</style>
