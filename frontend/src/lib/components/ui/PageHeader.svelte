<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = '',
		eyebrow = '',
		actions,
		meta
	}: {
		title: string;
		subtitle?: string;
		eyebrow?: string;
		actions?: Snippet;
		meta?: Snippet;
	} = $props();
</script>

<header class="page-header">
	<div class="intro">
		{#if eyebrow}
			<p class="eyebrow">{eyebrow}</p>
		{/if}
		<h1>{title}</h1>
		{#if subtitle}
			<p class="subtitle">{subtitle}</p>
		{/if}
	</div>

	{#if meta || actions}
		<div class="side">
			{#if meta}
				<div class="meta">
					{@render meta()}
				</div>
			{/if}
			{#if actions}
				<div class="actions">
					{@render actions()}
				</div>
			{/if}
		</div>
	{/if}
</header>

<style>
	.page-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-5);
	}

	.intro {
		max-width: 60ch;
		display: flex;
		flex-direction: column;
		gap: 10px;
	}

	.eyebrow {
		font-size: 0.72rem;
		font-weight: 600;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		color: var(--text-tertiary);
	}

	h1 {
		font-family: var(--font-body);
		font-size: clamp(1.55rem, 2vw, 1.9rem);
		font-weight: 700;
		line-height: 1.15;
		letter-spacing: 0;
	}

	.subtitle {
		color: var(--text-secondary);
	}

	.side {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: var(--space-3);
	}

	.meta,
	.actions {
		display: flex;
		flex-wrap: wrap;
		justify-content: flex-end;
		gap: var(--space-2);
	}

	@media (max-width: 860px) {
		.page-header {
			flex-direction: column;
		}

		.side {
			align-items: flex-start;
			width: 100%;
		}

		.meta,
		.actions {
			justify-content: flex-start;
		}
	}
</style>
