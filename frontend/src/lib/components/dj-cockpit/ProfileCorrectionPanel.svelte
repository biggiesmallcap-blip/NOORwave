<script lang="ts">
	import type {
		DjDeckStatus,
		DjProfileCorrectionRequest,
		DjTransitionSpeedBias,
	} from '$lib/api/client';

	let {
		current = undefined,
		next = undefined,
		transitionArmed = false,
		busy = false,
		onSave,
		onClear,
		onRebuild,
	}: {
		current?: DjDeckStatus;
		next?: DjDeckStatus;
		transitionArmed?: boolean;
		busy?: boolean;
		onSave: (correction: DjProfileCorrectionRequest) => void;
		onClear: (mediaRef: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>) => void;
		onRebuild: (mediaRef: Pick<DjProfileCorrectionRequest, 'media_ref_kind' | 'media_ref_id'>) => void;
	} = $props();

	let selectedRef = $state<'current' | 'next'>('next');
	let bpmMultiplier = $state(1);
	let downbeatOffset = $state(0);
	let phraseOffset = $state(0);
	let speedBias = $state<DjTransitionSpeedBias>('neutral');
	let safeOnly = $state(false);
	let manualDropText = $state('');
	let manualDropRefKey = $state('');
	let dropMarkerError = $state('');
	let notes = $state('');

	let selectedDeck = $derived(selectedRef === 'current' ? current : next);
	let effectiveResult = $derived(
		safeOnly
			? 'Forces SafeCrossfade'
			: speedBias === 'faster'
				? 'Requests faster handoff'
				: transitionArmed
					? 'Applies to next transition'
					: 'Updates the next eligible plan',
	);
	let detectedDropLabel = $derived(formatMarkers(selectedDeck?.drop_markers_ms ?? []));

	$effect(() => {
		const ref = mediaRef(selectedDeck);
		const key = ref ? `${ref.media_ref_kind}:${ref.media_ref_id}` : '';
		if (key !== manualDropRefKey) {
			manualDropRefKey = key;
			manualDropText = formatMarkerSeconds(selectedDeck?.manual_drop_markers_ms ?? []);
			dropMarkerError = '';
		}
	});

	function mediaRef(deck?: DjDeckStatus) {
		if (!deck) return null;
		return {
			media_ref_kind: deck.media_ref_kind,
			media_ref_id: deck.media_ref_id,
		};
	}

	function save() {
		const ref = mediaRef(selectedDeck);
		if (!ref) return;
		const manualDropMarkers = parseManualDropMarkers(manualDropText);
		if (!manualDropMarkers) return;
		onSave({
			...ref,
			bpm_multiplier: bpmMultiplier,
			downbeat_offset_beats: downbeatOffset,
			phrase_offset_bars: phraseOffset,
			safe_crossfade_only: safeOnly,
			transition_speed_bias: speedBias,
			manual_drop_markers_ms: manualDropMarkers,
			notes: notes.trim() || undefined,
		});
	}

	function parseManualDropMarkers(value: string) {
		const tokens = value
			.split(/[\s,]+/)
			.map((token) => token.trim())
			.filter(Boolean);
		if (tokens.length > 16) {
			dropMarkerError = 'Use 16 drop markers or fewer.';
			return null;
		}
		const markers: number[] = [];
		for (const token of tokens) {
			const seconds = Number(token);
			if (!Number.isFinite(seconds) || seconds < 0) {
				dropMarkerError = 'Drop markers must be positive seconds.';
				return null;
			}
			markers.push(Math.round(seconds * 1000));
		}
		dropMarkerError = '';
		return markers;
	}

	function formatMarkerSeconds(markersMs: number[]) {
		return markersMs.map((marker) => trimSeconds(marker / 1000)).join(', ');
	}

	function formatMarkers(markersMs: number[]) {
		return markersMs.length ? formatMarkerSeconds(markersMs) : 'none';
	}

	function trimSeconds(seconds: number) {
		return Number.isInteger(seconds) ? String(seconds) : seconds.toFixed(2).replace(/0+$/, '').replace(/\.$/, '');
	}
</script>

<section class="correction-panel" aria-labelledby="dj-corrections-heading">
	<header>
		<div>
			<p class="eyebrow">Corrections</p>
			<h2 id="dj-corrections-heading">Transition rules</h2>
		</div>
	</header>

	{#if transitionArmed}
		<p class="armed-note">Changes apply to the next transition.</p>
	{/if}
	<p class="rules-note">Rules change planning. They do not fire a transition now.</p>

	<div class="target-tabs" role="group" aria-label="Correction target">
		<button type="button" class:active={selectedRef === 'current'} onclick={() => { selectedRef = 'current'; }}>
			Current
		</button>
		<button type="button" class:active={selectedRef === 'next'} onclick={() => { selectedRef = 'next'; }}>
			Next
		</button>
	</div>

	{#if selectedDeck}
		<p class="target-label" title={selectedDeck.title}>{selectedDeck.title}</p>
		<p class="effective-result">{effectiveResult}</p>
		<div class="field-grid">
			<label>
				<span>BPM multiplier</span>
				<input type="number" min="0.5" max="1.5" step="0.01" bind:value={bpmMultiplier} />
			</label>
			<label>
				<span>Downbeat nudge</span>
				<input type="number" min="-16" max="16" step="1" bind:value={downbeatOffset} />
			</label>
			<label>
				<span>Phrase nudge</span>
				<input type="number" min="-16" max="16" step="1" bind:value={phraseOffset} />
			</label>
			<label>
				<span>Speed override</span>
				<select bind:value={speedBias}>
					<option value="slower">Slower</option>
					<option value="neutral">Neutral</option>
					<option value="faster">Faster</option>
				</select>
			</label>
		</div>

		<div class="drop-cues">
			<p>Detected drops: {detectedDropLabel}</p>
			<label>
				<span>Manual drops (seconds)</span>
				<input type="text" inputmode="decimal" bind:value={manualDropText} aria-invalid={dropMarkerError ? 'true' : 'false'} />
			</label>
			{#if dropMarkerError}
				<p class="field-error">{dropMarkerError}</p>
			{/if}
		</div>

		<label class="check-row">
			<input type="checkbox" bind:checked={safeOnly} />
			<span>Safe-crossfade only</span>
		</label>

		<label>
			<span>Notes</span>
			<textarea rows="3" bind:value={notes}></textarea>
		</label>

		<div class="actions">
			<button type="button" disabled={busy} onclick={save}>Save correction</button>
			<button type="button" disabled={busy} onclick={() => mediaRef(selectedDeck) && onClear(mediaRef(selectedDeck)!)}>Clear override</button>
			<button type="button" disabled={busy} onclick={() => mediaRef(selectedDeck) && onRebuild(mediaRef(selectedDeck)!)}>Rebuild profile</button>
		</div>
	{:else}
		<p class="empty">No deck selected for correction.</p>
	{/if}
</section>

<style>
	.correction-panel {
		display: grid;
		gap: var(--space-3);
	}

	.eyebrow,
	h2,
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

	h2 {
		font-size: var(--font-size-xl);
		line-height: var(--line-height-tight);
	}

	.armed-note,
	.rules-note {
		padding: var(--space-2) var(--space-3);
		border: 1px solid color-mix(in srgb, var(--state-warning) 34%, transparent);
		border-radius: var(--radius-sm);
		color: var(--state-warning);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.rules-note {
		border-color: var(--border-subtle);
		color: var(--text-secondary);
	}

	.target-tabs,
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	button {
		min-height: 2.75rem;
		padding: 0 var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 86%, transparent);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
	}

	button.active {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--accent-strong);
	}

	button:focus-visible,
	input:focus-visible,
	select:focus-visible,
	textarea:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.target-label {
		overflow: hidden;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.effective-result {
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--accent-line);
		border-radius: var(--radius-sm);
		background: var(--accent-soft);
		color: var(--accent-strong);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
	}

	.drop-cues {
		display: grid;
		gap: var(--space-2);
		padding: var(--space-2) var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 78%, transparent);
	}

	.drop-cues p,
	.field-error {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.field-error {
		color: var(--state-danger);
	}

	.field-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-3);
	}

	label {
		display: grid;
		gap: var(--space-1);
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	label span {
		font-weight: var(--font-weight-semibold);
	}

	input,
	select,
	textarea {
		width: 100%;
		min-height: 2.75rem;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 90%, transparent);
		color: var(--text-primary);
		font: inherit;
		padding: var(--space-2);
	}

	textarea {
		min-height: 5.5rem;
		resize: vertical;
	}

	.check-row {
		grid-template-columns: auto minmax(0, 1fr);
		align-items: center;
	}

	.check-row input {
		width: 1.25rem;
		min-height: 1.25rem;
	}

	.empty {
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	@media (max-width: 760px) {
		.field-grid {
			grid-template-columns: 1fr;
		}
	}
</style>
