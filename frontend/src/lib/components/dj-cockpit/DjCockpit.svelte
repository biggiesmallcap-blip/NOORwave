<script lang="ts">
	import { onMount } from 'svelte';
	import { api, type DjMixIntent, type DjProfileCorrectionRequest, type DjStatusResponse, type DjTransitionSpeedBias } from '$lib/api/client';
	import { showToast } from '$lib/stores/toast';
	import MixIntentControl from './MixIntentControl.svelte';
	import ProfileCorrectionPanel from './ProfileCorrectionPanel.svelte';
	import QueuePairPanel from './QueuePairPanel.svelte';
	import SafetyGuardrailPanel from './SafetyGuardrailPanel.svelte';
	import TransitionLane from './TransitionLane.svelte';

	let status = $state<DjStatusResponse | null>(null);
	let enabled = $state(false);
	let mixIntent = $state<DjMixIntent>('balanced');
	let speedBias = $state<DjTransitionSpeedBias>('neutral');
	let loading = $state(true);
	let saving = $state(false);
	let debugOpen = $state(false);
	let rebuildStatus = $state('');

	let transitionArmed = $derived(Boolean(status?.selected_program || status?.last_transition_event_id));

	async function refresh(showLoading = false) {
		if (showLoading) loading = true;
		try {
			const [enabledResponse, policyResponse, statusResponse] = await Promise.all([
				api.getDjEnabled(),
				api.getDjPolicy(),
				api.getDjStatus(),
			]);
			enabled = enabledResponse.enabled;
			mixIntent = policyResponse.mix_intent;
			speedBias = policyResponse.transition_speed_bias;
			status = statusResponse;
		} catch {
			showToast('Could not load DJ cockpit.', 'error');
		} finally {
			loading = false;
		}
	}

	onMount(() => {
		void refresh(true);
		const interval = window.setInterval(() => {
			void refresh();
		}, 2_000);
		return () => window.clearInterval(interval);
	});

	async function setEnabled(next: boolean) {
		saving = true;
		try {
			const response = await api.setDjEnabled(next);
			enabled = response.enabled;
			await refresh();
		} catch {
			showToast('Could not update DJ engine.', 'error');
		} finally {
			saving = false;
		}
	}

	async function setIntent(next: DjMixIntent) {
		mixIntent = next;
		try {
			await api.setDjMixIntent(next);
			await refresh();
		} catch {
			showToast('Could not update mix intent.', 'error');
		}
	}

	async function setSpeed(next: DjTransitionSpeedBias) {
		speedBias = next;
		try {
			await api.setDjPolicy({ transition_speed_bias: next });
			await refresh();
		} catch {
			showToast('Could not update transition speed.', 'error');
		}
	}

	async function saveCorrection(correction: DjProfileCorrectionRequest) {
		saving = true;
		try {
			await api.setDjProfileCorrection(correction);
			showToast('DJ correction saved.', 'success');
			await refresh();
		} catch {
			showToast('Could not save DJ correction.', 'error');
		} finally {
			saving = false;
		}
	}

	function clearCorrection(ref: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>) {
		void saveCorrection({
			...ref,
			bpm_multiplier: undefined,
			downbeat_offset_beats: undefined,
			phrase_offset_bars: undefined,
			safe_crossfade_only: false,
			transition_speed_bias: undefined,
			manual_drop_markers_ms: [],
			notes: undefined,
		});
	}

	async function rebuildProfile(ref: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>) {
		rebuildStatus = 'Requesting profile rebuild';
		try {
			const response = await api.rebuildDjProfile(ref);
			rebuildStatus = rebuildProfileStatusMessage(response.status, response.accepted);
			await refresh();
		} catch {
			rebuildStatus = 'Profile rebuild failed';
			showToast('Could not rebuild DJ profile.', 'error');
		}
	}

	function rebuildProfileStatusMessage(status: string, accepted: boolean) {
		if (accepted && status === 'already_running') return 'Profile rebuild already running';
		if (accepted) return 'Profile rebuild accepted';
		if (status === 'already_current') return 'Profile already current';
		if (status === 'dj_disabled') return 'DJ engine disabled';
		if (status === 'source_unavailable') return 'Profile source unavailable';
		if (status === 'retrying') return 'Profile retrying';
		if (status === 'decode_failed') return 'Profile decode failed';
		return 'Profile is not in the current pair';
	}

	async function recordFeedback(rating: 'good' | 'bad' | 'too_safe' | 'too_bold') {
		try {
			await api.recordDjFeedback({
				transition_event_id: status?.last_transition_event_id,
				rating,
			});
			showToast('DJ feedback recorded.', 'success');
			await refresh();
		} catch {
			showToast('Could not record DJ feedback.', 'error');
		}
	}

	function acceptSafeOnlySuggestion() {
		const suggestion = status?.safe_crossfade_suggestion;
		if (!suggestion) return;
		void saveCorrection({
			media_ref_kind: suggestion.media_ref_kind,
			media_ref_id: suggestion.media_ref_id,
			safe_crossfade_only: true,
			notes: 'Accepted safe-only suggestion',
		});
	}
</script>

<section class="dj-cockpit" aria-labelledby="dj-cockpit-heading">
	<header class="topbar">
		<div>
			<p class="eyebrow">DJ cockpit</p>
			<h1 id="dj-cockpit-heading">Transition control</h1>
		</div>
		<div class="engine-toggle">
			<span class="engine-label">DJ transitions</span>
			<button
				class="engine-switch"
				type="button"
				role="switch"
				aria-checked={enabled}
				aria-label={enabled ? 'Disable DJ transitions' : 'Enable DJ transitions'}
				disabled={saving}
				onclick={() => void setEnabled(!enabled)}
			>
				<span class="switch-track" aria-hidden="true">
					<span class="switch-thumb"></span>
				</span>
				<span class="switch-state">{enabled ? 'On' : 'Off'}</span>
			</button>
		</div>
	</header>

	{#if !enabled}
		<p class="disabled-note">
			Playback is using the legacy path. DJ lookahead and transition planning are stopped.
		</p>
	{:else}
		<p class="enabled-note">
			DJ is planning the next eligible current-plus-next pair.
		</p>
	{/if}

	<MixIntentControl
		intent={mixIntent}
		speed={speedBias}
		disabled={loading || saving}
		onIntentChange={(next) => void setIntent(next)}
		onSpeedChange={(next) => void setSpeed(next)}
	/>

	<div class="workspace">
		<div class="primary-column">
			<TransitionLane
				{status}
				debugOpen={debugOpen}
				onToggleDebug={() => { debugOpen = !debugOpen; }}
				onFeedback={(rating) => void recordFeedback(rating)}
			/>
			<QueuePairPanel current={status?.current} next={status?.next} />
		</div>

		<aside class="side-column">
			<ProfileCorrectionPanel
				current={status?.current}
				next={status?.next}
				transitionArmed={transitionArmed}
				busy={saving}
				onSave={(correction) => void saveCorrection(correction)}
				onClear={clearCorrection}
				onRebuild={(ref) => void rebuildProfile(ref)}
			/>
			{#if rebuildStatus}
				<p class="rebuild-status" role="status">{rebuildStatus}</p>
			{/if}
			<SafetyGuardrailPanel {status} onAcceptSafeOnly={acceptSafeOnlySuggestion} />
		</aside>
	</div>
</section>

<style>
	.dj-cockpit {
		width: min(100%, var(--content-width));
		margin: 0 auto;
		padding: var(--space-5);
		display: grid;
		gap: var(--space-4);
	}

	.topbar {
		display: flex;
		align-items: end;
		justify-content: space-between;
		gap: var(--space-4);
		padding: var(--space-4);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background:
			radial-gradient(
				130% 120% at 100% 0%,
				color-mix(in srgb, var(--accent-soft) 46%, transparent),
				transparent 60%
			),
			color-mix(in srgb, var(--bg-elevated) 90%, transparent);
	}

	.eyebrow,
	h1,
	p {
		margin: 0;
	}

	.eyebrow {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
		letter-spacing: 0;
	}

	h1 {
		margin-top: var(--space-1);
		font-size: var(--font-size-3xl);
		line-height: var(--line-height-tight);
	}

	.engine-toggle {
		display: flex;
		align-items: center;
		gap: var(--space-3);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.engine-label {
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
	}

	.engine-switch {
		min-height: 2.75rem;
		padding: 0 var(--space-2);
		border: 1px solid var(--border-muted);
		border-radius: 999px;
		background: color-mix(in srgb, var(--bg-raised) 86%, transparent);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.engine-switch[aria-checked='true'] {
		border-color: color-mix(in srgb, var(--state-success) 54%, var(--accent-line));
		background: color-mix(in srgb, var(--state-success) 14%, var(--accent-soft));
		color: var(--text-primary);
	}

	.engine-switch:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	.engine-switch:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.switch-track {
		width: 2.3rem;
		height: 1.25rem;
		padding: 0.15rem;
		border-radius: 999px;
		background: color-mix(in srgb, var(--text-tertiary) 32%, transparent);
		display: flex;
		align-items: center;
		transition: background var(--motion-fast);
	}

	.engine-switch[aria-checked='true'] .switch-track {
		background: color-mix(in srgb, var(--state-success) 68%, var(--accent-strong));
	}

	.switch-thumb {
		width: 0.95rem;
		height: 0.95rem;
		border-radius: 50%;
		background: var(--text-primary);
		box-shadow: 0 1px 4px rgba(0, 0, 0, 0.3);
		transform: translateX(0);
		transition: transform var(--motion-fast);
	}

	.engine-switch[aria-checked='true'] .switch-thumb {
		transform: translateX(1.05rem);
	}

	.switch-state {
		min-width: 2ch;
		text-align: left;
	}

	.disabled-note,
	.enabled-note,
	.rebuild-status {
		padding: var(--space-3);
		border: 1px solid color-mix(in srgb, var(--state-warning) 36%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
		color: var(--state-warning);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.enabled-note {
		border-color: color-mix(in srgb, var(--state-success) 32%, transparent);
		background: color-mix(in srgb, var(--state-success) 8%, transparent);
		color: var(--text-secondary);
	}

	.workspace {
		display: grid;
		grid-template-columns: minmax(0, 1.55fr) minmax(19rem, 0.85fr);
		gap: var(--space-4);
		align-items: start;
	}

	.primary-column,
	.side-column {
		display: grid;
		gap: var(--space-4);
		min-width: 0;
	}

	.side-column {
		padding: var(--space-4);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-elevated) 88%, transparent);
	}

	@media (max-width: 980px) {
		.workspace {
			grid-template-columns: 1fr;
		}
	}

	@media (max-width: 760px) {
		.dj-cockpit {
			padding: var(--space-3);
		}

		.topbar,
		.engine-toggle {
			display: grid;
			align-items: stretch;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		* {
			transition-duration: 1ms !important;
			animation-duration: 1ms !important;
		}
	}
</style>
