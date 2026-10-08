<script lang="ts">
	import type { Snippet } from 'svelte';

	// Toggling one filter on and off (STYLING.md "FilterChip"). On is the soft
	// accent: a filter changes what you see, so it reads as active without the
	// solid fill that ScopeTabs uses for "where you are".

	let {
		pressed,
		onclick,
		disabled = false,
		title,
		children,
	}: {
		pressed: boolean;
		onclick: () => void;
		disabled?: boolean;
		title?: string;
		children: Snippet;
	} = $props();
</script>

<button type="button" class="filter-chip" aria-pressed={pressed} {disabled} {title} {onclick}>
	{@render children()}
</button>

<style>
	.filter-chip {
		display: inline-flex;
		align-items: center;
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
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.filter-chip[aria-pressed='false']:hover:not(:disabled) {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.filter-chip[aria-pressed='true'] {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.filter-chip:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}
</style>
