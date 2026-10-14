<script lang="ts">
	import { discoverSpaceStore, walkBack, clearBranchPath } from './discover_space_store';
	import { currentTrack } from '$lib/stores/player';

	let path = $derived($discoverSpaceStore.branchPath);
	let currentLabel = $derived(currentSeedLabel());

	function currentSeedLabel(): string {
		const s = $discoverSpaceStore;
		if (s.activeSeedId === null) return '';
		const node = s.nodes.find((n) => n.trackId === s.activeSeedId);
		if (node) return node.title;
		const track = $currentTrack;
		if (track && track.id === s.activeSeedId) return track.title;
		return `Track ${s.activeSeedId}`;
	}
</script>

{#if path.length > 0}
	<nav class="breadcrumb" aria-label="Branch history">
		{#each path as step, index (index)}
			<button
				class="crumb"
				title={step.artist ? `${step.title} - ${step.artist}` : step.title}
				onclick={() => walkBack(index)}
			>
				{step.title}
			</button>
			<span class="sep" aria-hidden="true">›</span>
		{/each}
		<span class="crumb current" title="Current seed">{currentLabel}</span>
		<button class="clear" title="Clear branch history" onclick={clearBranchPath}>
			✕
		</button>
	</nav>
{/if}

<style>
	.breadcrumb {
		display: flex;
		align-items: center;
		gap: 4px;
		flex-wrap: wrap;
		/* Frosted, in the theme colours: a wallpaper reads through the page
		   scrim, and these panels sit on top of it. */
		background: color-mix(in srgb, var(--bg-elevated) 74%, transparent);
		backdrop-filter: var(--blur-base);
		-webkit-backdrop-filter: var(--blur-base);
		border: 1px solid var(--panel-border);
		border-radius: 999px;
		padding: 3px 10px;
		max-width: fit-content;
	}
	.crumb {
		border: none;
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		cursor: pointer;
		padding: 2px 4px;
		border-radius: 6px;
		max-width: 140px;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}
	.crumb:hover {
		color: var(--text-primary);
		background: var(--bg-surface);
	}
	.crumb.current {
		color: var(--accent-strong);
		cursor: default;
	}
	.sep {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}
	.clear {
		border: none;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		cursor: pointer;
		padding: 2px 4px;
	}
	.clear:hover {
		color: var(--text-primary);
	}
</style>
