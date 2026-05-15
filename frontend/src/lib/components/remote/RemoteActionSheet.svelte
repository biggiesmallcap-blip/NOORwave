<script lang="ts">
	import { actionSheet, closeActionSheet } from '$lib/remote/action_sheet';
	import type { MenuItem } from '$lib/stores/context_menu';
	import { tick } from 'svelte';

	const DISMISS_PX = 80;
	const EXIT_MS = 300;

	let dragOffset = $state(0);
	let dragStartY = 0;
	let dragging = $state(false);
	// "presented" toggles from false → true on open (drives the slide-up
	// transition) and back to false to dismiss (slide-down). Decoupled from
	// $actionSheet.open so we can animate the exit before tearing the store
	// data down.
	let presented = $state(false);
	let dismissing = $state(false);

	let sheetEl: HTMLDivElement | null = $state(null);
	let listEl: HTMLUListElement | null = $state(null);

	// iOS Safari fires a synthetic click on whatever element is under the
	// finger when the user lifts their long-press finger. Suppress that click
	// for a short window after the sheet opens — pointer events still flow
	// normally so the swipe-down gesture works immediately.
	const OPEN_GRACE_MS = 500;
	let openedAt = 0;

	function suppressGhostClicks(event: MouseEvent) {
		if (Date.now() - openedAt >= OPEN_GRACE_MS) return;
		const target = event.target;
		if (!(target instanceof Node)) return;
		if (sheetEl?.contains(target) || (target as Element).closest?.('.remote-actions-overlay')) {
			event.stopPropagation();
			event.preventDefault();
		}
	}

	$effect(() => {
		if (!$actionSheet.open) return;
		// Reset drag state for the new session.
		dragOffset = 0;
		dragging = false;
		dismissing = false;
		presented = false;
		openedAt = Date.now();

		// Force one frame at presented=false so the browser commits the
		// off-screen transform, then flip to presented=true on the next frame
		// so the transition has a from/to pair to animate between.
		void tick().then(() => {
			requestAnimationFrame(() => {
				requestAnimationFrame(() => {
					presented = true;
				});
			});
		});

		const prev = document.body.style.overflow;
		document.body.style.overflow = 'hidden';
		window.addEventListener('click', suppressGhostClicks, true);
		return () => {
			window.removeEventListener('click', suppressGhostClicks, true);
			document.body.style.overflow = prev;
		};
	});

	function animateDismiss() {
		if (dismissing) return;
		dismissing = true;
		dragging = false;
		dragOffset = 0;
		// Flip back to off-screen — the .remote-actions-sheet transition does
		// the slide. Tear down the store data after the transition completes.
		presented = false;
		setTimeout(() => {
			closeActionSheet();
		}, EXIT_MS);
	}

	async function pick(item: MenuItem) {
		if (item.disabled || item.separator || !item.onSelect) return;
		// Close immediately on a real selection — the menu choice itself is
		// the dismiss signal and any nav/toast wants to land right away.
		closeActionSheet();
		try {
			await item.onSelect();
		} catch {
			// Per-action errors surface via their own toast pathways.
		}
	}

	function onSheetPointerDown(event: PointerEvent) {
		if (dismissing) return;
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		const target = event.target;
		if (target instanceof Element) {
			// Don't start a drag from inside an interactive control or from a
			// scrolled-down menu list — let the user keep scrolling/tapping.
			if (target.closest('button, a, input, select, textarea')) return;
			if (listEl && listEl.contains(target) && listEl.scrollTop > 0) return;
		}
		dragging = true;
		dragStartY = event.clientY;
		dragOffset = 0;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function onSheetPointerMove(event: PointerEvent) {
		if (!dragging) return;
		const dy = event.clientY - dragStartY;
		if (dy <= 0) {
			dragOffset = 0;
			return;
		}
		dragOffset = dy;
	}

	function onSheetPointerUp(event: PointerEvent) {
		if (!dragging) return;
		dragging = false;
		try {
			(event.currentTarget as HTMLElement).releasePointerCapture(event.pointerId);
		} catch {
			// Already released — ignore.
		}
		if (dragOffset >= DISMISS_PX) {
			animateDismiss();
		} else {
			dragOffset = 0;
		}
	}
</script>

{#if $actionSheet.open}
	<div
		class="remote-actions-overlay"
		class:presented
		role="dialog"
		aria-modal="true"
		aria-label="Track actions"
	>
		<button
			type="button"
			class="remote-actions-scrim"
			aria-label="Close actions"
			onclick={() => animateDismiss()}
		></button>

		<div
			bind:this={sheetEl}
			class="remote-actions-sheet"
			class:presented
			class:dragging
			style="--drag-y: {Math.max(0, dragOffset)}px;"
			onpointerdown={onSheetPointerDown}
			onpointermove={onSheetPointerMove}
			onpointerup={onSheetPointerUp}
			onpointercancel={onSheetPointerUp}
			role="presentation"
		>
			<div class="remote-actions-handle" aria-hidden="true">
				<span class="remote-actions-grab"></span>
			</div>

			{#if $actionSheet.title}
				<header class="remote-actions-head">
					<strong>{$actionSheet.title}</strong>
					{#if $actionSheet.subtitle}
						<small>{$actionSheet.subtitle}</small>
					{/if}
				</header>
			{/if}

			<ul class="remote-actions-list" bind:this={listEl}>
				{#each $actionSheet.items as item, i (i)}
					{#if item.separator}
						<li class="remote-actions-sep" aria-hidden="true"></li>
					{:else}
						<li>
							<button
								type="button"
								class="remote-actions-row"
								class:danger={item.danger}
								disabled={item.disabled}
								onclick={() => void pick(item)}
							>
								{#if item.icon}
									<span class="remote-actions-icon" aria-hidden="true">{item.icon}</span>
								{:else}
									<span class="remote-actions-icon-empty" aria-hidden="true"></span>
								{/if}
								<span class="remote-actions-copy">
									<span class="remote-actions-label">{item.label}</span>
									{#if item.hint}
										<span class="remote-actions-hint">{item.hint}</span>
									{/if}
								</span>
							</button>
						</li>
					{/if}
				{/each}
			</ul>
		</div>
	</div>
{/if}

<style>
	.remote-actions-overlay {
		position: fixed;
		inset: 0;
		z-index: 70;
	}

	/* While the sheet is sliding out (presented flips false but the element
	   stays mounted for the transition), kill pointer events on the whole
	   overlay so taps land on whatever is behind it. Without this, the back
	   button under the scrim swallows the first 1-3 taps that happen during
	   the 300ms exit animation. */
	.remote-actions-overlay:not(.presented) {
		pointer-events: none;
	}

	.remote-actions-scrim {
		position: absolute;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(6px);
		-webkit-backdrop-filter: blur(6px);
		opacity: 0;
		transition: opacity 280ms ease;
	}

	.remote-actions-overlay.presented .remote-actions-scrim {
		opacity: 1;
	}

	.remote-actions-sheet {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		max-height: 80svh;
		display: grid;
		gap: 8px;
		padding: 0 12px max(20px, env(safe-area-inset-bottom));
		background: var(--bg-base);
		border-top-left-radius: 22px;
		border-top-right-radius: 22px;
		box-shadow: 0 -24px 60px rgba(0, 0, 0, 0.45);
		/* Compose the slide-base (100% off-screen → 0 when presented) with the
		   live drag offset. A single transform property keeps the transition
		   smooth across drag → release → animate-out. */
		--base-y: 100%;
		transform: translate3d(0, calc(var(--base-y) + var(--drag-y, 0px)), 0);
		transition:
			transform 300ms cubic-bezier(0.22, 1, 0.36, 1),
			opacity 280ms ease;
		opacity: 0;
		will-change: transform, opacity;
		touch-action: none;
	}

	.remote-actions-sheet.presented {
		--base-y: 0%;
		opacity: 1;
	}

	.remote-actions-sheet.dragging {
		transition: none;
	}

	.remote-actions-handle {
		display: grid;
		place-items: center;
		padding: 10px 0 6px;
	}

	.remote-actions-grab {
		display: block;
		width: 42px;
		height: 4px;
		border-radius: 999px;
		background: var(--surface-2);
	}

	.remote-actions-head {
		display: grid;
		gap: 2px;
		padding: 4px 16px 8px;
	}

	.remote-actions-head strong {
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-actions-head small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		display: block;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-actions-list {
		list-style: none;
		margin: 0;
		padding: 0 4px;
		display: grid;
		gap: 2px;
		overflow-y: auto;
		-webkit-overflow-scrolling: touch;
		touch-action: pan-y;
	}

	.remote-actions-sep {
		height: 1px;
		margin: 6px 12px;
		background: var(--surface-2);
	}

	.remote-actions-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px 12px;
		border-radius: 12px;
		background: transparent;
		color: var(--text-primary);
		text-align: left;
		width: 100%;
		min-height: 52px;
	}

	.remote-actions-row:active {
		background: var(--surface-1);
	}

	.remote-actions-row.danger {
		color: var(--state-error);
	}

	.remote-actions-row:disabled {
		opacity: 0.4;
	}

	.remote-actions-icon,
	.remote-actions-icon-empty {
		width: 24px;
		text-align: center;
		color: var(--text-muted);
		flex-shrink: 0;
	}

	.remote-actions-row.danger .remote-actions-icon {
		color: var(--state-error);
	}

	.remote-actions-copy {
		min-width: 0;
		display: grid;
		gap: 1px;
	}

	.remote-actions-label,
	.remote-actions-hint {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.remote-actions-hint {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}
</style>
