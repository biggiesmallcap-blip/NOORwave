<script lang="ts">
	import { tick } from 'svelte';
	import { portal } from '$lib/actions/portal';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import { setTuneMode, tuneMode } from './onair/tune_mode';
	import { VIEWING_SCOPES, curtainScope, videoCredits, type ViewingScope } from './viewing';

	// Viewing, under the player: how videos behave as they change. Channel
	// timing (join where the guide says, like TV, or start each video from
	// the beginning), then the curtain between videos and the song title
	// card, each off, on channels only, or on every video. Kept per device.

	let open = $state(false);
	let root = $state<HTMLDivElement>();
	let trigger = $state<HTMLButtonElement>();
	let panel = $state<HTMLDivElement>();
	// The panel lives at the top of the app (portal), placed by the button:
	// the page's sections form their own layers, and a panel inside them
	// would open under the grid that follows. Below the button when it fits,
	// else above it (the button sits low, under the player).
	const GAP = 8;
	const MARGIN = 16;
	let place = $state<{ top: number | null; bottom: number | null; right: number }>({ top: 0, bottom: null, right: 0 });

	function measure() {
		if (!trigger) return;
		const rect = trigger.getBoundingClientRect();
		const height = panel?.offsetHeight ?? 0;
		const right = Math.max(MARGIN, window.innerWidth - rect.right);
		const fitsBelow = rect.bottom + GAP + height <= window.innerHeight - MARGIN;
		place = fitsBelow || rect.top < height + GAP + MARGIN
			? { top: rect.bottom + GAP, bottom: null, right }
			: { top: null, bottom: window.innerHeight - rect.top + GAP, right };
	}

	async function toggle() {
		if (open) {
			close();
			return;
		}
		measure();
		open = true;
		await tick();
		measure();
	}

	function close(refocus = false) {
		cancelLeave();
		open = false;
		if (refocus) trigger?.focus();
	}

	// It goes once the pointer has left the button and the panel, after a
	// short grace so the way from one to the other does not close it.
	const LEAVE_MS = 450;
	let leaveTimer: ReturnType<typeof setTimeout> | null = null;

	function cancelLeave() {
		if (leaveTimer) clearTimeout(leaveTimer);
		leaveTimer = null;
	}

	function leave(event: PointerEvent) {
		if (!open || event.pointerType === 'touch') return;
		cancelLeave();
		leaveTimer = setTimeout(() => close(), LEAVE_MS);
	}

	function onWindowPointer(event: PointerEvent) {
		const target = event.target as Node;
		if (open && !root?.contains(target) && !panel?.contains(target)) close();
	}

	function onWindowKey(event: KeyboardEvent) {
		if (open && event.key === 'Escape') {
			event.stopPropagation();
			close(true);
		}
	}
</script>

<svelte:window onpointerdown={onWindowPointer} onkeydown={onWindowKey} onresize={() => open && measure()} onscrollcapture={() => open && measure()} />

<div class="viewing" bind:this={root} onpointerenter={cancelLeave} onpointerleave={leave} role="presentation">
	<button
		type="button"
		class="pill-btn"
		bind:this={trigger}
		aria-expanded={open}
		aria-haspopup="dialog"
		onclick={() => void toggle()}
	>
		<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 7h10M18 7h2M4 17h4M12 17h8" /><circle cx="16" cy="7" r="2" /><circle cx="10" cy="17" r="2" /></svg>
		Viewing
	</button>

	{#if open}
		<div
			class="panel"
			role="dialog"
			aria-label="Viewing"
			tabindex="-1"
			data-floating-layer
			use:portal
			bind:this={panel}
			onpointerenter={cancelLeave}
			onpointerleave={leave}
			style:top={place.top !== null ? `${place.top}px` : null}
			style:bottom={place.bottom !== null ? `${place.bottom}px` : null}
			style:right={`${place.right}px`}
		>
			<div class="setting">
				<div class="words">
					<p class="name">Sync channels with the guide</p>
					<p class="hint">Join a channel where it is, mid-video, like TV. Off: each video from the start.</p>
				</div>
				<Toggle
					checked={$tuneMode === 'live'}
					label="Sync channels with the guide"
					onchange={(event) => setTuneMode(event.currentTarget.checked ? 'live' : 'start')}
				/>
			</div>

			<div class="setting stacked">
				<div class="words">
					<p class="name">Curtain between videos</p>
					<p class="hint">The next video's still, under grain, while it loads.</p>
				</div>
				<Segmented
					options={VIEWING_SCOPES}
					value={$curtainScope}
					label="Curtain between videos"
					onchange={(scope: ViewingScope) => curtainScope.set(scope)}
				/>
			</div>

			<div class="setting stacked">
				<div class="words">
					<p class="name">Song titles</p>
					<p class="hint">The title card as a video starts, and what is up next as it ends.</p>
				</div>
				<Segmented
					options={VIEWING_SCOPES}
					value={$videoCredits}
					label="Song titles"
					onchange={(scope: ViewingScope) => videoCredits.set(scope)}
				/>
			</div>
		</div>
	{/if}
</div>

<style>
	.viewing {
		position: relative;
	}

	/* The same pill as Save and Close beside it. */
	.pill-btn {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		height: var(--control-h);
		padding: 0 14px;
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: transparent;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		cursor: pointer;
	}

	.pill-btn:hover,
	.pill-btn:focus-visible,
	.pill-btn[aria-expanded='true'] {
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.pill-btn svg {
		width: 15px;
		height: 15px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
	}

	.pill-btn circle {
		fill: var(--bg-base, transparent);
	}

	.panel {
		position: fixed;
		z-index: var(--z-overlay);
		display: grid;
		gap: var(--space-4);
		width: min(360px, calc(100vw - 32px));
		padding: var(--space-4);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: var(--bg-surface-strong);
		box-shadow: var(--panel-shadow);
		animation: panel-in var(--motion-fast) both;
	}

	@keyframes panel-in {
		from { opacity: 0; transform: translateY(-4px); }
	}

	.setting {
		display: grid;
		grid-template-columns: minmax(0, 1fr) auto;
		align-items: center;
		gap: var(--space-3);
	}

	.setting.stacked {
		grid-template-columns: minmax(0, 1fr);
		gap: var(--space-2);
	}

	.words {
		display: grid;
		gap: 2px;
		min-width: 0;
	}

	.name {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.hint {
		margin: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	@media (prefers-reduced-motion: reduce) {
		.panel {
			animation: none;
		}
	}
</style>
