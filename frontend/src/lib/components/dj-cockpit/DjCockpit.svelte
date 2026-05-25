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
			<span>{enabled ? 'DJ engine on' : 'Legacy playback path'}</span>
			<button type="button" aria-pressed={enabled} disabled={saving} onclick={() => void setEnabled(!enabled)}>
				{enabled ? 'Disable DJ' : 'Enable DJ'}
			</button>
		</div>
	</header>

	{#if !enabled}
		<p class="disabled-note">
			Playback is using the legacy path. DJ lookahead and transition planning are stopped.
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
		gap: var(--space-2);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	button {
		min-height: 2.75rem;
		padding: 0 var(--space-3);
		border: 1px solid var(--accent-line);
		border-radius: var(--radius-sm);
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
	}

	button:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.disabled-note,
	.rebuild-status {
		padding: var(--space-3);
		border: 1px solid color-mix(in srgb, var(--state-warning) 36%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
		color: var(--state-warning);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
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
