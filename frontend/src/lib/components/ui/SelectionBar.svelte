<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		summary,
		sticky = false,
		actions
	}: {
		summary: string;
		sticky?: boolean;
		actions?: Snippet;
	} = $props();
</script>

<div class:sticky class="selection-bar glass">
	<p>{summary}</p>
	{#if actions}
		<div class="actions">
			{@render actions()}
		</div>
	{/if}
</div>

<style>
	.selection-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 12px 14px;
	}

	.selection-bar p {
		color: var(--text-primary);
		font-weight: 500;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.sticky {
		position: sticky;
		top: 0;
		z-index: 12;
	}

	@media (max-width: 860px) {
		.selection-bar {
			flex-direction: column;
			align-items: flex-start;
		}
	}
</style>
