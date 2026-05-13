<script lang="ts">
	import { onMount } from 'svelte';
	import {
		automixEnabled,
		automixDiscoverNew,
		automixUseLearning,
		automixAllowExternal,
		crossfadeMs,
		shuffleMode,
		currentTrack,
		currentTrackFeatures,
		playbackQueue,
		setPlayerAutomixEnabled,
		setPlayerCrossfadeMs,
		setPlayerShuffleMode,
		setPlayerDiscoverNew,
		setPlayerAutomixUseLearning,
		setPlayerAutomixAllowExternal,
		refreshPlaybackRuntime,
		currentStreamDisplay,
		refreshPlaybackState,
		moveQueueTrackNext,
		removeTrackFromQueue,
		startSongRadio
	} from '$lib/stores/player';
	import {
		api,
		type AudioDspFeatures,
		type AudioFeaturesStats,
		type DiscoveryStatus,
		type PlaybackRuntimeInfo
	} from '$lib/api/client';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, type MenuTrack } from '$lib/player/track_menu';
	import {
		automixHealth,
		buildForecastRows,
		countForecastRows,
		formatFeatureSummary,
		type AutomixForecastRow
	} from './automix_diagnostics';
	import type { Snapshot } from './$types';

	let saving = $state(false);
	let draftCrossfade = $state(0);
	let errorMsg = $state('');
	let runtime = $state<PlaybackRuntimeInfo | null>(null);
	let runtimeAvailable = $state(false);
	let audioStats = $state<AudioFeaturesStats | null>(null);
	let discoveryStatus = $state<DiscoveryStatus | null>(null);

	export const snapshot: Snapshot<{ scrollY: number }> = {
		capture: () => ({ scrollY: typeof window !== 'undefined' ? window.scrollY : 0 }),
		restore: (saved) => {
			requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }));
		}
	};

	onMount(() => {
		void refreshPlaybackState();
		void loadControlData();
		const unsub = crossfadeMs.subscribe((v) => {
			draftCrossfade = v;
		});
		return unsub;
	});

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

	function applyAutomix(enabled: boolean) {
		return runSaving(() => setPlayerAutomixEnabled(enabled, draftCrossfade));
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

	function saveCrossfade() {
		return runSaving(() => setPlayerCrossfadeMs(draftCrossfade));
	}

	const CROSSFADE_STEPS = [0, 1000, 2000, 3000, 5000, 8000, 10000, 12000];

	function crossfadeLabel(ms: number): string {
		if (ms === 0) return 'Off';
		if (ms < 1000) return `${ms}ms`;
		return `${ms / 1000}s`;
	}

	const shuffleModes = [
		{ mode: 'off' as const, label: 'Off', copy: 'Queue order stays untouched.', meter: 0.15 },
		{ mode: 'genre' as const, label: 'Genre mix', copy: 'Clustered flow with related detours.', meter: 0.62 },
		{ mode: 'weighted' as const, label: 'Smart shuffle', copy: 'Freshness, favorites, and skips all count.', meter: 0.8 },
		{ mode: 'true' as const, label: 'True random', copy: 'Flat random coverage for the full queue.', meter: 0.38 }
	];

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

	async function startCurrentSongRadio() {
		const trackId = $currentTrack?.id;
		if (!trackId) return;
		await runSaving(() => startSongRadio(trackId));
	}
</script>

<svelte:head>
	<title>Automix | NOOR</title>
</svelte:head>

<div class="page-shell automix-page animate-in">
	<PageHeader
		eyebrow="Automix"
		title="Automix diagnostics"
		subtitle="Seed health, queue blend forecast, and controls for fixing weak transitions."
	>
		{#snippet actions()}
			<button class="btn btn-glass" onclick={loadControlData} disabled={saving}>Refresh data</button>
			<button class="btn btn-glass" onclick={startCurrentSongRadio} disabled={saving || !$currentTrack}>
				Start radio
			</button>
			<button
				class="btn {$automixEnabled ? 'btn-primary' : 'btn-glass'}"
				onclick={() => applyAutomix(!$automixEnabled)}
				disabled={saving}
			>
				{$automixEnabled ? 'Automix on' : 'Automix off'}
			</button>
		{/snippet}
	</PageHeader>

	{#if errorMsg}
		<div class="error-banner glass-panel">{errorMsg}</div>
	{/if}

	<section class="diagnostic-top">
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="seed-panel glass-panel"
			oncontextmenu={(e) => {
				if ($currentTrack) openTrackContextMenu(e, $currentTrack);
			}}
		>
			<div class="seed-art-shell">
				{#if $currentTrack?.artwork_url}
					<img src={$currentTrack.artwork_url} alt="" />
				{:else}
					<div class="seed-art-empty">NOOR</div>
				{/if}
			</div>
			<div class="seed-copy">
				<p class="eyebrow">Current seed</p>
				<h2>{$currentTrack?.title ?? 'No active track'}</h2>
				<p>{$currentTrack?.artist_name ?? 'Start playback to seed Automix.'}</p>
				<div class="signal-strip">
					<span>{currentFeatureSummary}</span>
					<span>{$currentStreamDisplay?.audio_quality ?? 'Stream idle'}</span>
					<span>{runtime?.device_name ?? (runtimeAvailable ? 'Runtime ready' : 'Runtime offline')}</span>
				</div>
			</div>
		</div>

		<div class="health-panel glass-panel">
			<div class="card-heading">
				<div>
					<p class="eyebrow">Health</p>
					<h3>{health.label}</h3>
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

	<section class="queue-lab glass-panel">
		<div class="card-heading">
			<div>
				<p class="eyebrow">Forecast</p>
				<h3>Upcoming blends</h3>
			</div>
			<StateBadge label={`${queueUpcoming.slice(0, INDICATOR_WINDOW).length} visible`} tone="default" compact={true} />
		</div>

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
						{#if row.item.track.artwork_url}
							<img class="queue-art" src={row.item.track.artwork_url} alt="" />
						{:else}
							<div class="queue-art placeholder">♪</div>
						{/if}
						<div class="queue-meta">
							<strong>{row.item.track.title}</strong>
							<span>{row.item.track.artist_name ?? 'Unknown artist'}</span>
						</div>
						<div class="forecast-diagnostics">
							<span>{formatFeatureSummary(row.nextFeatures)}</span>
							{#if row.verdict !== 'unknown'}
								<b class="compat-pill compat-{row.verdict}">
									{row.keyLabel ?? row.verdict}
									{#if row.bpmDeltaLabel}
										<small>{row.bpmDeltaLabel}</small>
									{/if}
								</b>
							{:else}
								<b class="compat-pill">Analyzing</b>
							{/if}
							{#if row.energyDeltaLabel}
								<span>{row.energyDeltaLabel}</span>
							{/if}
							{#if row.missing.length > 0}
								<span>{row.missing.join(', ')}</span>
							{/if}
						</div>
						<StateBadge label={row.sourceLabel} tone={row.isExternalPending ? 'default' : 'active'} compact={true} />
						<div class="forecast-actions">
							<button class="forecast-action" onclick={(event) => void moveForecastRowNext(row, event)} disabled={saving || row.item.is_pending}>
								Next
							</button>
							<button class="forecast-action" onclick={(event) => void refreshForecastRow(row, event)} disabled={saving}>
								Refresh
							</button>
							<button class="forecast-action danger" onclick={(event) => void removeForecastRow(row, event)} disabled={saving}>
								Remove
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

	<section class="control-layout">
		<section class="glass-panel control-card">
			<div class="card-heading">
				<div>
					<p class="eyebrow">Fade</p>
					<h3>Crossfade</h3>
				</div>
				<StateBadge label={crossfadeLabel($crossfadeMs)} tone={$crossfadeMs > 0 ? 'active' : 'muted'} compact={true} />
			</div>

			<div class="crossfade-steps">
				{#each CROSSFADE_STEPS as step}
					<button
						class="step-btn {draftCrossfade === step ? 'active' : ''}"
						onclick={() => {
							draftCrossfade = step;
						}}
					>
						{crossfadeLabel(step)}
					</button>
				{/each}
			</div>

			<div class="slider-row">
				<input type="range" min="0" max="12000" step="500" bind:value={draftCrossfade} class="crossfade-slider" />
				<span class="slider-value">{crossfadeLabel(draftCrossfade)}</span>
			</div>

			<button class="btn btn-primary save-btn" onclick={saveCrossfade} disabled={saving || draftCrossfade === $crossfadeMs}>
				{saving ? 'Saving...' : 'Apply crossfade'}
			</button>
		</section>

		<section class="glass-panel control-card">
			<div class="card-heading">
				<div>
					<p class="eyebrow">Policy</p>
					<h3>Queue source</h3>
				</div>
			</div>
			<div class="policy-grid">
				<button class="policy-toggle {$automixDiscoverNew ? 'active' : ''}" onclick={toggleDiscoverNew} disabled={saving} aria-pressed={$automixDiscoverNew}>
					<strong>Include new</strong>
					<span>Search beyond local tracks.</span>
				</button>
				<button class="policy-toggle {$automixUseLearning ? 'active' : ''}" onclick={toggleUseLearning} disabled={saving} aria-pressed={$automixUseLearning}>
					<strong>Learned radio</strong>
					<span>Use listening signals.</span>
				</button>
				<button class="policy-toggle {$automixAllowExternal ? 'active' : ''}" onclick={toggleAllowExternal} disabled={saving} aria-pressed={$automixAllowExternal}>
					<strong>External picks</strong>
					<span>Allow stream candidates.</span>
				</button>
			</div>
		</section>

		<section class="glass-panel control-card wide">
			<div class="card-heading">
				<div>
					<p class="eyebrow">Shuffle</p>
					<h3>Mode</h3>
				</div>
			</div>
			<div class="shuffle-options">
				{#each shuffleModes as { mode, label, copy, meter }}
					<button
						class="shuffle-opt {$shuffleMode === mode ? 'active' : ''}"
						onclick={() => void setPlayerShuffleMode(mode)}
						style={`--meter:${meter}`}
					>
						<span class="shuffle-meter"></span>
						<strong>{label}</strong>
						<small>{copy}</small>
					</button>
				{/each}
			</div>
		</section>
	</section>

	<section class="data-calls">
		<div class="glass-panel data-card">
			<span>Embedding coverage</span>
			<strong>{percentLabel(discoveryStatus?.coverage_ratio)}</strong>
			<div class="mini-bar"><i style={`width:${percentLabel(discoveryStatus?.coverage_ratio)}`}></i></div>
		</div>
		<div class="glass-panel data-card">
			<span>Neighbor tracks</span>
			<strong>{discoveryStatus?.neighbor_tracks?.toLocaleString() ?? '0'}</strong>
			<div class="mini-bar"><i style={`width:${Math.min(100, (discoveryStatus?.neighbor_tracks ?? 0) / 100).toFixed(0)}%`}></i></div>
		</div>
		<div class="glass-panel data-card">
			<span>Queue DSP proxy</span>
			<strong>{analyzedCoverage == null ? '--' : percentLabel(analyzedCoverage)}</strong>
			<div class="mini-bar"><i style={`width:${percentLabel(analyzedCoverage ?? 0)}`}></i></div>
		</div>
	</section>
</div>

<style>
	.automix-page {
		gap: var(--space-5);
	}

	.automix-page :global(.page-header) {
		align-items: center;
		padding-top: 2px;
	}

	.automix-page :global(.page-header .intro) {
		max-width: 68ch;
		gap: var(--space-2);
	}

	.automix-page :global(.page-header .subtitle) {
		max-width: 62ch;
		font-size: var(--font-size-md);
		line-height: var(--line-height-loose);
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

	.seed-panel,
	.health-panel {
		padding: var(--space-4);
	}

	.seed-panel {
		display: grid;
		grid-template-columns: clamp(8rem, 12vw, 10rem) minmax(0, 1fr);
		gap: var(--space-4);
		align-items: center;
		min-width: 0;
	}

	.seed-art-shell,
	.seed-art-empty {
		aspect-ratio: 1;
		border-radius: var(--radius-md);
		overflow: hidden;
		background: var(--bg-raised);
		border: 1px solid var(--border-subtle);
	}

	.seed-art-shell img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.seed-art-empty {
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

	.seed-copy h2 {
		font-family: var(--font-body);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		letter-spacing: 0;
		overflow-wrap: anywhere;
	}

	.seed-copy p:not(.eyebrow) {
		color: var(--text-secondary);
	}

	.signal-strip {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.signal-strip span,
	.compat-pill,
	.step-btn,
	.policy-toggle,
	.shuffle-opt {
		border: 1px solid var(--border-subtle);
		background: rgba(255, 255, 255, 0.035);
	}

	.signal-strip span {
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
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

	.control-layout {
		display: grid;
		grid-template-columns: repeat(2, minmax(18rem, 1fr));
		gap: var(--space-4);
	}

	.control-card,
	.queue-lab,
	.data-card {
		padding: var(--space-4);
	}

	.control-card {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.control-card.wide {
		grid-column: 1 / -1;
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

	.crossfade-steps,
	.policy-grid,
	.shuffle-options,
	.data-calls {
		display: grid;
		gap: var(--space-2);
	}

	.crossfade-steps {
		grid-template-columns: repeat(auto-fit, minmax(4.5rem, 1fr));
	}

	.step-btn {
		padding: var(--space-2) var(--space-3);
		border-radius: 999px;
		color: var(--text-secondary);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.step-btn.active,
	.step-btn:hover {
		border-color: var(--accent-line);
		background: var(--accent-soft);
		color: var(--text-primary);
	}

	.slider-row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.crossfade-slider {
		flex: 1;
	}

	.slider-value {
		min-width: 3rem;
		text-align: right;
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
	}

	.save-btn {
		align-self: flex-start;
	}

	.policy-grid {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}

	.policy-toggle {
		display: grid;
		gap: var(--space-1);
		padding: var(--space-3);
		border-radius: var(--radius-md);
		text-align: left;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
	}

	.policy-toggle span,
	.shuffle-opt small {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.policy-toggle.active {
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}

	.shuffle-options {
		grid-template-columns: repeat(4, minmax(0, 1fr));
	}

	.shuffle-opt {
		--meter: 0.4;
		position: relative;
		overflow: hidden;
		display: grid;
		gap: var(--space-2);
		padding: var(--space-3);
		border-radius: var(--radius-md);
		text-align: left;
	}

	.shuffle-meter {
		width: 100%;
		height: 0.3125rem;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		overflow: hidden;
	}

	.shuffle-meter::after {
		content: '';
		display: block;
		width: calc(var(--meter) * 100%);
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, var(--accent), var(--state-success));
	}

	.shuffle-opt.active {
		border-color: var(--accent-line);
		background: var(--accent-soft);
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
		background: rgba(255, 255, 255, 0.026);
		border: 1px solid transparent;
	}

	.forecast-row:hover {
		border-color: var(--border-subtle);
		background: rgba(255, 255, 255, 0.045);
	}

	.forecast-row.verdict-clash {
		border-color: color-mix(in srgb, var(--state-error) 28%, transparent);
	}

	.queue-index {
		font-family: var(--font-mono);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.queue-art {
		width: clamp(2.25rem, 3vw, 2.75rem);
		height: clamp(2.25rem, 3vw, 2.75rem);
		border-radius: var(--radius-sm);
		object-fit: cover;
		background: rgba(255, 255, 255, 0.04);
	}

	.placeholder {
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
	.forecast-diagnostics span {
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
		padding: var(--space-1) var(--space-2);
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: 1;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast);
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
		.control-layout,
		.data-calls {
			grid-template-columns: 1fr;
		}

		.shuffle-options,
		.policy-grid {
			grid-template-columns: 1fr 1fr;
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
		.seed-panel,
		.shuffle-options,
		.policy-grid {
			grid-template-columns: 1fr;
		}

		.seed-art-shell {
			width: min(11rem, 70vw);
		}
	}
</style>
