<script lang="ts">
	// The Automix part of the Mix page: Up next (the forecast queue), This
	// session (queue source and shuffle, plus whatever the page passes as
	// `session`), and the one Diagnostics disclosure (Automix health and
	// library signals, plus the page's `diagnostics`). The Mix header owns the
	// Automix switch, Start radio and Refresh data (`refresh()` below).
	import { onMount, type Snippet } from 'svelte';
	import {
		automixEnabled,
		automixDiscoverNew,
		automixUseLearning,
		automixAllowExternal,
		shuffleMode,
		currentTrack,
		currentTrackFeatures,
		playbackQueue,
		setPlayerShuffleMode,
		setPlayerDiscoverNew,
		setPlayerAutomixUseLearning,
		setPlayerAutomixAllowExternal,
		refreshPlaybackRuntime,
		currentStreamDisplay,
		refreshPlaybackState,
		moveQueueTrackNext,
		removeTrackFromQueue
	} from '$lib/stores/player';
	import {
		api,
		type AudioDspFeatures,
		type AudioFeaturesStats,
		type DiscoveryStatus,
		type PlaybackRuntimeInfo
	} from '$lib/api/client';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import FilterChip from '$lib/components/ui/FilterChip.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';
	import QueueSourceLegend from '$lib/components/QueueSourceLegend.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, type MenuTrack } from '$lib/player/track_menu';
	import {
		automixHealth,
		buildForecastRows,
		countForecastRows,
		formatFeatureSummary,
		invalidateCacheForTrack,
		type AutomixForecastRow
	} from './automix_diagnostics';

	let { session, diagnostics }: { session?: Snippet; diagnostics?: Snippet } = $props();

	let saving = $state(false);
	let errorMsg = $state('');
	let runtime = $state<PlaybackRuntimeInfo | null>(null);
	let runtimeAvailable = $state(false);
	let audioStats = $state<AudioFeaturesStats | null>(null);
	let discoveryStatus = $state<DiscoveryStatus | null>(null);

	function handleDspUpdated(event: Event) {
		const trackId = (event as CustomEvent<{ trackId: number }>).detail?.trackId;
		if (typeof trackId !== 'number') return;
		invalidateCacheForTrack(featureCache, trackId);
		inflight.delete(trackId);
		requestFeatures(trackId);
		featureCacheVersion++;
		void api
			.getAudioFeaturesStats()
			.then((res) => {
				audioStats = res.stats ?? audioStats;
			})
			.catch(() => {});
	}

	onMount(() => {
		void refreshPlaybackState();
		void loadControlData();
		window.addEventListener('noor:dsp_updated', handleDspUpdated);
		return () => {
			window.removeEventListener('noor:dsp_updated', handleDspUpdated);
		};
	});

	/** Reload runtime, analysis and model stats (the Mix header's Refresh data). */
	export function refresh() {
		return loadControlData();
	}

	async function loadControlData() {
		try {
			const [runtimeResponse, statsResponse, discoveryResponse] = await Promise.all([
				api.getPlaybackRuntime().catch(() => null),
				api.getAudioFeaturesStats().catch(() => null),
				api.getDiscoveryStatus().catch(() => null)
			]);
			if (runtimeResponse) {
				runtimeAvailable = runtimeResponse.available;
				runtime = runtimeResponse.runtime;
				currentStreamDisplay.set(runtimeResponse.stream ?? null);
			}
			audioStats = statsResponse?.stats ?? null;
			discoveryStatus = discoveryResponse?.status ?? null;
			await refreshPlaybackRuntime();
		} catch {
			// Secondary cockpit data should never block playback controls.
		}
	}

	async function runSaving(action: () => Promise<void>) {
		saving = true;
		errorMsg = '';
		try {
			await action();
		} catch (e) {
			errorMsg = String(e);
		} finally {
			saving = false;
		}
	}

	let bpmOverrideSaving = $state(false);

	async function applyBpmMultiplier(factor: number) {
		const trackId = $currentTrack?.id;
		if (!trackId || !$currentTrackFeatures) return;
		bpmOverrideSaving = true;
		try {
			await api.setBpmMultiplier(trackId, factor);
			// The backend emits TrackAnalyzed → noor:dsp_updated, which the
			// player store already listens for and refetches features. Nothing
			// else to do here.
		} catch (e) {
			errorMsg = `BPM override failed: ${String(e)}`;
		} finally {
			bpmOverrideSaving = false;
		}
	}

	function toggleDiscoverNew() {
		return runSaving(() => setPlayerDiscoverNew(!$automixDiscoverNew));
	}

	function toggleUseLearning() {
		return runSaving(() => setPlayerAutomixUseLearning(!$automixUseLearning));
	}

	function toggleAllowExternal() {
		return runSaving(() => setPlayerAutomixAllowExternal(!$automixAllowExternal));
	}

	const shuffleModes = [
		{ mode: 'off' as const, label: 'Off', copy: 'Queue order stays untouched.' },
		{ mode: 'genre' as const, label: 'Genre mix', copy: 'Clustered flow with related detours.' },
		{ mode: 'weighted' as const, label: 'Smart shuffle', copy: 'Freshness, favorites, and skips all count.' },
		{ mode: 'true' as const, label: 'True random', copy: 'Flat random coverage for the full queue.' }
	];
	const SHUFFLE_OPTIONS = shuffleModes.map(({ mode, label }) => ({ value: mode, label }));
	const shuffleCopy = $derived(shuffleModes.find((option) => option.mode === $shuffleMode)?.copy ?? '');

	const queueUpcoming = $derived(
		$playbackQueue.filter((item) => {
			const currentId = $currentTrack?.id;
			if (!currentId) return true;
			const currentPos = $playbackQueue.find((q) => q.track.id === currentId)?.position ?? -1;
			return item.position > currentPos;
		})
	);

	const automixQueueCount = $derived(
		queueUpcoming.filter((item) => item.source === 'automix').length
	);
	const pendingQueueCount = $derived(
		queueUpcoming.filter((item) => item.is_pending === true).length
	);
	const analyzedCoverage = $derived(
		audioStats && $playbackQueue.length > 0
			? Math.min(1, audioStats.total_analyzed / Math.max(1, $playbackQueue.length + audioStats.total_analyzed))
			: null
	);
	const currentFeatureSummary = $derived(formatFeatureSummary($currentTrackFeatures));
	const discoveryCoverageLabel = $derived(
		discoveryStatus
			? `${Math.round(discoveryStatus.coverage_ratio * 100)}% embedded`
			: 'Not loaded'
	);

	const featureCache = new Map<number, AudioDspFeatures | null>();
	const inflight = new Set<number>();
	let featureCacheVersion = $state(0);
	const INDICATOR_WINDOW = 24;

	function requestFeatures(trackId: number): void {
		if (featureCache.has(trackId) || inflight.has(trackId)) return;
		inflight.add(trackId);
		void api
			.getTrackAudioFeatures(trackId)
			.then((res) => {
				featureCache.set(trackId, res.features ?? null);
				featureCacheVersion++;
			})
			.catch(() => {
				featureCache.set(trackId, null);
				featureCacheVersion++;
			})
			.finally(() => {
				inflight.delete(trackId);
			});
	}

	function featuresFor(trackId: number | null | undefined): AudioDspFeatures | null | undefined {
		if (trackId == null) return undefined;
		void featureCacheVersion;
		const current = $currentTrack;
		if (current && current.id === trackId) return $currentTrackFeatures;
		return featureCache.get(trackId);
	}

	$effect(() => {
		for (const item of queueUpcoming.slice(0, INDICATOR_WINDOW)) {
			requestFeatures(item.track.id);
		}
	});

	const forecastRows = $derived(
		buildForecastRows({
			currentTrack: $currentTrack,
			currentFeatures: $currentTrackFeatures,
			upcoming: queueUpcoming.slice(0, INDICATOR_WINDOW),
			featuresFor
		})
	);

	const forecastCounts = $derived(countForecastRows(forecastRows));
	const health = $derived(
		automixHealth({
			automixEnabled: $automixEnabled,
			currentTrack: $currentTrack,
			currentFeatures: $currentTrackFeatures,
			upcomingCount: queueUpcoming.length,
			pendingCount: pendingQueueCount,
			runtimeAvailable,
			runtime,
			discoveryStatus
		})
	);

	function percentLabel(value: number | null | undefined): string {
		if (value == null || !Number.isFinite(value)) return '--';
		return `${Math.round(value * 100)}%`;
	}

	function openTrackContextMenu(event: MouseEvent, track: MenuTrack, queueItemId?: number) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildTrackMenu(track, { queueItemId }), track.title);
	}

	async function moveForecastRowNext(row: AutomixForecastRow, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		if (row.item.is_pending) return;
		await runSaving(() => moveQueueTrackNext(row.item.id));
	}

	async function removeForecastRow(row: AutomixForecastRow, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		await runSaving(() => removeTrackFromQueue(row.item.id));
	}

	async function refreshForecastRow(row: AutomixForecastRow, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		featureCache.delete(row.item.track.id);
		requestFeatures(row.item.track.id);
		featureCacheVersion++;
	}

</script>

<div class="automix-panel">
	{#if errorMsg}
		<div class="error-banner" role="alert">{errorMsg}</div>
	{/if}


	<section class="queue-lab">
		<div class="card-heading">
			<div>
				<h2 class="t-section">Up next</h2>
			</div>
			<StateBadge label={`${queueUpcoming.slice(0, INDICATOR_WINDOW).length} visible`} tone="default" compact={true} />
		</div>

		<!-- The player panel used to carry this legend as permanent chrome. It
		     belongs here, next to the forecast that explains where queue rows
		     come from in the first place. -->
		<QueueSourceLegend />

		{#if queueUpcoming.length === 0}
			<EmptyState title="Queue is empty" copy={$automixEnabled ? 'Automix will fill it as tracks finish.' : 'Enable automix or add tracks manually.'} />
		{:else}
			<div class="queue-list">
				{#each forecastRows as row, i (`${row.item.id}-${i}`)}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="forecast-row verdict-{row.verdict}"
						oncontextmenu={(e) => openTrackContextMenu(e, row.item.track, row.item.id)}
					>
						<div class="queue-index">{String(i + 1).padStart(2, '0')}</div>
						<ArtworkImage
							className="queue-art"
							src={row.item.track.artwork_url}
							alt={row.item.track.title}
							size={320}
							fallbackText={row.item.track.title.slice(0, 2).toUpperCase()}
							decorative={true}
						/>
						<div class="queue-meta">
							<strong>{row.item.track.title}</strong>
							<span>{row.item.track.artist_name ?? 'Unknown artist'}</span>
						</div>
						<div class="forecast-diagnostics">
							{#if row.nextFeatures}<span>{formatFeatureSummary(row.nextFeatures)}</span>{/if}
							{#if row.selectionReasonLabel}
								<span class="selection-reason"><b>Why</b>{row.selectionReasonLabel}</span>
							{/if}
							{#if row.verdict !== 'unknown' && row.verdict !== 'pending'}
								<b class="compat-pill compat-{row.verdict}">
									{row.keyLabel ?? row.verdict}
									{#if row.bpmDeltaLabel}
										<small>{row.bpmDeltaLabel}</small>
									{/if}
								</b>
							{:else}
								<span class="t-meta" title={row.missing.length > 0 ? `Waiting for ${row.missing.join(', ')}` : undefined}>Analysing</span>
							{/if}
							{#if row.energyDeltaLabel}
								<span>{row.energyDeltaLabel}</span>
							{/if}
						</div>
						<StateBadge label={row.sourceLabel} tone={row.isExternalPending ? 'default' : 'active'} compact={true} />
						<div class="forecast-actions">
							<button
								class="forecast-action icon"
								aria-label="Move next"
								title="Move next"
								onclick={(event) => void moveForecastRowNext(row, event)}
								disabled={saving || row.item.is_pending}
							>
								↑
							</button>
							<button
								class="forecast-action icon"
								aria-label="Refresh DSP"
								title="Refresh DSP"
								onclick={(event) => void refreshForecastRow(row, event)}
								disabled={saving}
							>
								↻
							</button>
							<button
								class="forecast-action icon danger"
								aria-label="Remove from queue"
								title="Remove from queue"
								onclick={(event) => void removeForecastRow(row, event)}
								disabled={saving}
							>
								×
							</button>
						</div>
					</div>
				{/each}
				{#if queueUpcoming.length > INDICATOR_WINDOW}
					<p class="queue-overflow">+ {queueUpcoming.length - INDICATOR_WINDOW} more tracks</p>
				{/if}
			</div>
		{/if}
	</section>

	<section class="mix-session" aria-labelledby="mix-session-heading">
		<h2 id="mix-session-heading" class="t-section">This session</h2>
		<div class="session-row">
			<span class="row-label">Queue source</span>
			<div class="chips">
				<FilterChip pressed={$automixDiscoverNew} onclick={toggleDiscoverNew} disabled={saving} title="Search beyond local tracks.">Include new</FilterChip>
				<FilterChip pressed={$automixUseLearning} onclick={toggleUseLearning} disabled={saving} title="Use listening signals.">Learned radio</FilterChip>
				<FilterChip pressed={$automixAllowExternal} onclick={toggleAllowExternal} disabled={saving} title="Allow stream candidates.">External picks</FilterChip>
			</div>
		</div>
		<div class="session-row">
			<span class="row-label">Shuffle</span>
			<Segmented label="Shuffle" options={SHUFFLE_OPTIONS} value={$shuffleMode} onchange={(mode) => void setPlayerShuffleMode(mode)} />
			<span class="t-meta">{shuffleCopy}</span>
		</div>
		{@render session?.()}
		<p class="t-meta">Crossfade lives in <a href="/settings?category=playback">Settings, Playback</a>.</p>
	</section>

	<details class="automix-disclosure">
		<summary>Diagnostics</summary>
		<div class="diagnostics-body">
	<section class="diagnostic-top">
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="seed-panel"
			oncontextmenu={(e) => {
				if ($currentTrack) openTrackContextMenu(e, $currentTrack);
			}}
		>
			<div class="seed-art-shell">
				<ArtworkImage
					className="seed-art"
					src={$currentTrack?.artwork_url}
					alt={$currentTrack?.title ?? 'Current seed artwork'}
					size={640}
					fallbackText="NOOR"
					decorative={true}
				/>
			</div>
			<div class="seed-copy">
				<h3>Current seed</h3>
				<p class="seed-title">{$currentTrack?.title ?? 'No active track'}</p>
				<p>{$currentTrack?.artist_name ?? 'Start playback to seed Automix.'}</p>
				<div class="signal-strip">
					<span>{currentFeatureSummary}</span>
					{#if $currentTrackFeatures && $currentTrack}
						<button
							type="button"
							class="bpm-tweak"
							title="Halve the detected BPM (for doubled-tempo detections)"
							disabled={bpmOverrideSaving}
							onclick={() => applyBpmMultiplier(0.5)}
						>
							÷2
						</button>
						<button
							type="button"
							class="bpm-tweak"
							title="Double the detected BPM (for half-time detections)"
							disabled={bpmOverrideSaving}
							onclick={() => applyBpmMultiplier(2.0)}
						>
							×2
						</button>
					{/if}
					<span>{$currentStreamDisplay?.audio_quality ?? 'Stream idle'}</span>
					<span>{runtime?.device_name ?? (runtimeAvailable ? 'Runtime ready' : 'Runtime offline')}</span>
				</div>
			</div>
		</div>

		<div class="health-panel">
			<div class="card-heading">
				<div>
					<h3>Health</h3>
				</div>
				<StateBadge
					label={health.label}
					tone={health.status === 'ready' ? 'active' : health.status === 'blocked' ? 'error' : 'warning'}
					compact={true}
				/>
			</div>
			<div class="health-reasons">
				{#each health.reasons.slice(0, 4) as reason}
					<span>{reason}</span>
				{/each}
			</div>
			<div class="radar-stats">
				<div>
					<span>Good</span>
					<strong>{forecastCounts.good}</strong>
				</div>
				<div>
					<span>Pending</span>
					<strong>{forecastCounts.pending}</strong>
				</div>
				<div>
					<span>Clashes</span>
					<strong>{forecastCounts.clash}</strong>
				</div>
			</div>
		</div>
	</section>

	<section class="stat-grid">
		<MetricPair label="Upcoming" value={queueUpcoming.length} copy="After current track." />
		<MetricPair label="Automix" value={automixQueueCount} copy="Generated rows." />
		<MetricPair label="Model" value={discoveryCoverageLabel} copy={`${discoveryStatus?.playable_tracks?.toLocaleString() ?? 0} playable indexed.`} />
		<MetricPair label="DSP" value={audioStats?.total_analyzed?.toLocaleString() ?? '0'} copy={`BPM ${audioStats?.avg_bpm?.toFixed(1) ?? '--'} / key ${audioStats?.top_key ?? '--'}.`} />
	</section>
	<section class="data-calls">
		<div class="data-card">
			<span>Embedding coverage</span>
			<strong>{percentLabel(discoveryStatus?.coverage_ratio)}</strong>
			<div class="mini-bar"><i style={`width:${percentLabel(discoveryStatus?.coverage_ratio)}`}></i></div>
		</div>
		<div class="data-card">
			<span>Neighbor tracks</span>
			<strong>{discoveryStatus?.neighbor_tracks?.toLocaleString() ?? '0'}</strong>
			<div class="mini-bar"><i style={`width:${Math.min(100, (discoveryStatus?.neighbor_tracks ?? 0) / 100).toFixed(0)}%`}></i></div>
		</div>
		<div class="data-card">
			<span>Queue DSP proxy</span>
			<strong>{analyzedCoverage == null ? '--' : percentLabel(analyzedCoverage)}</strong>
			<div class="mini-bar"><i style={`width:${percentLabel(analyzedCoverage ?? 0)}`}></i></div>
		</div>
	</section>
			{@render diagnostics?.()}
		</div>
	</details>

</div>

<style>
	.automix-disclosure { border: 1px solid var(--border-subtle); border-radius: var(--radius-md); padding: var(--space-3); background: var(--bg-surface); }
	.automix-disclosure > summary { cursor: pointer; padding: var(--space-2); color: var(--text-secondary); font-size: var(--font-size-sm); font-weight: var(--font-weight-semibold); }
	.automix-disclosure > summary:focus-visible { outline: 2px solid var(--accent-strong); outline-offset: 2px; }
	.automix-disclosure[open] > summary { margin-bottom: var(--space-3); }
	.automix-panel {
		display: grid;
		gap: var(--space-5);
		min-width: 0;
	}

	.mix-session {
		display: grid;
		gap: var(--space-3);
	}

	.mix-session h2,
	.mix-session p {
		margin: 0;
	}

	.session-row {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: var(--space-2) var(--space-3);
	}

	.row-label {
		min-width: 7rem;
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
	}

	.chips {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.diagnostics-body {
		display: grid;
		gap: var(--space-4);
	}

	.error-banner {
		padding: var(--space-3) var(--space-4);
		color: var(--state-error);
		font-size: var(--font-size-sm);
	}

	.diagnostic-top {
		display: grid;
		grid-template-columns: minmax(0, 1.25fr) minmax(18rem, 0.75fr);
		gap: var(--space-4);
		align-items: stretch;
	}

	.seed-panel {
		display: grid;
		grid-template-columns: clamp(8rem, 12vw, 10rem) minmax(0, 1fr);
		gap: var(--space-4);
		align-items: center;
		min-width: 0;
	}

	.seed-art-shell {
		aspect-ratio: 1;
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--bg-raised);
		border: 1px solid var(--border-subtle);
	}

	.seed-art-shell :global(.seed-art) {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.seed-art-shell :global(.seed-art.fallback) {
		display: grid;
		place-items: center;
		font-family: var(--font-mono);
		color: var(--text-tertiary);
	}

	.seed-copy {
		min-width: 0;
		display: grid;
		gap: var(--space-2);
	}

	.seed-copy h3,
	.seed-copy p {
		margin: 0;
	}

	.seed-title {
		font-family: var(--font-display);
		font-size: var(--font-size-2xl);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		overflow-wrap: anywhere;
	}

	.seed-copy p:not(.seed-title) {
		color: var(--text-secondary);
	}

	.signal-strip {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.signal-strip span,
	.compat-pill {
		border: 1px solid var(--border-subtle);
		background: rgba(255, 255, 255, 0.035);
	}

	.signal-strip span {
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.bpm-tweak {
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: rgba(255, 255, 255, 0.035);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		cursor: pointer;
		transition: background-color 120ms ease, color 120ms ease;
	}

	.bpm-tweak:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.08);
		color: var(--text-primary);
	}

	.bpm-tweak:disabled {
		cursor: progress;
		opacity: 0.6;
	}

	.radar-stats span,
	.data-card span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.radar-stats {
		display: grid;
		gap: var(--space-2);
	}

	.radar-stats div {
		display: flex;
		justify-content: space-between;
		gap: var(--space-3);
		padding-bottom: var(--space-2);
		border-bottom: 1px solid var(--border-subtle);
	}

	.health-panel {
		display: grid;
		gap: var(--space-3);
	}

	.health-reasons {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.health-reasons span,
	.forecast-action {
		border: 1px solid var(--border-subtle);
		background: rgba(255, 255, 255, 0.035);
	}

	.health-reasons span {
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: 1;
	}

	.data-card {
		display: grid;
		gap: var(--space-1);
	}

	.card-heading {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-3);
	}

	.card-heading h3 {
		font-size: var(--font-size-md);
	}

	.data-calls {
		display: grid;
		gap: var(--space-2);
	}

	.queue-lab {
		display: grid;
		gap: var(--space-3);
	}

	.queue-list {
		display: grid;
		gap: var(--space-2);
	}

	.forecast-row {
		display: grid;
		grid-template-columns: 2.125rem clamp(2.25rem, 3vw, 2.75rem) minmax(0, 1fr) minmax(14rem, 0.85fr) auto auto;
		align-items: center;
		gap: var(--space-3);
		padding: var(--space-2);
		border-radius: var(--radius-sm);
		transition: background var(--motion-fast);
	}

	.forecast-row:hover {
		background: var(--bg-hover);
	}

	.forecast-row.verdict-clash {
		box-shadow: inset 2px 0 0 color-mix(in srgb, var(--state-error) 70%, transparent);
	}

	.queue-index {
		font-family: var(--font-mono);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.forecast-row :global(.queue-art) {
		width: clamp(2.25rem, 3vw, 2.75rem);
		height: clamp(2.25rem, 3vw, 2.75rem);
		border-radius: var(--radius-sm);
		object-fit: cover;
		display: block;
		background: rgba(255, 255, 255, 0.04);
	}

	.forecast-row :global(.queue-art.fallback) {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		border: 1px solid var(--border-subtle);
	}

	.queue-meta,
	.forecast-diagnostics {
		min-width: 0;
		display: grid;
		gap: var(--space-1);
	}

	.queue-meta strong,
	.queue-meta span,
	.forecast-diagnostics span,
	.selection-reason {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.queue-meta span,
	.forecast-diagnostics span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
	}

	.selection-reason {
		display: inline-flex;
		align-items: center;
		gap: var(--space-1);
		min-width: 0;
		color: var(--text-primary);
	}

	.selection-reason b {
		flex: 0 0 auto;
		color: var(--accent);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		line-height: 1;
		text-transform: uppercase;
	}

	.compat-pill {
		width: fit-content;
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.compat-good {
		color: var(--state-success);
		border-color: color-mix(in srgb, var(--state-success) 28%, transparent);
		background: color-mix(in srgb, var(--state-success) 10%, transparent);
	}

	.compat-okay {
		color: var(--state-warning);
		border-color: color-mix(in srgb, var(--state-warning) 28%, transparent);
		background: color-mix(in srgb, var(--state-warning) 10%, transparent);
	}

	.compat-clash {
		color: var(--state-error);
		border-color: color-mix(in srgb, var(--state-error) 28%, transparent);
		background: color-mix(in srgb, var(--state-error) 10%, transparent);
	}

	.compat-pending {
		color: var(--text-secondary);
		border-color: var(--border-subtle);
		background: rgba(255, 255, 255, 0.04);
	}

	.forecast-actions {
		display: flex;
		gap: var(--space-1);
	}

	.forecast-action {
		padding: var(--space-1);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: 1;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.forecast-action.icon {
		display: inline-grid;
		place-items: center;
		width: clamp(1.75rem, 2vw, 2rem);
		height: clamp(1.75rem, 2vw, 2rem);
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-bold);
	}

	.forecast-action:hover:not(:disabled) {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	.forecast-action.danger:hover:not(:disabled) {
		border-color: color-mix(in srgb, var(--state-error) 45%, transparent);
		color: var(--state-error);
	}

	.queue-overflow {
		color: var(--text-secondary);
		text-align: center;
		padding: var(--space-2);
	}

	.data-calls {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}

	.data-card {
		display: grid;
		gap: var(--space-2);
	}

	.data-card strong {
		font-family: var(--font-body);
		font-size: var(--font-size-2xl);
		letter-spacing: 0;
	}

	.mini-bar {
		height: 6px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		overflow: hidden;
	}

	.mini-bar i {
		display: block;
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, var(--accent), var(--state-success));
	}

	@media (max-width: 980px) {
		.diagnostic-top,
		.data-calls {
			grid-template-columns: 1fr;
		}

		.forecast-row {
			grid-template-columns: 1.75rem clamp(2.25rem, 3vw, 2.5rem) minmax(0, 1fr);
		}

		.forecast-diagnostics,
		.forecast-actions {
			grid-column: 3 / -1;
		}
	}

	@media (max-width: 640px) {
		.seed-panel {
			grid-template-columns: 1fr;
		}

		.seed-art-shell {
			width: min(11rem, 70vw);
		}
	}
</style>
