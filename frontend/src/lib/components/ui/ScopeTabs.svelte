<script lang="ts">
	// Moving between views of one page (STYLING.md "ScopeTabs"): a row of
	// quiet outline pills, the current one filled with the accent. When every
	// view is its own route the tabs are links (aria-current); otherwise they
	// are a tablist with one tab stop and onselect. Arrow keys, Home and End
	// move between tabs either way; in a tablist they also select.

	type ScopeTab = { id: string; label: string; href?: string; count?: number | string | null };

	let {
		tabs,
		current,
		label,
		onselect,
		replaceState = false,
		align = 'center',
	}: {
		tabs: readonly ScopeTab[];
		current: string | null;
		label: string;
		onselect?: (id: string) => void;
		/** Route tabs replace the history entry, so Back leaves the page in one step. */
		replaceState?: boolean;
		align?: 'center' | 'start';
	} = $props();

	let routed = $derived(tabs.every((tab) => tab.href));
	let root = $state<HTMLElement>();

	function onKeydown(event: KeyboardEvent) {
		const items = [...(root?.querySelectorAll<HTMLElement>('.scope-tab') ?? [])];
		const from = items.indexOf(event.currentTarget as HTMLElement);
		const last = items.length - 1;
		let to = -1;
		if (event.key === 'ArrowRight') to = from === last ? 0 : from + 1;
		else if (event.key === 'ArrowLeft') to = from === 0 ? last : from - 1;
		else if (event.key === 'Home') to = 0;
		else if (event.key === 'End') to = last;
		if (from < 0 || to < 0) return;
		// The window-level player shortcuts read arrows as seek.
		event.preventDefault();
		event.stopPropagation();
		items[to].focus();
		if (!routed) onselect?.(tabs[to].id);
	}
</script>

{#if routed}
	<nav class="scope-tabs" class:start={align === 'start'} aria-label={label} bind:this={root}>
		{#each tabs as tab (tab.id)}
			<a
				class="scope-tab"
				class:active={current === tab.id}
				href={tab.href}
				data-sveltekit-replacestate={replaceState || undefined}
				aria-current={current === tab.id ? 'page' : undefined}
				onkeydown={onKeydown}
			>{tab.label}{#if tab.count != null}<span class="count">{tab.count}</span>{/if}</a>
		{/each}
	</nav>
{:else}
	<div class="scope-tabs" class:start={align === 'start'} role="tablist" aria-label={label} bind:this={root}>
		{#each tabs as tab, index (tab.id)}
			<button
				type="button"
				role="tab"
				class="scope-tab"
				class:active={current === tab.id}
				aria-selected={current === tab.id}
				tabindex={current === tab.id || (current === null && index === 0) ? 0 : -1}
				onclick={() => onselect?.(tab.id)}
				onkeydown={onKeydown}
			>{tab.label}{#if tab.count != null}<span class="count">{tab.count}</span>{/if}</button>
		{/each}
	</div>
{/if}

<style>
	.scope-tabs {
		display: flex;
		align-items: center;
		justify-content: center;
		flex-wrap: wrap;
		gap: 6px;
		width: 100%;
		max-width: var(--measure-command);
		margin: 0 auto;
		min-width: 0;
	}

	.scope-tabs.start {
		justify-content: flex-start;
		max-width: none;
		margin: 0;
	}

	.scope-tab {
		display: inline-flex;
		align-items: center;
		justify-content: center;
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
		text-decoration: none;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.scope-tab:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.scope-tab.active {
		background: var(--accent);
		border-color: var(--accent);
		color: var(--text-on-accent);
		font-weight: var(--font-weight-semibold);
	}

	.scope-tab:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.count {
		color: var(--text-tertiary);
		font-weight: var(--font-weight-medium);
		font-variant-numeric: tabular-nums;
		/* A count that lands after first paint (no cached value yet) fades in
		   rather than popping. */
		animation: scope-count-in var(--motion-base) both;
	}

	@keyframes scope-count-in {
		from { opacity: 0; }
	}

	@media (prefers-reduced-motion: reduce) {
		.count {
			animation: none;
		}
	}

	.scope-tab.active .count {
		color: inherit;
		opacity: 0.78;
	}
</style>
