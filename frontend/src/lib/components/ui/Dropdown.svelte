<script lang="ts" generics="T extends string">
	import { tick } from 'svelte';
	import { portal } from '$lib/actions/portal';

	// Picking one value from a list too long for Segmented (STYLING.md
	// "Dropdown"). A sunken trigger opens a listbox portalled into the overlay
	// layer, so no transformed or clipped ancestor can trap it. While open,
	// keyboard focus stays on the list (aria-activedescendant): arrows and
	// typeahead move, Enter or Space choose, Escape closes back to the trigger
	// and Tab closes and carries on from it, like a native select.

	let {
		options,
		value,
		label,
		onchange,
		disabled = false,
		full = false,
	}: {
		options: readonly { value: T; label: string }[];
		value: T;
		/** Accessible name; the trigger reads it with the current value. */
		label: string;
		onchange: (value: T) => void;
		disabled?: boolean;
		/** Stretch the trigger to its container. */
		full?: boolean;
	} = $props();

	const id = $props.id();
	const labelId = `${id}-label`;
	const valueId = `${id}-value`;
	const listId = `${id}-list`;
	const optionId = (index: number) => `${id}-option-${index}`;
	// Matches --motion-exit, so the list unmounts as its exit ends.
	const EXIT_MS = 140;
	const GAP = 6;
	const MARGIN = 8;

	let open = $state(false);
	let closing = $state(false);
	let activeIndex = $state(-1);
	let trigger = $state<HTMLButtonElement>();
	let menu = $state<HTMLElement>();
	let place = $state({ top: 0, left: 0, width: 0, maxHeight: 320, up: false });
	let closeTimer: ReturnType<typeof setTimeout> | undefined;

	let selectedIndex = $derived(options.findIndex((option) => option.value === value));

	function measure() {
		if (!trigger) return;
		const rect = trigger.getBoundingClientRect();
		const below = window.innerHeight - rect.bottom - GAP - MARGIN;
		const above = rect.top - GAP - MARGIN;
		const natural = menu?.scrollHeight ?? 0;
		const up = natural > below && above > below;
		const maxHeight = Math.min(320, up ? above : below);
		const height = Math.min(natural, maxHeight);
		const width = Math.max(rect.width, menu?.offsetWidth ?? 0);
		place = {
			top: up ? rect.top - GAP - height : rect.bottom + GAP,
			left: Math.max(MARGIN, Math.min(rect.left, window.innerWidth - MARGIN - width)),
			width: rect.width,
			maxHeight,
			up,
		};
	}

	async function show() {
		if (disabled) return;
		clearTimeout(closeTimer);
		closing = false;
		activeIndex = selectedIndex < 0 ? 0 : selectedIndex;
		measure();
		open = true;
		await tick();
		measure();
		menu?.focus({ preventScroll: true });
		revealActive();
	}

	function hide(refocus: boolean) {
		if (!open || closing) return;
		closing = true;
		if (refocus) trigger?.focus();
		closeTimer = setTimeout(() => {
			open = false;
			closing = false;
		}, EXIT_MS);
	}

	function choose(index: number) {
		const option = options[index];
		if (option && option.value !== value) onchange(option.value);
		hide(true);
	}

	async function revealActive() {
		await tick();
		document.getElementById(optionId(activeIndex))?.scrollIntoView({ block: 'nearest' });
	}

	// Options take no focus of their own: the list holds it, so pointer
	// events are read off the option under the pointer.
	function optionIndex(event: Event): number {
		const option = (event.target as Element).closest<HTMLElement>('[role="option"]');
		return option ? Number(option.dataset.index) : -1;
	}

	function typeahead(key: string) {
		const needle = key.toLowerCase();
		for (let step = 1; step <= options.length; step += 1) {
			const index = (activeIndex + step) % options.length;
			if (options[index].label.toLowerCase().startsWith(needle)) {
				activeIndex = index;
				return;
			}
		}
	}

	// Space is play/pause on the window, so the trigger opens on it here, as a
	// native select would; arrows (volume on the window) open it too.
	function onTriggerKeydown(event: KeyboardEvent) {
		if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp' && event.key !== ' ') return;
		event.preventDefault();
		event.stopPropagation();
		if (!open || closing) void show();
	}

	function onMenuKeydown(event: KeyboardEvent) {
		// The open list owns the keyboard: letters are typeahead here, not the
		// window's player shortcuts (L likes, S shuffles, Space pauses).
		event.stopPropagation();
		const last = options.length - 1;
		switch (event.key) {
			case 'ArrowDown':
				activeIndex = Math.min(last, activeIndex + 1);
				break;
			case 'ArrowUp':
				activeIndex = Math.max(0, activeIndex - 1);
				break;
			case 'Home':
				activeIndex = 0;
				break;
			case 'End':
				activeIndex = last;
				break;
			case 'Enter':
			case ' ':
				choose(activeIndex);
				break;
			case 'Escape':
				hide(true);
				break;
			case 'Tab':
				// Back on the trigger before the browser moves focus, so Tab and
				// Shift+Tab carry on from where the list was opened.
				hide(true);
				return;
			default:
				if (event.key.length !== 1 || event.ctrlKey || event.metaKey || event.altKey) return;
				typeahead(event.key);
		}
		event.preventDefault();
		void revealActive();
	}

	$effect(() => {
		if (!open) return;
		const onPointerDown = (event: PointerEvent) => {
			const target = event.target as Node;
			if (menu?.contains(target) || trigger?.contains(target)) return;
			hide(false);
		};
		// The list is fixed to where the trigger was; any page scroll or resize
		// would leave it behind, so it closes instead of drifting.
		const onScroll = (event: Event) => {
			if (menu && event.target instanceof Node && menu.contains(event.target)) return;
			hide(false);
		};
		const onResize = () => hide(false);
		window.addEventListener('pointerdown', onPointerDown, true);
		window.addEventListener('scroll', onScroll, true);
		window.addEventListener('resize', onResize);
		return () => {
			window.removeEventListener('pointerdown', onPointerDown, true);
			window.removeEventListener('scroll', onScroll, true);
			window.removeEventListener('resize', onResize);
		};
	});

	$effect(() => () => clearTimeout(closeTimer));
</script>

<span id={labelId} hidden>{label}</span>
<button
	bind:this={trigger}
	type="button"
	class="trigger"
	class:full
	aria-haspopup="listbox"
	aria-expanded={open}
	aria-controls={open ? listId : undefined}
	aria-labelledby="{labelId} {valueId}"
	{disabled}
	onclick={() => (open && !closing ? hide(true) : void show())}
	onkeydown={onTriggerKeydown}
>
	<span id={valueId} class="value">{options[selectedIndex]?.label ?? ''}</span>
	<svg class="chevron" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
		<path d="M3 4.5 6 7.5 9 4.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
	</svg>
</button>

{#if open}
	<div
		use:portal
		bind:this={menu}
		id={listId}
		class="menu"
		class:up={place.up}
		class:closing
		role="listbox"
		aria-labelledby={labelId}
		aria-activedescendant={activeIndex >= 0 ? optionId(activeIndex) : undefined}
		tabindex="-1"
		style:top="{place.top}px"
		style:left="{place.left}px"
		style:min-width="{place.width}px"
		style:max-height="{place.maxHeight}px"
		onkeydown={onMenuKeydown}
		onpointermove={(event) => {
			const index = optionIndex(event);
			if (index >= 0) activeIndex = index;
		}}
		onclick={(event) => {
			const index = optionIndex(event);
			if (index >= 0) choose(index);
		}}
	>
		{#each options as option, index (option.value)}
			<div
				id={optionId(index)}
				class="option"
				class:active={index === activeIndex}
				role="option"
				aria-selected={index === selectedIndex}
				data-index={index}
			>
				<span>{option.label}</span>
				{#if index === selectedIndex}
					<svg class="check" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
						<path d="M2.5 6.5 5 9l4.5-6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" />
					</svg>
				{/if}
			</div>
		{/each}
	</div>
{/if}

<style>
	.trigger {
		display: inline-flex;
		align-items: center;
		justify-content: space-between;
		gap: 8px;
		max-width: 100%;
		height: var(--control-h);
		padding: 0 10px 0 14px;
		border: 0;
		border-radius: 999px;
		background: var(--bg-surface);
		color: var(--text-primary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.trigger.full {
		width: 100%;
	}

	.trigger:hover:not(:disabled),
	.trigger[aria-expanded='true'] {
		background: var(--bg-hover);
	}

	.trigger:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.value {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.chevron {
		flex: none;
		opacity: 0.6;
		transition: transform var(--motion-fast);
	}

	.trigger[aria-expanded='true'] .chevron {
		transform: rotate(180deg);
	}

	.menu {
		position: fixed;
		z-index: var(--z-overlay);
		box-sizing: border-box;
		max-width: min(360px, calc(100vw - 16px));
		overflow-y: auto;
		padding: 6px;
		background: color-mix(in srgb, var(--bg-surface-strong) 94%, transparent);
		backdrop-filter: var(--blur-modal);
		-webkit-backdrop-filter: var(--blur-modal);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		box-shadow: var(--panel-shadow);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		scrollbar-width: thin;
		transform-origin: top center;
		animation: menu-in var(--motion-fast) both;
	}

	.menu.up {
		transform-origin: bottom center;
	}

	.menu.closing {
		pointer-events: none;
		animation: menu-out var(--motion-exit) both;
	}

	/* The active option carries the keyboard focus (aria-activedescendant). */
	.menu:focus-visible {
		outline: none;
	}

	.option {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding: 8px 10px;
		border-radius: 8px;
		color: var(--text-secondary);
		white-space: nowrap;
		cursor: pointer;
	}

	.option.active {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.menu:focus-visible .option.active {
		outline: 2px solid var(--accent-strong);
		outline-offset: -2px;
	}

	.option[aria-selected='true'] {
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
	}

	.check {
		flex: none;
		color: var(--accent-strong);
	}

	@keyframes menu-in {
		from {
			opacity: 0;
			transform: scale(0.98);
		}
	}

	@keyframes menu-out {
		to {
			opacity: 0;
			transform: scale(0.98);
		}
	}

	@keyframes menu-fade-in {
		from {
			opacity: 0;
		}
	}

	@keyframes menu-fade-out {
		to {
			opacity: 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.menu {
			animation-name: menu-fade-in;
		}

		.menu.closing {
			animation-name: menu-fade-out;
		}

		.chevron {
			transition: none;
		}
	}
</style>
