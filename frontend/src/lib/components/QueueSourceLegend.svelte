<script lang="ts">
	import { SOURCE_LEGEND } from '$lib/player/queue_source';

	// Collapsed by default: this is reference material, not something you need
	// in front of you while the queue plays.
	let expanded = $state(false);
</script>

<div class="source-legend-row">
	<button
		class="source-legend-toggle"
		type="button"
		aria-expanded={expanded}
		aria-controls="queue-source-legend"
		onclick={() => { expanded = !expanded; }}
	>
		Source legend
		<span aria-hidden="true">{expanded ? '^' : 'v'}</span>
	</button>
	{#if expanded}
		<ul class="source-legend" id="queue-source-legend">
			{#each SOURCE_LEGEND as entry}
				<li>
					<span class="queue-source-dot source-{entry.slug}" aria-hidden="true"></span>
					<span>{entry.label}</span>
				</li>
			{/each}
		</ul>
	{/if}
</div>

<style>
	.source-legend-row {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.source-legend-toggle {
		align-self: flex-start;
		display: inline-flex;
		align-items: center;
		gap: 4px;
		padding: 4px 8px;
		border-radius: 999px;
		border: 1px solid transparent;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		cursor: pointer;
	}

	.source-legend-toggle:hover {
		color: var(--text-secondary);
		border-color: var(--border-subtle);
	}

	.source-legend {
		display: flex;
		flex-wrap: wrap;
		gap: 10px 14px;
		margin: 0;
		padding: 8px 10px;
		list-style: none;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--instrument-surface) 70%, transparent);
		border: 1px solid var(--border-subtle);
	}

	.source-legend li {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	/* The dot is absolutely positioned over queue artwork by default (app.css);
	   inside the legend it is just an inline swatch. */
	.source-legend :global(.queue-source-dot) {
		position: static;
		width: 8px;
		height: 8px;
		border-width: 0;
	}
</style>
