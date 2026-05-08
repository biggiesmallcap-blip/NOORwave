<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = '',
		eyebrow = '',
		actions
	}: {
		title: string;
		subtitle?: string;
		eyebrow?: string;
		actions?: Snippet;
	} = $props();
</script>

<div class="section-header">
	<div class="copy">
		{#if eyebrow}
			<p class="eyebrow">{eyebrow}</p>
		{/if}
		<h3>{title}</h3>
		{#if subtitle}
			<p class="subtitle">{subtitle}</p>
		{/if}
	</div>

	{#if actions}
		<div class="actions">
			{@render actions()}
		</div>
	{/if}
</div>

<style>
	.section-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
	}

	.copy {
		max-width: 60ch;
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.eyebrow {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--text-tertiary);
	}

	.subtitle {
		color: var(--text-secondary);
	}

	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	@media (max-width: 860px) {
		.section-header {
			flex-direction: column;
		}
	}
</style>
