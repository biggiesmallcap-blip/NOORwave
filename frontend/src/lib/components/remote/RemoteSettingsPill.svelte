<script lang="ts">
	import { audioSettings } from '$lib/stores/audio_settings';
	import { exclusiveStatus } from '$lib/stores/exclusive_status';
	import { hapticsEnabled, toggleHaptics } from '$lib/remote/haptics_settings';
	import { hapticTap, hapticAccent } from '$lib/remote/haptics';
	import {
		cancelSleepTimer,
		sleepTimer,
		startSleepTimer
	} from '$lib/remote/sleep_timer';

	let open = $state(false);

	let settings = $derived($audioSettings.settings);
	let loading = $derived($audioSettings.loading || $audioSettings.pendingApply);

	let exclusiveOn = $derived(settings?.exclusive_mode === true);
	let sampleFollowOn = $derived(settings?.sample_rate_follow === true);
	let exclusiveEngaged = $derived($exclusiveStatus.engaged);
	let exclusiveFailureReason = $derived($exclusiveStatus.failureReason);

	const SLEEP_OPTIONS = [15, 30, 45, 60];

	// Live countdown for the sleep-timer label. Updates once per 10s so the
	// remaining-minutes label stays roughly current without churning state.
	let now = $state(Date.now());
	$effect(() => {
		if (!$sleepTimer.fireAt) return;
		const id = setInterval(() => {
			now = Date.now();
		}, 10_000);
		return () => clearInterval(id);
	});

	let sleepRemainingLabel = $derived.by(() => {
		const fireAt = $sleepTimer.fireAt;
		if (!fireAt) return null;
		const remainingMs = Math.max(0, fireAt - now);
		const mins = Math.max(1, Math.ceil(remainingMs / 60_000));
		return `Stops in ${mins} min`;
	});

	let pillLabel = $derived.by(() => {
		if (!settings) return 'Output';
		if (exclusiveOn && exclusiveEngaged) return 'Exclusive';
		if (exclusiveOn && exclusiveFailureReason) return 'Excl · failed';
		if (exclusiveOn) return 'Excl · idle';
		return 'Shared';
	});

	let dragOffset = $state(0);
	let dragStartY = 0;
	let dragging = $state(false);
	const DISMISS_PX = 70;

	$effect(() => {
		if (!open) return;
		const prev = document.body.style.overflow;
		document.body.style.overflow = 'hidden';
		return () => {
			document.body.style.overflow = prev;
		};
	});

	async function toggleExclusive() {
		if (!settings) return;
		hapticTap();
		await audioSettings.patch({ exclusive_mode: !settings.exclusive_mode });
	}

	async function toggleSampleRateFollow() {
		if (!settings) return;
		hapticTap();
		await audioSettings.patch({ sample_rate_follow: !settings.sample_rate_follow });
	}

	function onToggleHaptics() {
		// Pulse *before* flipping the store so users feel the cue once on the
		// transition from off→on (otherwise the next tap is the first feedback).
		if (!$hapticsEnabled) hapticAccent();
		toggleHaptics();
	}

	function onPickSleepDuration(minutes: number) {
		hapticAccent();
		startSleepTimer(minutes);
	}

	function onCancelSleep() {
		hapticTap();
		cancelSleepTimer();
	}

	function close() {
		open = false;
		dragOffset = 0;
	}

	async function openSheet() {
		if (!settings) {
			await audioSettings.load();
		}
		open = true;
	}

	function onHandleDown(event: PointerEvent) {
		if (event.pointerType === 'mouse' && event.button !== 0) return;
		dragging = true;
		dragStartY = event.clientY;
		dragOffset = 0;
		(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
	}

	function onHandleMove(event: PointerEvent) {
		if (!dragging) return;
		const dy = event.clientY - dragStartY;
		dragOffset = dy > 0 ? dy : dy * 0.2;
	}

	function onHandleUp() {
		if (!dragging) return;
		dragging = false;
		if (dragOffset >= DISMISS_PX) close();
		else dragOffset = 0;
	}
</script>

<button
	type="button"
	class="remote-settings-pill"
	class:active={exclusiveOn}
	aria-label="Output settings"
	onclick={() => void openSheet()}
>
	<svg viewBox="0 0 24 24" aria-hidden="true" focusable="false">
		<path
			d="M4 8h11M19 8h1M4 16h5M13 16h7M15 5.5v5M9 13.5v5"
			fill="none"
			stroke="currentColor"
			stroke-width="1.8"
			stroke-linecap="round"
		/>
	</svg>
	<span>{pillLabel}</span>
</button>

{#if open}
	<div class="remote-settings-overlay" role="dialog" aria-modal="true" aria-label="Output settings">
		<button
			type="button"
			class="remote-settings-scrim"
			aria-label="Close output settings"
			onclick={close}
		></button>

		<div
			class="remote-settings-sheet"
			class:dragging
			style="--drag-y: {Math.max(0, dragOffset)}px;"
		>
			<div
				class="remote-settings-handle"
				role="presentation"
				onpointerdown={onHandleDown}
				onpointermove={onHandleMove}
				onpointerup={onHandleUp}
				onpointercancel={onHandleUp}
			>
				<span class="remote-settings-grab" aria-hidden="true"></span>
			</div>

			<header class="remote-settings-head">
				<h2>Output</h2>
				{#if loading}
					<small>Applying…</small>
				{/if}
			</header>

			{#if !settings}
				<p class="remote-settings-empty">Loading…</p>
			{:else}
				<div class="remote-settings-rows">
					<button
						type="button"
						class="remote-settings-row"
						class:on={exclusiveOn}
						aria-pressed={exclusiveOn}
						onclick={() => void toggleExclusive()}
					>
						<span class="remote-settings-row-copy">
							<strong>Exclusive mode</strong>
							<small>
								{#if exclusiveOn && exclusiveEngaged}
									Engaged · bit-perfect output
								{:else if exclusiveOn && exclusiveFailureReason}
									Couldn't grab device. Falling back to shared.
								{:else if exclusiveOn}
									Waiting for playback to grab the device.
								{:else}
									Shared output. Other apps can mix in.
								{/if}
							</small>
						</span>
						<span class="remote-settings-switch" aria-hidden="true">
							<span class="remote-settings-knob"></span>
						</span>
					</button>

					<button
						type="button"
						class="remote-settings-row"
						class:on={sampleFollowOn}
						aria-pressed={sampleFollowOn}
						onclick={() => void toggleSampleRateFollow()}
					>
						<span class="remote-settings-row-copy">
							<strong>Sample-rate follow</strong>
							<small>
								{sampleFollowOn
									? 'Output matches the source rate of every track.'
									: 'Output stays at the device default rate.'}
							</small>
						</span>
						<span class="remote-settings-switch" aria-hidden="true">
							<span class="remote-settings-knob"></span>
						</span>
					</button>

					<button
						type="button"
						class="remote-settings-row"
						class:on={$hapticsEnabled}
						aria-pressed={$hapticsEnabled}
						onclick={onToggleHaptics}
					>
						<span class="remote-settings-row-copy">
							<strong>Haptic feedback</strong>
							<small>
								{$hapticsEnabled
									? 'Short buzz on swipes, toggles, and timer events.'
									: 'No vibration cues anywhere on the remote.'}
							</small>
						</span>
						<span class="remote-settings-switch" aria-hidden="true">
							<span class="remote-settings-knob"></span>
						</span>
					</button>
				</div>

				<section class="remote-settings-sleep">
					<header>
						<strong>Sleep timer</strong>
						<small>
							{#if sleepRemainingLabel}
								{sleepRemainingLabel}
							{:else}
								Pause playback after a few minutes.
							{/if}
						</small>
					</header>
					<div class="remote-settings-sleep-row">
						{#each SLEEP_OPTIONS as mins (mins)}
							<button
								type="button"
								class="remote-settings-sleep-chip"
								class:active={$sleepTimer.minutes === mins && $sleepTimer.fireAt !== null}
								aria-label="Stop after {mins} minutes"
								onclick={() => onPickSleepDuration(mins)}
							>
								{mins}m
							</button>
						{/each}
						{#if $sleepTimer.fireAt !== null}
							<button
								type="button"
								class="remote-settings-sleep-cancel"
								aria-label="Cancel sleep timer"
								onclick={onCancelSleep}
							>
								Off
							</button>
						{/if}
					</div>
				</section>

				{#if exclusiveOn && exclusiveFailureReason}
					<p class="remote-settings-note">
						<strong>Exclusive grab failed.</strong> {exclusiveFailureReason}
					</p>
				{/if}
			{/if}
		</div>
	</div>
{/if}

<style>
	.remote-settings-pill {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		min-height: 32px;
		padding: 0 12px;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.04em;
	}

	.remote-settings-pill svg {
		width: 14px;
		height: 14px;
	}

	.remote-settings-pill.active {
		background: color-mix(in oklab, var(--accent) 22%, var(--surface-1));
		color: var(--accent);
	}

	.remote-settings-pill:active {
		background: var(--surface-2);
	}

	.remote-settings-overlay {
		position: fixed;
		inset: 0;
		z-index: 60;
	}

	.remote-settings-scrim {
		position: absolute;
		inset: 0;
		background: rgba(0, 0, 0, 0.55);
		backdrop-filter: blur(6px);
		-webkit-backdrop-filter: blur(6px);
		animation: remote-settings-fade 200ms ease both;
	}

	.remote-settings-sheet {
		position: absolute;
		left: 0;
		right: 0;
		bottom: 0;
		display: grid;
		gap: 12px;
		padding: 6px 16px max(20px, env(safe-area-inset-bottom));
		background: var(--bg-base);
		border-top-left-radius: 22px;
		border-top-right-radius: 22px;
		box-shadow: 0 -24px 60px rgba(0, 0, 0, 0.45);
		transform: translate3d(0, var(--drag-y, 0px), 0);
		animation: remote-settings-slide 260ms cubic-bezier(0.22, 1.2, 0.36, 1) both;
		transition: transform 220ms cubic-bezier(0.22, 1.2, 0.36, 1);
	}

	.remote-settings-sheet.dragging {
		transition: none;
	}

	.remote-settings-handle {
		display: grid;
		place-items: center;
		padding: 8px 0 4px;
		touch-action: none;
		cursor: grab;
	}

	.remote-settings-grab {
		display: block;
		width: 42px;
		height: 4px;
		border-radius: 999px;
		background: var(--surface-2);
	}

	.remote-settings-head {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 10px;
		padding: 0 4px;
	}

	.remote-settings-head h2 {
		margin: 0;
		font-size: var(--font-size-md);
	}

	.remote-settings-head small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-settings-empty {
		margin: 16px 4px;
		color: var(--text-muted);
	}

	.remote-settings-rows {
		display: grid;
		gap: 8px;
	}

	.remote-settings-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 12px 14px;
		border-radius: 14px;
		background: var(--surface-1);
		color: var(--text-primary);
		text-align: left;
		min-height: 64px;
	}

	.remote-settings-row:active {
		background: var(--surface-2);
	}

	.remote-settings-row-copy {
		flex: 1;
		min-width: 0;
		display: grid;
		gap: 2px;
	}

	.remote-settings-row-copy strong {
		font-size: var(--font-size-sm);
	}

	.remote-settings-row-copy small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	.remote-settings-switch {
		flex-shrink: 0;
		width: 44px;
		height: 26px;
		border-radius: 999px;
		background: var(--surface-2);
		position: relative;
		transition: background 180ms ease;
	}

	.remote-settings-row.on .remote-settings-switch {
		background: var(--accent);
	}

	.remote-settings-knob {
		position: absolute;
		top: 3px;
		left: 3px;
		width: 20px;
		height: 20px;
		border-radius: 999px;
		background: var(--text-primary);
		box-shadow: 0 1px 3px rgba(0, 0, 0, 0.25);
		transition: transform 200ms cubic-bezier(0.22, 1.2, 0.36, 1);
	}

	.remote-settings-row.on .remote-settings-knob {
		transform: translateX(18px);
		background: var(--surface-0);
	}

	.remote-settings-note {
		margin: 0 4px;
		padding: 10px 12px;
		border-radius: 10px;
		background: color-mix(in oklab, var(--state-error) 12%, var(--surface-1));
		color: var(--text-primary);
		font-size: var(--font-size-xs);
	}

	.remote-settings-sleep {
		display: grid;
		gap: 8px;
		padding: 0 4px;
	}

	.remote-settings-sleep header {
		display: grid;
		gap: 1px;
	}

	.remote-settings-sleep header strong {
		font-size: var(--font-size-sm);
	}

	.remote-settings-sleep header small {
		color: var(--text-muted);
		font-size: var(--font-size-xs);
	}

	.remote-settings-sleep-row {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.remote-settings-sleep-chip {
		flex: 1 1 auto;
		min-width: 56px;
		min-height: 40px;
		padding: 0 12px;
		border-radius: 999px;
		background: var(--surface-1);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.remote-settings-sleep-chip:active {
		background: var(--surface-2);
	}

	.remote-settings-sleep-chip.active {
		background: var(--accent);
		color: var(--surface-0);
	}

	.remote-settings-sleep-cancel {
		flex: 0 0 auto;
		min-height: 40px;
		padding: 0 14px;
		border-radius: 999px;
		background: color-mix(in oklab, var(--state-error) 18%, var(--surface-1));
		color: var(--state-error);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.remote-settings-sleep-cancel:active {
		background: color-mix(in oklab, var(--state-error) 28%, var(--surface-1));
	}

	@keyframes remote-settings-slide {
		from {
			transform: translate3d(0, 100%, 0);
		}
		to {
			transform: translate3d(0, 0, 0);
		}
	}

	@keyframes remote-settings-fade {
		from {
			opacity: 0;
		}
		to {
			opacity: 1;
		}
	}
</style>
