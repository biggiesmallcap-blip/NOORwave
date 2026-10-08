<script lang="ts">
	import type { Snippet } from 'svelte';

	// The header of a search-led page (STYLING.md "Command header"): Back,
	// when the page has one, beside a field centred at the command measure;
	// scope tabs under it; then a fixed-height toolbar row. A page whose tabs
	// differ in tools passes a toolbar on every tab so the header keeps one
	// height. Anything else (a scope note) renders last.

	let {
		field,
		tabs,
		toolbar,
		children,
		onback,
		backLabel = 'Back',
	}: {
		field: Snippet;
		tabs?: Snippet;
		toolbar?: Snippet;
		children?: Snippet;
		onback?: () => void;
		backLabel?: string;
	} = $props();
</script>

<header class="command-header">
	<!-- Back sits beside the field instead of on a row of its own, so the field
	     lands at the same height on every search-led page. The side columns
	     are equal, which keeps the field centred on the page. -->
	<div class="search-row">
		{#if onback}
			<button type="button" class="back-link" onclick={onback}>{backLabel}</button>
		{/if}
		<div class="search-slot">
			{@render field()}
		</div>
	</div>
	{#if tabs}
		{@render tabs()}
	{/if}
	{#if toolbar}
		<div class="toolbar">
			{@render toolbar()}
		</div>
	{/if}
	{@render children?.()}
</header>

<style>
	.command-header {
		display: grid;
		gap: var(--space-4);
		width: 100%;
	}

	.search-row {
		display: grid;
		grid-template-columns: 1fr minmax(0, var(--measure-command)) 1fr;
		align-items: center;
		gap: var(--space-3);
	}

	.search-row .back-link {
		grid-column: 1;
		justify-self: start;
	}

	.search-slot {
		grid-column: 2;
		min-width: 0;
	}

	.toolbar {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		min-height: 36px;
		min-width: 0;
	}

	@media (max-width: 620px) {
		.search-row {
			grid-template-columns: auto minmax(0, 1fr);
		}
	}
</style>
