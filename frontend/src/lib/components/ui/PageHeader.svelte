<script lang="ts">
	// The title header (STYLING.md "Page frames"): one title, no eyebrow over it.
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = '',
		variant = 'default',
		actions,
		meta
	}: {
		title: string;
		subtitle?: string;
		variant?: 'default' | 'editorial';
		actions?: Snippet;
		meta?: Snippet;
	} = $props();
</script>

<header class="page-header" class:editorial={variant === 'editorial'}>
	<div class="intro">
		<h1 class="t-page-title">{title}</h1>
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

	.page-header.editorial h1 {
		color: var(--text-primary);
		font-size: var(--font-size-3xl);
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
