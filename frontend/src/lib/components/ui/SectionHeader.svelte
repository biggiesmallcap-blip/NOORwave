<script lang="ts">
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = '',
		eyebrow = '',
		variant = 'default',
		level = 3,
		href = '',
		linkLabel = 'View all',
		actions
	}: {
		title: string;
		subtitle?: string;
		eyebrow?: string;
		variant?: 'default' | 'charts';
		level?: 2 | 3;
		/** Route this section is a preview of. Renders a trailing link, which is
		 *  how a home shelf points at its own full page. */
		href?: string;
		linkLabel?: string;
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

	{#if actions || href}
		<div class="actions">
			{#if actions}
				{@render actions()}
			{/if}
			{#if href}
				<a class="section-link" {href}>{linkLabel} -&gt;</a>
			{/if}
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

	/* The charts variant used to re-style the eyebrow (zero tracking,
	   --text-muted). Between that, the global `.eyebrow` and one component's
	   local override, the same word rendered three different ways in a single
	   scroll on Home. The eyebrow is now identical in both variants; charts
	   only adjusts the metrics it needs for its tighter header. */
	.section-header.charts .eyebrow {
		margin: 0;
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
		align-items: center;
		gap: var(--space-2);
		flex-shrink: 0;
	}

	.section-link {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		text-decoration: none;
		white-space: nowrap;
		transition: color var(--motion-fast);
	}

	.section-link:hover,
	.section-link:focus-visible {
		color: var(--text-primary);
		outline: none;
	}

	@media (max-width: 860px) {
		.section-header {
			flex-direction: column;
		}
	}
</style>
