<script lang="ts">
	import type { DjDeckStatus, DjStatusResponse } from '$lib/api/client';

	let {
		current = undefined,
		next = undefined,
		status = null,
	}: {
		current?: DjDeckStatus;
		next?: DjDeckStatus;
		status: DjStatusResponse | null;
	} = $props();

	const fallbackPeaks = Array.from({ length: 32 }, (_, index) => {
		const phase = index / 31;
		return 0.18 + Math.sin(phase * Math.PI) * 0.42;
	});

	let plannedMs = $derived(status?.planned_start_ms ?? null);
	let actualMs = $derived(status?.actual_start_ms ?? null);
	let missed = $derived(status?.timing_status === 'missed');
	let mixWindowLabel = $derived(status?.renderer_template ?? status?.planned_template ?? status?.selected_program ?? 'No plan');

	function peaks(deck?: DjDeckStatus) {
		return deck?.waveform_peaks?.length ? deck.waveform_peaks : fallbackPeaks;
	}

	function waveformStatus(deck?: DjDeckStatus) {
		if (!deck) return 'missing';
		return deck.waveform_status;
	}

	function isSkeleton(deck?: DjDeckStatus) {
		return waveformStatus(deck) !== 'ready';
	}

	function markerPercent(deck: DjDeckStatus | undefined, value: number) {
		const markers = deck?.beat_markers_ms ?? [];
		const last = markers.at(-1) ?? 0;
		if (last <= 0) return 0;
		return Math.max(0, Math.min(100, (value / last) * 100));
	}

	function timingPercent(deck: DjDeckStatus | undefined, value: number | null) {
		if (value == null) return null;
		return markerPercent(deck, value);
	}

	function formatTiming(value: number | null) {
		return value == null ? 'pending' : `${Math.round(value / 1000)}s`;
	}
</script>

<section class="waveform-shell" aria-labelledby="dj-waveform-heading">
	<header>
		<div>
			<p class="eyebrow">Transition visual</p>
			<h3 id="dj-waveform-heading">Mix window</h3>
		</div>
		<span class="window-pill">{mixWindowLabel}</span>
	</header>

	<div class="waveform-stack" aria-label="Read-only transition waveform">
		{#each [{ label: 'Outgoing', deck: current }, { label: 'Incoming', deck: next }] as lane}
			<div class="waveform-lane" class:skeleton={isSkeleton(lane.deck)}>
				<div class="lane-label">
					<span>{lane.label}</span>
					<small>{waveformStatus(lane.deck)}</small>
				</div>
				<div class="waveform-track">
					<svg viewBox="0 0 100 36" preserveAspectRatio="none" role="img" aria-label={`${lane.label} waveform`}>
						{#each peaks(lane.deck) as peak, index}
							{@const width = 100 / peaks(lane.deck).length}
							{@const height = Math.max(2, peak * 30)}
							<rect
								x={index * width}
								y={(36 - height) / 2}
								width={Math.max(0.08, width * 0.72)}
								height={height}
								rx="0.1"
							/>
						{/each}
						{#each lane.deck?.downbeat_markers_ms?.slice(0, 48) ?? [] as marker}
							<line class="marker-svg downbeat" x1={markerPercent(lane.deck, marker)} x2={markerPercent(lane.deck, marker)} y1="0" y2="36" />
						{/each}
						{#each lane.deck?.phrase_markers_ms?.slice(0, 16) ?? [] as marker}
							<line class="marker-svg phrase" x1={markerPercent(lane.deck, marker)} x2={markerPercent(lane.deck, marker)} y1="0" y2="36" />
						{/each}
						{#each lane.deck?.drop_markers_ms?.slice(0, 16) ?? [] as marker}
							<line class="marker-svg drop" x1={markerPercent(lane.deck, marker)} x2={markerPercent(lane.deck, marker)} y1="0" y2="36" />
						{/each}
						{#each lane.deck?.manual_drop_markers_ms?.slice(0, 16) ?? [] as marker}
							<line class="marker-svg manual-drop" x1={markerPercent(lane.deck, marker)} x2={markerPercent(lane.deck, marker)} y1="0" y2="36" />
						{/each}
						{#if timingPercent(lane.deck, plannedMs) != null}
							<line class="fire-svg planned" x1={timingPercent(lane.deck, plannedMs) ?? 0} x2={timingPercent(lane.deck, plannedMs) ?? 0} y1="0" y2="36" />
						{/if}
						{#if actualMs != null && timingPercent(lane.deck, actualMs) != null}
							<line class="fire-svg actual" x1={timingPercent(lane.deck, actualMs) ?? 0} x2={timingPercent(lane.deck, actualMs) ?? 0} y1="0" y2="36" />
						{/if}
					</svg>
					<div class="mix-window" aria-hidden="true"></div>
				</div>
			</div>
		{/each}
	</div>

	<div class="timing-strip">
		<span>Planned fire: {formatTiming(plannedMs)}</span>
		<span>Actual fire: {missed ? 'missed' : formatTiming(actualMs)}</span>
		<span>Fire delta: {missed ? 'missed' : status?.timing_delta_ms != null ? `${status.timing_delta_ms} ms` : 'pending'}</span>
	</div>
</section>

<style>
	.waveform-shell {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 76%, transparent);
	}

	header,
	.lane-label,
	.timing-strip {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-2);
	}

	.eyebrow,
	h3 {
		margin: 0;
	}

	.eyebrow {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
	}

	h3 {
		font-size: var(--font-size-lg);
		line-height: var(--line-height-tight);
	}

	.window-pill,
	.lane-label small {
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: 1;
	}

	.waveform-stack {
		display: grid;
		gap: var(--space-2);
	}

	.waveform-lane {
		display: grid;
		gap: var(--space-1);
	}

	.lane-label span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
	}

	.waveform-track {
		position: relative;
		min-height: 3.25rem;
		overflow: hidden;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-raised) 82%, transparent);
	}

	svg {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
	}

	rect {
		fill: color-mix(in srgb, var(--accent) 74%, var(--text-secondary));
		opacity: 0.72;
	}

	.skeleton rect {
		fill: var(--border-muted);
		opacity: 0.45;
	}

	.mix-window {
		position: absolute;
		inset: 0 32%;
		border-inline: 1px solid var(--accent-line);
		background: color-mix(in srgb, var(--accent-soft) 48%, transparent);
	}

	.marker-svg {
		stroke-width: 0.18;
	}

	.marker-svg.downbeat {
		stroke: color-mix(in srgb, var(--text-secondary) 34%, transparent);
	}

	.marker-svg.phrase {
		stroke: color-mix(in srgb, var(--state-warning) 62%, transparent);
		stroke-width: 0.34;
	}

	.marker-svg.drop {
		stroke: color-mix(in srgb, var(--state-error) 72%, transparent);
		stroke-width: 0.38;
	}

	.marker-svg.manual-drop {
		stroke: var(--state-error);
		stroke-width: 0.58;
	}

	.fire-svg {
		stroke: var(--accent-strong);
		stroke-width: 0.42;
	}

	.fire-svg.actual {
		stroke: var(--state-success);
	}

	.timing-strip {
		flex-wrap: wrap;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	@media (max-width: 760px) {
		header,
		.timing-strip {
			align-items: start;
			flex-direction: column;
		}
	}
</style>
