<script lang="ts">
	import { setVideoBrowseMode, videoBrowseMode, videoSession } from '$lib/stores/video_session';

	let { current, canBrowse = false }: {
		current: 'videos' | 'liked' | 'editorial';
		canBrowse?: boolean;
	} = $props();

	const destinations = [
		{ id: 'videos', href: '/videos', label: 'Videos' },
		{ id: 'liked', href: '/videos/liked', label: 'Liked videos' },
		{ id: 'editorial', href: '/tidal/videos', label: 'TIDAL editorial' },
	] as const;
</script>

<div class="video-navigation">
	<nav aria-label="Video pages">
		{#each destinations as destination (destination.id)}
			<a
				class="btn btn-glass destination"
				href={destination.href}
				aria-current={current === destination.id ? 'page' : undefined}
			>{destination.label}</a>
		{/each}
	</nav>
	{#if $videoSession.active}
		{#if current === 'videos' && !$videoBrowseMode && canBrowse}
			<button type="button" class="back-link" onclick={() => setVideoBrowseMode(true)}>Back to picks</button>
		{:else if current === 'videos' && $videoBrowseMode}
			<button type="button" class="back-link" onclick={() => setVideoBrowseMode(false)}>Back to the player</button>
		{:else if current !== 'videos'}
			<a class="back-link" href="/videos" onclick={() => setVideoBrowseMode(false)}>Back to the player</a>
		{/if}
	{/if}
</div>

<style>
	.video-navigation,
	nav {
		display: flex;
		align-items: center;
		justify-content: flex-start;
		flex-wrap: wrap;
		gap: var(--space-2);
		min-width: 0;
	}

	.destination {
		white-space: nowrap;
		text-decoration: none;
	}

	.destination[aria-current='page'] {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.destination:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}
</style>
