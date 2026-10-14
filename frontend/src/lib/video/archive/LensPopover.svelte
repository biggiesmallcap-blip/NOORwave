<script lang="ts">
	import { tick, type Snippet } from 'svelte';
	import { portal } from '$lib/actions/portal';

	// One lens in the Archive's lens row: a pill that names the lens, or what
	// is picked once something is (with an x that puts it down), and a panel
	// under it with the choices. The panel is portalled into the overlay layer
	// like Dropdown's list, so no clipped ancestor traps it. It closes on
	// Escape, an outside press, a page scroll or a resize, and when the body
	// calls the close it is handed.

	let {
		label,
		value = null,
		hue = null,
		width = 320,
		disabled = false,
		onclear,
		panel,
	}: {
		/** The lens name, shown while nothing is picked. */
		label: string;
		/** What is picked, in words; null shows the lens name. */
		value?: string | null;
		/** A scene's colour for the dot in the pill. */
		hue?: string | null;
		/** Panel width in px, clamped to the window. */
		width?: number;
		disabled?: boolean;
		onclear: () => void;
		panel: Snippet<[() => void]>;
	} = $props();

	const id = $props.id();
	const panelId = `${id}-panel`;
	const GAP = 8;
	const MARGIN = 12;
	// Matches --motion-exit, so the panel unmounts as its exit ends.
	const EXIT_MS = 140;

	let open = $state(false);
	let closing = $state(false);
	let trigger = $state<HTMLButtonElement>();
	let sheet = $state<HTMLElement>();
	let place = $state({ top: 0, left: 0, width: 320, maxHeight: 480 });
	let closeTimer: ReturnType<typeof setTimeout> | undefined;

	function measure() {
		if (!trigger) return;
		const rect = trigger.getBoundingClientRect();
		const w = Math.min(width, window.innerWidth - MARGIN * 2);
		place = {
			top: rect.bottom + GAP,
			left: Math.max(MARGIN, Math.min(rect.left, window.innerWidth - MARGIN - w)),
			width: w,
			maxHeight: Math.max(220, window.innerHeight - rect.bottom - GAP - MARGIN),
		};
	}

	async function show() {
		if (disabled) return;
		clearTimeout(closeTimer);
		closing = false;
		measure();
		open = true;
		await tick();
		const first = sheet?.querySelector<HTMLElement>('[data-autofocus]');
		(first ?? sheet)?.focus({ preventScroll: true });
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

	// The open panel owns the keyboard: letters typed into its search are not
	// the window's player shortcuts.
	function onSheetKeydown(event: KeyboardEvent) {
		event.stopPropagation();
		if (event.key === 'Escape') {
			event.preventDefault();
			hide(true);
		}
	}

	$effect(() => {
		if (!open) return;
		const onPointerDown = (event: PointerEvent) => {
			const target = event.target as Node;
			if (sheet?.contains(target) || trigger?.contains(target)) return;
			hide(false);
		};
		const onScroll = (event: Event) => {
			if (sheet && event.target instanceof Node && sheet.contains(event.target)) return;
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

<span class="lens" class:set={value != null} class:open>
	<button
		bind:this={trigger}
		type="button"
		class="pill"
		aria-haspopup="dialog"
		aria-expanded={open}
		aria-controls={open ? panelId : undefined}
		aria-label={value != null ? `${label}: ${value}` : label}
		{disabled}
		onclick={() => (open && !closing ? hide(true) : void show())}
	>
		{#if hue}<span class="dot" style:background={hue} aria-hidden="true"></span>{/if}
		<span class="text">{value ?? label}</span>
		{#if value == null}
			<svg class="chevron" viewBox="0 0 12 12" width="12" height="12" aria-hidden="true">
				<path d="M3 4.5 6 7.5 9 4.5" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
			</svg>
		{/if}
	</button>
	{#if value != null}
		<button type="button" class="clear" aria-label={`Clear ${label.toLowerCase()}`} onclick={onclear}>
			<svg viewBox="0 0 12 12" width="10" height="10" aria-hidden="true">
				<path d="M3 3l6 6M9 3l-6 6" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
			</svg>
		</button>
	{/if}
</span>

{#if open}
	<div
		use:portal
		bind:this={sheet}
		id={panelId}
		class="sheet"
		class:closing
		role="dialog"
		aria-label={label}
		tabindex="-1"
		style:top="{place.top}px"
		style:left="{place.left}px"
		style:width="{place.width}px"
		style:max-height="{place.maxHeight}px"
		onkeydown={onSheetKeydown}
	>
		{@render panel(() => hide(true))}
	</div>
{/if}

<style>
	/* A pill on the ground: hairline at rest, the accent wash once a value is
	   picked, with the x inside the same pill. */
	.lens {
		display: inline-flex;
		align-items: center;
		height: var(--control-h);
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		color: var(--text-secondary);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.lens:hover,
	.lens.open {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.lens.set {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.pill {
		display: inline-flex;
		align-items: center;
		gap: 7px;
		height: 100%;
		max-width: 26ch;
		padding: 0 12px 0 14px;
		border: 0;
		border-radius: 999px;
		background: none;
		color: inherit;
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		white-space: nowrap;
		cursor: pointer;
	}

	.lens.set .pill {
		padding-right: 4px;
		font-weight: var(--font-weight-semibold);
	}

	.pill:disabled {
		cursor: default;
		opacity: 0.55;
	}

	.text {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex: 0 0 auto;
	}

	.chevron {
		flex: none;
		opacity: 0.55;
		transition: transform var(--motion-fast);
	}

	.lens.open .chevron {
		transform: rotate(180deg);
	}

	.clear {
		display: inline-grid;
		place-items: center;
		width: 26px;
		height: 26px;
		margin-right: 2px;
		padding: 0;
		border: 0;
		border-radius: 50%;
		background: none;
		color: inherit;
		cursor: pointer;
	}

	.clear:hover {
		background: var(--accent-soft);
	}

	.pill:focus-visible,
	.clear:focus-visible {
		outline: 2px solid var(--accent-strong);
		outline-offset: 2px;
	}

	.sheet {
		position: fixed;
		z-index: var(--z-overlay);
		box-sizing: border-box;
		display: grid;
		align-content: start;
		overflow-y: auto;
		padding: 10px;
		background: color-mix(in srgb, var(--bg-surface-strong) 96%, transparent);
		backdrop-filter: var(--blur-modal);
		-webkit-backdrop-filter: var(--blur-modal);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		box-shadow: var(--panel-shadow);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		scrollbar-width: thin;
		transform-origin: top left;
		animation: sheet-in var(--motion-fast) both;
	}

	.sheet:focus-visible {
		outline: none;
	}

	.sheet.closing {
		pointer-events: none;
		animation: sheet-out var(--motion-exit) both;
	}

	@keyframes sheet-in {
		from {
			opacity: 0;
			transform: translateY(-4px) scale(0.99);
		}
	}

	@keyframes sheet-out {
		to {
			opacity: 0;
			transform: translateY(-4px) scale(0.99);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.sheet,
		.sheet.closing {
			animation: none;
		}
	}
</style>
