<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		copy = '',
		actions
	}: {
		title: string;
		copy?: string;
		actions?: Snippet;
	} = $props();
</script>

<div class="empty-state glass-panel">
	<h3>{title}</h3>
	{#if copy}
		<p>{copy}</p>
	{/if}
	{#if actions}
		<div class="actions">
			{@render actions()}
		</div>
	{/if}
</div>

<style>
	.empty-state {
		padding: 24px;
		display: flex;
		flex-direction: column;
		align-items: flex-start;
		gap: 10px;
	}

	.empty-state p {
		color: var(--text-secondary);
		max-width: 52ch;
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		margin-top: 2px;
	}
</style>
