<script lang="ts">
	// Shared search input primitive. Owns the token-driven field recipe and the
	// faceted fill affordances (focus facet-name popover, inline Tab-completion,
	// removable filter chips) because those are a pure function of the text plus
	// parseQuery. It does NOT own debounce/orchestration or domain keybindings
	// (Enter=play, arrows=cursor, slash mode) - shells forward those via onkeydown.
	import type { Snippet } from 'svelte';
	import { parseQuery, filtersToChips, stripFilter } from '$lib/search/query_parser';
	import { FACETS, matchFacets, inlineCompletionFor, type FacetDescriptor } from '$lib/search/facets';

	interface Props {
		value?: string;
		placeholder?: string;
		ariaLabel?: string;
		variant?: 'page' | 'modal';
		size?: 'md' | 'sm';
		fill?: boolean;
		facets?: boolean;
		inlineCompletion?: boolean;
		filterChips?: boolean;
		suppressSuggestions?: boolean;
		autofocus?: boolean;
		disabled?: boolean;
		inputEl?: HTMLInputElement | null;
		leading?: Snippet;
		trailing?: Snippet;
		oninput?: (value: string) => void;
		onkeydown?: (event: KeyboardEvent) => void;
		onfocus?: () => void;
		onblur?: () => void;
	}

	let {
		value = $bindable(''),
		placeholder = 'Search',
		ariaLabel = undefined,
		variant = 'page',
		size = 'md',
		fill = false,
		facets = false,
		inlineCompletion = false,
		filterChips = false,
		suppressSuggestions = false,
		autofocus = false,
		disabled = false,
		inputEl = $bindable(null),
		leading = undefined,
		trailing = undefined,
		oninput = undefined,
		onkeydown = undefined,
		onfocus = undefined,
		onblur = undefined,
	}: Props = $props();

	let focused = $state(false);

	const parsed = $derived(parseQuery(value));
	const chips = $derived(filterChips ? filtersToChips(parsed.filters) : []);

	// The word currently being typed (after the last space). Empty when the
	// value is empty or ends with a space.
	const tail = $derived(value.slice(value.lastIndexOf(' ') + 1));
	const isEmpty = $derived(value.trim() === '');
	const isSlash = $derived(value.startsWith('/'));

	const suggestions = $derived<FacetDescriptor[]>(
		isEmpty ? FACETS : tail ? matchFacets(tail) : []
	);
	const showPopover = $derived(
		facets && focused && !suppressSuggestions && !isSlash && suggestions.length > 0
	);
	// Tab-completion target: only when the trailing word uniquely prefixes a facet.
	const tabCompletion = $derived(
		inlineCompletion && !suppressSuggestions && !isSlash ? inlineCompletionFor(tail) : null
	);

	let didAutofocus = false;
	$effect(() => {
		if (autofocus && inputEl && !didAutofocus) {
			didAutofocus = true;
			inputEl.focus();
		}
	});

	function emit(next: string) {
		value = next;
		oninput?.(next);
	}

	function handleInput(event: Event) {
		emit((event.currentTarget as HTMLInputElement).value);
	}

	// Replace the trailing partial word with a completed facet token, keep focus.
	function completeTail(token: string) {
		const idx = value.lastIndexOf(' ');
		const head = idx === -1 ? '' : value.slice(0, idx + 1);
		emit(`${head}${token}`);
		inputEl?.focus();
	}

	function handleKeydown(event: KeyboardEvent) {
		if (event.key === 'Tab' && !event.shiftKey && tabCompletion) {
			event.preventDefault();
			completeTail(tabCompletion);
			return;
		}
		onkeydown?.(event);
	}

	function handleFocus() {
		focused = true;
		onfocus?.();
	}

	function handleBlur() {
		focused = false;
		onblur?.();
	}

	function removeChip(key: string) {
		emit(stripFilter(value, key));
		inputEl?.focus();
	}
</script>

<div
	class="sf"
	class:sf--page={variant === 'page'}
	class:sf--modal={variant === 'modal'}
	class:sf--sm={size === 'sm'}
	class:sf--fill={fill}
	class:sf--disabled={disabled}
>
	<div class="sf-shell">
		{#if leading}
			{@render leading()}
		{:else if variant === 'page'}
			<svg class="sf-icon" viewBox="0 0 24 24" fill="none" aria-hidden="true">
				<circle cx="11" cy="11" r="7" stroke="currentColor" stroke-width="2" />
				<path d="M20 20l-3.2-3.2" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
			</svg>
		{/if}
		<input
			bind:this={inputEl}
			class="sf-input"
			type={variant === 'modal' ? 'text' : 'search'}
			{placeholder}
			aria-label={ariaLabel ?? placeholder}
			value={value}
			{disabled}
			autocomplete="off"
			spellcheck={false}
			oninput={handleInput}
			onkeydown={handleKeydown}
			onfocus={handleFocus}
			onblur={handleBlur}
		/>
		{#if trailing}{@render trailing()}{/if}
	</div>

	{#if showPopover}
		<div class="sf-popover" aria-label="Filter suggestions">
			<p class="sf-pop-head">Add a filter</p>
			{#each suggestions as facet (facet.key)}
				<button
					type="button"
					class="sf-suggestion"
					onmousedown={(e) => e.preventDefault()}
					onclick={() => completeTail(facet.token)}
				>
					<span class="sf-tok">{facet.token}</span>
					<span class="sf-label">{facet.label}</span>
					<span class="sf-ex">{facet.example}</span>
				</button>
			{/each}
			{#if tabCompletion}
				<p class="sf-pop-foot"><kbd>Tab</kbd> completes <code>{tabCompletion}</code></p>
			{/if}
		</div>
	{/if}

	{#if filterChips && chips.length > 0}
		<div class="sf-chips">
			{#each chips as chip (chip.key)}
				<button
					type="button"
					class="sf-chip"
					title="Remove filter"
					onclick={() => removeChip(chip.key)}
				>{chip.display}<span class="sf-chip-x">×</span></button>
			{/each}
		</div>
	{/if}
</div>

<style>
	.sf {
		position: relative;
	}
	.sf--page {
		width: 100%;
		max-width: 720px;
		margin: 0 auto;
	}
	.sf--page.sf--sm {
		max-width: none;
	}
	.sf--fill {
		max-width: none;
		margin: 0;
		flex: 1 1 auto;
		min-width: 0;
	}
	.sf--disabled {
		opacity: 0.5;
	}

	.sf-shell {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
	}
	.sf--page .sf-shell {
		background: var(--panel-bg);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-lg);
		padding: 14px 22px;
		transition: border-color var(--motion-fast), background var(--motion-fast),
			box-shadow var(--motion-fast);
	}
	.sf--page.sf--sm .sf-shell {
		padding: 10px 14px;
		border-radius: var(--radius-md);
	}
	.sf--page .sf-shell:focus-within {
		border-color: var(--accent);
		background: var(--input-focus);
		box-shadow: 0 0 0 3px var(--accent-soft);
	}
	.sf--modal .sf-shell {
		padding: 14px 18px;
		border-bottom: 1px solid var(--border-subtle);
	}

	.sf-icon {
		width: 18px;
		height: 18px;
		color: var(--text-tertiary);
		flex-shrink: 0;
	}

	.sf-input {
		flex: 1;
		min-width: 0;
		width: 100%;
		background: none;
		border: none;
		border-radius: 0;
		padding: 0;
		outline: none;
		color: var(--text-primary);
		font-family: inherit;
		font-size: var(--font-size-md);
	}
	.sf--sm .sf-input {
		font-size: var(--font-size-sm);
	}
	.sf-input::placeholder {
		color: var(--text-tertiary);
	}

	.sf-popover {
		position: absolute;
		top: calc(100% + 6px);
		left: 0;
		right: 0;
		z-index: var(--z-overlay);
		background: var(--bg-elevated);
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-md);
		box-shadow: 0 24px 48px -20px rgba(0, 0, 0, 0.65);
		padding: 6px;
		max-height: 320px;
		overflow-y: auto;
	}
	.sf-pop-head,
	.sf-pop-foot {
		margin: 0;
		padding: 6px 10px;
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
	}
	.sf-pop-head {
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}
	.sf-pop-foot {
		display: flex;
		align-items: center;
		gap: 6px;
	}
	.sf-pop-foot code {
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		color: var(--accent-strong);
	}
	.sf-suggestion {
		display: flex;
		align-items: center;
		gap: 10px;
		width: 100%;
		padding: 7px 10px;
		background: none;
		border: none;
		border-radius: var(--radius-sm);
		color: var(--text-primary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		text-align: left;
		cursor: pointer;
		transition: background var(--motion-fast);
	}
	.sf-suggestion:hover {
		background: var(--bg-hover);
	}
	.sf-tok {
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--accent-strong);
		flex-shrink: 0;
		min-width: 92px;
	}
	.sf-label {
		color: var(--text-secondary);
	}
	.sf-ex {
		margin-left: auto;
		font-family: var(--font-mono);
		font-size: var(--font-size-2xs);
		color: var(--text-muted);
		flex-shrink: 0;
	}

	.sf-chips {
		margin: 10px 0 0;
		display: flex;
		gap: 6px;
		flex-wrap: wrap;
	}
	.sf-chip {
		display: inline-flex;
		align-items: center;
		gap: 5px;
		background: var(--bg-elevated);
		border: 1px solid var(--accent-line);
		color: var(--text-secondary);
		border-radius: var(--radius-md);
		padding: 4px 12px;
		font-size: var(--font-size-xs);
		font-family: inherit;
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast),
			color var(--motion-fast);
	}
	.sf-chip:hover {
		background: var(--bg-hover);
		border-color: var(--accent);
		color: var(--text-primary);
	}
	.sf-chip-x {
		font-size: var(--font-size-sm);
		line-height: 1;
		color: var(--text-tertiary);
		margin-left: 2px;
	}
	.sf-chip:hover .sf-chip-x {
		color: var(--text-primary);
	}

	.sf-popover kbd {
		background: var(--bg-raised);
		border: 1px solid var(--border-subtle);
		border-radius: 4px;
		padding: 1px 5px;
		font-size: var(--font-size-2xs);
		font-family: var(--font-mono);
		color: var(--text-secondary);
	}
</style>
