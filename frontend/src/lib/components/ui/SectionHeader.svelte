<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = '',
		eyebrow = '',
		variant = 'default',
		level = 3,
		actions
	}: {
		title: string;
		subtitle?: string;
		eyebrow?: string;
		variant?: 'default' | 'charts';
		level?: 2 | 3;
		actions?: Snippet;
	} = $props();
</script>

<div class="section-header" class:charts={variant === 'charts'}>
	<div class="copy">
		{#if eyebrow}
			<p class="eyebrow">{eyebrow}</p>
		{/if}
		{#if level === 2}
			<h2 class="title">{title}</h2>
		{:else}
			<h3 class="title">{title}</h3>
		{/if}
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

	.title {
		margin: 0;
	}

	.section-header.charts {
		align-items: center;
	}

	.section-header.charts .copy {
		gap: var(--space-1);
	}

	.section-header.charts .eyebrow {
		margin: 0;
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		letter-spacing: 0;
		line-height: 1;
	}

	.section-header.charts .title {
		color: var(--text-primary);
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		letter-spacing: 0;
	}

	.section-header.charts .subtitle {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
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
