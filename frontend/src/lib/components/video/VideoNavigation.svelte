<script lang="ts">
	import { VIDEO_TABS, type VideoTab } from '$lib/video/section';

	// Sub-navigation for the video section. Same treatment as the category
	// pills under the search field on /search and /library: a centered row of
	// quiet outline pills, the current tab filled with the accent. The watch
	// page passes null, so no tab is lit while a video has the page. Tab hops
	// replace the history entry, so Back leaves the section in one step.
	let { current }: { current: VideoTab | null } = $props();
</script>

<nav class="video-navigation" aria-label="Video pages">
	{#each VIDEO_TABS as destination (destination.id)}
		<a
			class="nav-pill"
			class:active={current === destination.id}
			href={destination.href}
			data-sveltekit-replacestate
			aria-current={current === destination.id ? 'page' : undefined}
		>{destination.label}</a>
	{/each}
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
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}
</style>
