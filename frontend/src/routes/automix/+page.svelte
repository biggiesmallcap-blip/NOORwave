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
		refreshPlaybackState
	} from '$lib/stores/player';
	import {
		api,
		type AudioDspFeatures,
		type AudioFeaturesStats,
		type DiscoveryStatus,
		type PlaybackRuntimeInfo
	} from '$lib/api/client';
	import { harmonicCompat } from '$lib/utils/camelot';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import { openContextMenu } from '$lib/stores/context_menu';
	import { buildTrackMenu, type MenuTrack } from '$lib/player/track_menu';
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
	const selectedShuffle = $derived(shuffleModes.find((m) => m.mode === $shuffleMode) ?? shuffleModes[0]);
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

	const compatibilityRows = $derived.by(() => {
		const rows: { level: string; keyLabel: string | null; bpmDelta: number | null }[] = [];
		const visible = queueUpcoming.slice(0, INDICATOR_WINDOW);
		for (let i = 0; i < visible.length; i += 1) {
			const previousTrackId = i === 0 ? $currentTrack?.id : visible[i - 1].track.id;
			const compat = harmonicCompat(featuresFor(previousTrackId), featuresFor(visible[i].track.id));
			if (compat) rows.push(compat);
		}
		return rows;
	});

	const goodMixCount = $derived(compatibilityRows.filter((row) => row.level === 'good').length);
	const clashMixCount = $derived(compatibilityRows.filter((row) => row.level === 'clash').length);

	function bpmDeltaLabel(delta: number | null): string | null {
		if (delta === null) return null;
		const sign = delta > 0 ? '+' : '';
		return `${sign}${delta.toFixed(1)} BPM`;
	}

	function formatFeatureSummary(features: AudioDspFeatures | null): string {
		if (!features) return 'DSP pending';
		const parts = [
			features.camelot_key ?? features.key_signature,
			features.bpm ? `${Math.round(features.bpm)} BPM` : null,
			features.energy != null ? `${Math.round(features.energy * 100)}% energy` : null
		].filter(Boolean);
		return parts.join(' / ') || 'DSP pending';
	}

	function percentLabel(value: number | null | undefined): string {
		if (value == null || !Number.isFinite(value)) return '--';
		return `${Math.round(value * 100)}%`;
	}

	function openTrackContextMenu(event: MouseEvent, track: MenuTrack, queueItemId?: number) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, buildTrackMenu(track, { queueItemId }), track.title);
	}
</script>

<svelte:head>
	<title>Automix | NOOR</title>
</svelte:head>

<div class="page-shell automix-page animate-in">
	<PageHeader
		eyebrow="Automix"
		title="Automix controls"
		subtitle="Crossfade, queue policy, and upcoming blend checks."
	>
		{#snippet actions()}
			<button class="btn btn-glass" onclick={loadControlData} disabled={saving}>Refresh data</button>
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

	<section class="automix-hero glass-panel">
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="now-card"
			oncontextmenu={(e) => {
				if ($currentTrack) openTrackContextMenu(e, $currentTrack);
			}}
		>
			<div class="now-art-shell">
				{#if $currentTrack?.artwork_url}
					<img src={$currentTrack.artwork_url} alt="" />
				{:else}
					<div class="now-art-empty">NOOR</div>
				{/if}
			</div>
			<div class="now-copy">
				<p class="eyebrow">Now playing</p>
				<h2>{$currentTrack?.title ?? 'No active track'}</h2>
				<p>{$currentTrack?.artist_name ?? 'Start playback to seed automix.'}</p>
				<div class="signal-strip">
					<span>{currentFeatureSummary}</span>
					<span>{$currentStreamDisplay?.audio_quality ?? 'Stream idle'}</span>
					<span>{runtime?.device_name ?? (runtimeAvailable ? 'Runtime ready' : 'Runtime offline')}</span>
				</div>
			</div>
		</div>

		<div class="mix-radar">
			<div class="radar-ring" style={`--mix:${selectedShuffle.meter}; --fade:${Math.min(1, draftCrossfade / 12000)}`}>
				<div class="radar-core">
					<strong>{crossfadeLabel(draftCrossfade)}</strong>
					<span>{selectedShuffle.label}</span>
				</div>
			</div>
			<div class="radar-stats">
				<div>
					<span>Good blends</span>
					<strong>{goodMixCount}</strong>
				</div>
				<div>
					<span>Clashes</span>
					<strong>{clashMixCount}</strong>
				</div>
				<div>
					<span>Pending</span>
					<strong>{pendingQueueCount}</strong>
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
				{#each queueUpcoming.slice(0, INDICATOR_WINDOW) as item, i (`${item.id}-${i}`)}
					{@const previousTrackId = i === 0 ? $currentTrack?.id : queueUpcoming[i - 1].track.id}
					{@const compat = harmonicCompat(featuresFor(previousTrackId), featuresFor(item.track.id))}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<div
						class="queue-row"
						oncontextmenu={(e) => openTrackContextMenu(e, item.track, item.id)}
					>
						<div class="queue-index">{String(i + 1).padStart(2, '0')}</div>
						{#if item.track.artwork_url}
							<img class="queue-art" src={item.track.artwork_url} alt="" />
						{:else}
							<div class="queue-art placeholder">♪</div>
						{/if}
						<div class="queue-meta">
							<strong>{item.track.title}</strong>
							<span>{item.track.artist_name ?? 'Unknown artist'}</span>
						</div>
						<div class="queue-dsp">
							<span>{formatFeatureSummary(featuresFor(item.track.id) ?? null)}</span>
							{#if compat}
								<b class="compat-pill compat-{compat.level}">
									{compat.keyLabel ?? compat.level}
									{#if compat.bpmDelta !== null}
										<small>{bpmDeltaLabel(compat.bpmDelta)}</small>
									{/if}
								</b>
							{:else}
								<b class="compat-pill">Analyzing</b>
							{/if}
						</div>
						{#if item.source === 'automix'}
							<StateBadge label="Automix" tone="active" compact={true} />
						{/if}
					</div>
				{/each}
				{#if queueUpcoming.length > INDICATOR_WINDOW}
					<p class="queue-overflow">+ {queueUpcoming.length - INDICATOR_WINDOW} more tracks</p>
				{/if}
			</div>
		{/if}
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
		gap: 7px;
	}

	.automix-page :global(.page-header .subtitle) {
		max-width: 62ch;
		font-size: var(--font-size-md);
		line-height: var(--line-height-loose);
	}

	.error-banner {
		padding: 12px 16px;
		color: var(--state-error);
		font-size: var(--font-size-sm);
	}

	.automix-hero {
		display: grid;
		grid-template-columns: minmax(0, 1.4fr) minmax(280px, 0.8fr);
		gap: 22px;
		padding: 22px;
		align-items: stretch;
	}

	.now-card {
		display: grid;
		grid-template-columns: 150px minmax(0, 1fr);
		gap: 18px;
		align-items: center;
		min-width: 0;
	}

	.now-art-shell,
	.now-art-empty {
		aspect-ratio: 1;
		border-radius: 12px;
		overflow: hidden;
		background: linear-gradient(135deg, rgba(124, 128, 255, 0.18), rgba(109, 184, 155, 0.08));
		border: 1px solid var(--border-subtle);
	}

	.now-art-shell img {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	.now-art-empty {
		display: grid;
		place-items: center;
		font-family: var(--font-mono);
		color: var(--text-tertiary);
	}

	.now-copy {
		min-width: 0;
		display: grid;
		gap: 10px;
	}

	.now-copy h2 {
		font-family: var(--font-body);
		font-size: var(--font-size-3xl);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0;
		overflow-wrap: anywhere;
	}

	.now-copy p:not(.eyebrow) {
		color: var(--text-secondary);
	}

	.signal-strip {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
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
		padding: 6px 9px;
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.mix-radar {
		display: grid;
		grid-template-columns: 180px 1fr;
		gap: 18px;
		align-items: center;
	}

	.radar-ring {
		--mix: 0.5;
		--fade: 0.4;
		aspect-ratio: 1;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background:
			conic-gradient(from 210deg, var(--accent) calc(var(--mix) * 280deg), rgba(255, 255, 255, 0.08) 0),
			radial-gradient(circle, rgba(109, 184, 155, calc(var(--fade) * 0.22)) 0 45%, transparent 46%);
		border: 1px solid var(--border-subtle);
	}

	.radar-core {
		width: 68%;
		aspect-ratio: 1;
		border-radius: 50%;
		display: grid;
		place-items: center;
		align-content: center;
		gap: 4px;
		background: color-mix(in srgb, var(--bg-base) 78%, transparent);
		border: 1px solid var(--border-subtle);
	}

	.radar-core strong {
		font-family: var(--font-body);
		font-size: var(--font-size-2xl);
		letter-spacing: 0;
	}

	.radar-core span,
	.radar-stats span,
	.data-card span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.radar-stats {
		display: grid;
		gap: 10px;
	}

	.radar-stats div {
		display: flex;
		justify-content: space-between;
		gap: 12px;
		padding-bottom: 10px;
		border-bottom: 1px solid var(--border-subtle);
	}

	.control-layout {
		display: grid;
		grid-template-columns: repeat(2, minmax(280px, 1fr));
		gap: var(--space-4);
	}

	.control-card,
	.queue-lab,
	.data-card {
		padding: 20px;
	}

	.control-card {
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.control-card.wide {
		grid-column: 1 / -1;
	}

	.card-heading {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 14px;
	}

	.card-heading h3 {
		font-size: var(--font-size-md);
	}

	.crossfade-steps,
	.policy-grid,
	.shuffle-options,
	.data-calls {
		display: grid;
		gap: 8px;
	}

	.crossfade-steps {
		grid-template-columns: repeat(auto-fit, minmax(70px, 1fr));
	}

	.step-btn {
		padding: 8px 12px;
		border-radius: 999px;
		color: var(--text-secondary);
		transition: all var(--motion-fast);
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
		gap: 12px;
	}

	.crossfade-slider {
		flex: 1;
	}

	.slider-value {
		min-width: 42px;
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
		gap: 6px;
		padding: 14px;
		border-radius: 12px;
		text-align: left;
		transition: all var(--motion-fast);
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
		gap: 7px;
		padding: 14px;
		border-radius: 12px;
		text-align: left;
	}

	.shuffle-meter {
		width: 100%;
		height: 5px;
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
		gap: 16px;
	}

	.queue-list {
		display: grid;
		gap: 6px;
	}

	.queue-row {
		display: grid;
		grid-template-columns: 34px 44px minmax(0, 1fr) minmax(180px, 0.7fr) auto;
		align-items: center;
		gap: 12px;
		padding: 10px;
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.026);
		border: 1px solid transparent;
	}

	.queue-row:hover {
		border-color: var(--border-subtle);
		background: rgba(255, 255, 255, 0.045);
	}

	.queue-index {
		font-family: var(--font-mono);
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
	}

	.queue-art {
		width: 44px;
		height: 44px;
		border-radius: 8px;
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
	.queue-dsp {
		min-width: 0;
		display: grid;
		gap: 3px;
	}

	.queue-meta strong,
	.queue-meta span,
	.queue-dsp span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.queue-meta span,
	.queue-dsp span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.compat-pill {
		width: fit-content;
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 4px 8px;
		border-radius: 999px;
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.compat-good {
		color: #86efac;
		border-color: rgba(74, 222, 128, 0.28);
		background: rgba(74, 222, 128, 0.1);
	}

	.compat-okay {
		color: #fcd34d;
		border-color: rgba(251, 191, 36, 0.28);
		background: rgba(251, 191, 36, 0.1);
	}

	.compat-clash {
		color: #fca5a5;
		border-color: rgba(248, 113, 113, 0.28);
		background: rgba(248, 113, 113, 0.1);
	}

	.queue-overflow {
		color: var(--text-secondary);
		text-align: center;
		padding: 8px;
	}

	.data-calls {
		grid-template-columns: repeat(3, minmax(0, 1fr));
	}

	.data-card {
		display: grid;
		gap: 10px;
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
		.automix-hero,
		.control-layout,
		.data-calls {
			grid-template-columns: 1fr;
		}

		.mix-radar {
			grid-template-columns: 160px 1fr;
		}

		.shuffle-options,
		.policy-grid {
			grid-template-columns: 1fr 1fr;
		}

		.queue-row {
			grid-template-columns: 28px 40px minmax(0, 1fr);
		}

		.queue-dsp {
			grid-column: 3 / -1;
		}
	}

	@media (max-width: 640px) {
		.now-card,
		.mix-radar,
		.shuffle-options,
		.policy-grid {
			grid-template-columns: 1fr;
		}

		.now-art-shell {
			width: min(180px, 70vw);
		}
	}
</style>
