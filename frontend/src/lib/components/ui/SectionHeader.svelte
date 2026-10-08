<script lang="ts">
	// A section gets a title, never an eyebrow over it (STYLING.md "Typography").
	// Where the source matters, say it once in the subtitle.
	import type { Snippet } from 'svelte';

	let {
		title,
		subtitle = '',
		variant = 'default',
		level = 3,
		href = '',
		linkLabel = 'View all',
		actions
	}: {
		title: string;
		subtitle?: string;
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
		{#if level === 2}
			<h2 class="title t-section">{title}</h2>
		{:else}
			<h3 class="title t-section">{title}</h3>
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
				<a class="section-link" {href}>{linkLabel} →</a>
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

	/* Charts only tightens the metrics for its denser header. */
	.section-header.charts .title {
		color: var(--text-primary);
		line-height: var(--line-height-tight);
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
