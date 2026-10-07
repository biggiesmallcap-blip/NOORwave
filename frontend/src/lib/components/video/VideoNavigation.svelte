<script lang="ts">
	import { setVideoBrowseMode, videoBrowseMode, videoSession } from '$lib/stores/video_session';

	// Sub-navigation for the video pages. Same treatment as the category pills
	// under the search field on /search and /library: a centered row of quiet
	// outline pills, the current page filled with the accent. The way back to
	// the player is a pill in the same row, set apart by a divider, so it reads
	// as part of the bar rather than a fifth tab in a different style.
	let { current, canBrowse = false }: {
		current: 'videos' | 'liked' | 'stations' | 'editorial';
		canBrowse?: boolean;
	} = $props();

	const destinations = [
		{ id: 'videos', href: '/videos', label: 'Videos' },
		{ id: 'liked', href: '/videos/liked', label: 'Liked videos' },
		{ id: 'stations', href: '/videos/stations', label: 'Stations' },
		{ id: 'editorial', href: '/tidal/videos', label: 'TIDAL editorial' },
	] as const;

	let back = $derived.by(() => {
		if (!$videoSession.active) return null;
		if (current === 'videos') {
			if (!$videoBrowseMode && canBrowse) return 'picks';
			if ($videoBrowseMode) return 'player';
			return null;
		}
		return 'link';
	});
</script>

<nav class="video-navigation" aria-label="Video pages">
	{#each destinations as destination (destination.id)}
		<a
			class="nav-pill"
			class:active={current === destination.id}
			href={destination.href}
			aria-current={current === destination.id ? 'page' : undefined}
		>{destination.label}</a>
	{/each}
	{#if back}
		<span class="divider" aria-hidden="true"></span>
		{#if back === 'picks'}
			<button type="button" class="nav-pill back" onclick={() => setVideoBrowseMode(true)}>Back to picks</button>
		{:else if back === 'player'}
			<button type="button" class="nav-pill back" onclick={() => setVideoBrowseMode(false)}>Back to the player</button>
		{:else}
			<a class="nav-pill back" href="/videos" onclick={() => setVideoBrowseMode(false)}>Back to the player</a>
		{/if}
	{/if}
</nav>

<style>
	.video-navigation {
		display: flex;
		align-items: center;
		justify-content: center;
		flex-wrap: wrap;
		gap: 6px;
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
		min-width: 0;
	}

	.nav-pill {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		gap: 6px;
		height: var(--control-h);
		padding: 0 14px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: transparent;
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		text-decoration: none;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.nav-pill:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.nav-pill.active {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--text-on-accent);
		font-weight: var(--font-weight-semibold);
	}

	.nav-pill:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.back::before {
		content: '\2039';
		font-size: var(--font-size-md);
		line-height: 1;
	}

	.divider {
		width: 1px;
		height: 18px;
		margin: 0 4px;
		background: var(--border-subtle);
	}
</style>
