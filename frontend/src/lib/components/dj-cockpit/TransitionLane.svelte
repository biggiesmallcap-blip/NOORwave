<script lang="ts">
	import type { DjStatusResponse } from '$lib/api/client';

	let {
		status,
		onFeedback,
		debugOpen = false,
		onToggleDebug,
	}: {
		status: DjStatusResponse | null;
		onFeedback: (rating: 'good' | 'bad' | 'too_safe' | 'too_bold') => void;
		debugOpen?: boolean;
		onToggleDebug: () => void;
	} = $props();

	let fallback = $derived(status?.fallback_reason ?? null);
	let planningStatus = $derived(status?.planning_status ?? 'disabled');
	let transitionId = $derived(status?.last_transition_event_id ?? null);
	let transitionArmed = $derived(
		planningStatus === 'armed' ||
			(planningStatus !== 'missed' &&
				Boolean(transitionId || status?.renderer_template || status?.selected_program)),
	);
	let currentReady = $derived(status?.current?.profile_ready === true);
	let nextReady = $derived(status?.next?.profile_ready === true);
	let currentFailed = $derived(status?.current?.profile_status === 'decode_failed');
	let nextFailed = $derived(status?.next?.profile_status === 'decode_failed');
	let rendererLabel = $derived(
		status?.renderer_template ??
			(status?.renderer_mode === 'legacy_overlap'
				? 'DJ overlap armed'
				: status?.renderer_mode === 'dj_gain_program'
					? 'DJ gain program armed'
					: status?.renderer_mode === 'dj_full_program'
						? 'DJ full program armed'
						: 'Transition armed'),
	);
	let laneTitle = $derived(
		planningStatus === 'disabled'
			? 'Legacy path'
			: planningStatus === 'armed'
				? rendererLabel
				: planningStatus === 'pair_missing'
					? 'Pair not detected'
					: planningStatus === 'profile_failed'
						? 'Profile analysis failed'
					: planningStatus === 'waiting_for_profiles'
						? 'Analyzing profiles'
						: planningStatus === 'waiting_for_window'
							? 'Waiting for mix window'
							: planningStatus === 'missed'
								? 'Transition missed'
								: 'Ready to plan',
	);
	let laneCopy = $derived(
		planningStatus === 'disabled'
			? 'Playback is using the legacy path.'
			: planningStatus === 'armed'
				? status?.renderer_mode === 'legacy_overlap'
					? 'DJ planned this pair, but audio is using the overlap fallback.'
					: 'Transition armed for the current pair.'
				: planningStatus === 'pair_missing'
					? 'Waiting for current and next tracks.'
					: planningStatus === 'profile_failed' && currentFailed && nextFailed
						? 'Current and next profile decodes failed.'
						: planningStatus === 'profile_failed' && currentFailed
							? 'The current profile decode failed.'
							: planningStatus === 'profile_failed' && nextFailed
								? 'The next profile decode failed.'
								: planningStatus === 'waiting_for_profiles' && !currentReady && !nextReady
						? 'Building current and next DJ profiles.'
						: planningStatus === 'waiting_for_profiles' && !currentReady
							? 'Building the current DJ profile.'
							: planningStatus === 'waiting_for_profiles' && !nextReady
								? 'Building the next DJ profile.'
								: planningStatus === 'waiting_for_window'
									? 'Both DJ profiles are ready. Waiting for the mix window.'
									: planningStatus === 'missed'
										? 'The last armed transition missed its fire point.'
										: 'Both DJ profiles are ready. Waiting for a transition plan to arm.',
	);
	let showFallback = $derived(
		Boolean(
			fallback &&
				!['disabled', 'pair_missing', 'missing_current_profile', 'missing_next_profile'].includes(
					fallback,
				) &&
				!['current_profile_decode_failed', 'next_profile_decode_failed'].includes(fallback),
		),
	);
	let recentTimingEvents = $derived(status?.recent_timing_events ?? []);
	let timingSummary = $derived(status?.timing_history_summary ?? null);

	function formatTimingMs(value: number | null | undefined) {
		return typeof value === 'number' ? `${value} ms` : 'pending';
	}

	function formatTimingDelta(value: number | null | undefined) {
		if (typeof value !== 'number') {
			return 'pending';
		}
		return `${value > 0 ? '+' : ''}${value} ms`;
	}

	function formatActualTiming(event: DjStatusResponse['recent_timing_events'][number]) {
		return event.timing_status === 'missed' ? 'missed' : formatTimingMs(event.actual_start_ms);
	}

	function formatEventDelta(event: DjStatusResponse['recent_timing_events'][number]) {
		return event.timing_status === 'missed' ? 'missed' : formatTimingDelta(event.timing_delta_ms);
	}

	function formatTimingDirection(value: string | null | undefined) {
		switch (value) {
			case 'on_time':
				return 'on time';
			case 'early':
			case 'late':
			case 'missed':
			case 'pending':
				return value;
			default:
				return 'unknown';
		}
	}

	function formatTrackLabel(title: string | undefined, artist: string | undefined, fallback: string) {
		if (!title) {
			return fallback;
		}
		return artist ? `${title} - ${artist}` : title;
	}

	function formatTimingPair(event: DjStatusResponse['recent_timing_events'][number]) {
		const from = formatTrackLabel(event.from_title, event.from_artist, `Event ${event.event_id}`);
		const to = formatTrackLabel(event.to_title, event.to_artist, 'unknown');
		return `${from} -> ${to}`;
	}
</script>

<section class="transition-lane" aria-labelledby="dj-transition-heading">
	<header>
		<div>
			<p class="eyebrow">Transition lane</p>
			<h2 id="dj-transition-heading">{laneTitle}</h2>
		</div>
		<span class="event-id">{transitionId ? `Event ${transitionId}` : 'No event armed'}</span>
	</header>

	{#if showFallback}
		<p class="fallback" role="status">Fallback reason: {fallback}</p>
	{:else}
		<p class:pending={!transitionArmed} class:success={transitionArmed} role="status">{laneCopy}</p>
	{/if}

	<div class="lane-actions" aria-label="Transition feedback">
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('good')}>Good</button>
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('bad')}>Bad</button>
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('too_safe')}>Too safe</button>
		<button type="button" disabled={!transitionId} onclick={() => onFeedback('too_bold')}>Too bold</button>
	</div>

	<button class="debug-toggle" type="button" aria-expanded={debugOpen} onclick={onToggleDebug}>
		Debug planner facts
	</button>
	{#if debugOpen}
		<div class="debug-panel">
			<p class="debug-heading">Current event</p>
			<dl>
				<div>
					<dt>Planned template</dt>
					<dd>{status?.planned_template ?? status?.selected_program ?? 'none'}</dd>
				</div>
				<div>
					<dt>Renderer template</dt>
					<dd>{status?.renderer_template ?? 'none'}</dd>
				</div>
				<div>
					<dt>Renderer mode</dt>
					<dd>{status?.renderer_mode ?? 'none'}</dd>
				</div>
				<div>
					<dt>Downgrade</dt>
					<dd>{status?.downgrade_reason ?? 'none'}</dd>
				</div>
				<div>
					<dt>Planning reason</dt>
					<dd>{status?.planning_reason ?? 'none'}</dd>
				</div>
				<div>
					<dt>Sync target</dt>
					<dd>{status?.sync_target ?? 'none'}</dd>
				</div>
				<div>
					<dt>Current planned</dt>
					<dd>{formatTimingMs(status?.planned_start_ms)}</dd>
				</div>
				<div>
					<dt>Current fire</dt>
					<dd>{formatTimingMs(status?.actual_start_ms)}</dd>
				</div>
				<div>
					<dt>Current delta</dt>
					<dd>{formatTimingDelta(status?.timing_delta_ms)}</dd>
				</div>
				<div>
					<dt>Sync source</dt>
					<dd>{status?.timing_source ?? 'none'}</dd>
				</div>
				<div>
					<dt>Timing status</dt>
					<dd>{status?.timing_status ?? 'none'}</dd>
				</div>
				<div>
					<dt>Timing direction</dt>
					<dd>{formatTimingDirection(status?.timing_direction)}</dd>
				</div>
				<div>
					<dt>Timing quality</dt>
					<dd>{status?.timing_quality ?? 'unknown'}</dd>
				</div>
				<div>
					<dt>Readiness block</dt>
					<dd>{status?.fallback_reason ?? 'none'}</dd>
				</div>
				<div>
					<dt>Planning status</dt>
					<dd>{status?.planning_status ?? 'none'}</dd>
				</div>
				<div>
					<dt>Confidence floor</dt>
					<dd>{status?.profile_confidence_floor ?? 0}</dd>
				</div>
			</dl>
			<p class="debug-heading">Recent timing</p>
			{#if timingSummary && timingSummary.event_count > 0}
				<div class="timing-summary" aria-label="Recent DJ timing summary">
					<div>
						<span>Avg delta</span>
						<strong>{formatTimingDelta(timingSummary.average_delta_ms)}</strong>
					</div>
					<div>
						<span>Avg abs</span>
						<strong>{formatTimingMs(timingSummary.average_abs_delta_ms)}</strong>
					</div>
					<div>
						<span>Late</span>
						<strong>{timingSummary.late_count}</strong>
					</div>
					<div>
						<span>Missed</span>
						<strong>{timingSummary.missed_count}</strong>
					</div>
					<div>
						<span>Tight</span>
						<strong>{timingSummary.tight_count}</strong>
					</div>
					<div>
						<span>Bad</span>
						<strong>{timingSummary.bad_count}</strong>
					</div>
				</div>
			{/if}
			{#if recentTimingEvents.length > 0}
				<div class="timing-history-header" aria-hidden="true">
					<span>Pair</span>
					<span>Status</span>
					<span>Timing</span>
					<span>Quality</span>
					<span>Plan</span>
					<span>Source</span>
					<span>Planned</span>
					<span>Actual</span>
					<span>Delta</span>
				</div>
				<ul class="timing-history" aria-label="Recent DJ transition timing">
					{#each recentTimingEvents as event}
						<li>
							<span>{formatTimingPair(event)}</span>
							<span>{event.timing_status ?? 'none'}</span>
							<span>{formatTimingDirection(event.timing_direction)}</span>
							<span>{event.timing_quality}</span>
							<span>{event.planning_reason ?? 'none'}</span>
							<span>{event.timing_source ?? 'none'}</span>
							<span>{formatTimingMs(event.planned_start_ms)}</span>
							<span>{formatActualTiming(event)}</span>
							<span>{formatEventDelta(event)}</span>
						</li>
					{/each}
				</ul>
			{:else}
				<p class="empty-history">No completed DJ transitions yet.</p>
			{/if}
		</div>
	{/if}
</section>

<style>
	.transition-lane {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-4);
		border: 1px solid var(--border-muted);
		border-radius: var(--radius-md);
		background: color-mix(in srgb, var(--bg-raised) 90%, transparent);
	}

	header {
		display: flex;
		align-items: start;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.eyebrow,
	h2,
	p,
	dl {
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
		margin-top: var(--space-1);
		font-size: var(--font-size-2xl);
		line-height: var(--line-height-tight);
	}

	.event-id {
		flex-shrink: 0;
		padding: var(--space-1) var(--space-2);
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		line-height: 1;
	}

	.fallback,
	.success,
	.pending {
		padding: var(--space-3);
		border-radius: var(--radius-sm);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.fallback {
		border: 1px solid color-mix(in srgb, var(--state-warning) 36%, transparent);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
		color: var(--state-warning);
	}

	.success {
		border: 1px solid color-mix(in srgb, var(--state-success) 28%, transparent);
		background: color-mix(in srgb, var(--state-success) 9%, transparent);
		color: var(--text-secondary);
	}

	.pending {
		border: 1px solid var(--border-subtle);
		background: color-mix(in srgb, var(--bg-surface) 84%, transparent);
		color: var(--text-secondary);
	}

	.lane-actions {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: var(--space-2);
	}

	button {
		min-height: 2.75rem;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 86%, transparent);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		line-height: 1;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	button:hover,
	button:focus-visible {
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	button:focus-visible {
		outline: 2px solid var(--accent);
		outline-offset: 2px;
	}

	button:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.debug-toggle {
		justify-self: start;
		padding: 0 var(--space-3);
	}

	.debug-panel {
		display: grid;
		gap: var(--space-3);
		padding: var(--space-3);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 78%, transparent);
	}

	.debug-heading {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
	}

	dl {
		display: grid;
		gap: var(--space-2);
	}

	dl div {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
	}

	dt {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	dd {
		margin: 0;
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-align: right;
	}

	.timing-history,
	.timing-history-header,
	.timing-history li,
	.empty-history {
		margin: 0;
	}

	.timing-history-header {
		display: grid;
		grid-template-columns: minmax(10rem, 1fr) repeat(8, minmax(4.25rem, auto));
		gap: var(--space-2);
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
	}

	.timing-history-header span:not(:first-child) {
		text-align: right;
	}

	.timing-history {
		display: grid;
		gap: var(--space-2);
		padding: 0;
		list-style: none;
	}

	.timing-history li {
		display: grid;
		grid-template-columns: minmax(10rem, 1fr) repeat(8, minmax(4.25rem, auto));
		gap: var(--space-2);
		align-items: center;
		padding-top: var(--space-2);
		border-top: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.timing-history span:first-child,
	.timing-history span:nth-child(2) {
		color: var(--text-primary);
		font-weight: var(--font-weight-semibold);
	}

	.timing-history span:not(:first-child) {
		text-align: right;
	}

	.empty-history {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.timing-summary {
		display: grid;
		grid-template-columns: repeat(6, minmax(0, 1fr));
		gap: var(--space-2);
	}

	.timing-summary div {
		display: grid;
		gap: var(--space-1);
		padding: var(--space-2);
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--bg-surface) 66%, transparent);
	}

	.timing-summary span {
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
	}

	.timing-summary strong {
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-tight);
	}

	@media (max-width: 760px) {
		header,
		dl div {
			display: grid;
		}

		.timing-history li {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.timing-summary {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.timing-history-header {
			display: none;
		}

		.timing-history span:not(:first-child) {
			text-align: left;
		}

		.lane-actions {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}
	}
</style>
