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
		refreshPlaybackState
	} from '$lib/stores/player';
	import { api, type AudioDspFeatures } from '$lib/api/client';
	import { harmonicCompat } from '$lib/utils/camelot';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';

	let saving = $state(false);
	let draftCrossfade = $state(0);
	let errorMsg = $state('');

	onMount(() => {
		void refreshPlaybackState();
		const unsub = crossfadeMs.subscribe((v) => {
			draftCrossfade = v;
		});
		return unsub;
	});

	async function applyAutomix(enabled: boolean) {
		saving = true;
		errorMsg = '';
		try {
			await setPlayerAutomixEnabled(enabled, draftCrossfade);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			saving = false;
		}
	}

	async function toggleDiscoverNew() {
		saving = true;
		errorMsg = '';
		try {
			await setPlayerDiscoverNew(!$automixDiscoverNew);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			saving = false;
		}
	}

	async function toggleUseLearning() {
		saving = true;
		errorMsg = '';
		try {
			await setPlayerAutomixUseLearning(!$automixUseLearning);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			saving = false;
		}
	}

	async function toggleAllowExternal() {
		saving = true;
		errorMsg = '';
		try {
			await setPlayerAutomixAllowExternal(!$automixAllowExternal);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			saving = false;
		}
	}

	async function saveCrossfade() {
		saving = true;
		errorMsg = '';
		try {
			await setPlayerCrossfadeMs(draftCrossfade);
		} catch (e) {
			errorMsg = String(e);
		} finally {
			saving = false;
		}
	}

	const CROSSFADE_STEPS = [0, 1000, 2000, 3000, 5000, 8000, 10000, 12000];

	function crossfadeLabel(ms: number): string {
		if (ms === 0) return 'Off';
		if (ms < 1000) return `${ms}ms`;
		return `${ms / 1000}s`;
	}

	const shuffleModes = [
		{ mode: 'off' as const, label: 'Off', copy: 'Sequential — follows queue order.' },
		{ mode: 'genre' as const, label: 'Genre mix', copy: 'Interleaves genres from your session taste — familiar clusters with discovery woven in.' },
		{ mode: 'weighted' as const, label: 'Smart shuffle', copy: 'Boosts unplayed and favourite tracks, penalises recently played ones with time-decay.' },
		{ mode: 'true' as const, label: 'True shuffle', copy: 'Fisher-Yates full-coverage — statistically flat, no weighting.' },
	];

	const queueUpcoming = $derived($playbackQueue.filter((item) => {
		const currentId = $currentTrack?.id;
		if (!currentId) return true;
		const currentPos = $playbackQueue.find((q) => q.track.id === currentId)?.position ?? -1;
		return item.position > currentPos;
	}));

	// ─── DSP feature cache for queue harmonic indicators ──────────────────────
	// Queue items don't carry DSP features, so we lazily fetch them for the
	// visible top-N rows. Cache entries: undefined = not yet requested, null =
	// fetched but no features available, object = loaded. All reactive via a
	// bumped version counter to avoid mutating the Map in place losing Svelte's
	// reactivity.
	const featureCache = new Map<number, AudioDspFeatures | null>();
	const inflight = new Set<number>();
	let featureCacheVersion = $state(0);

	const INDICATOR_WINDOW = 20;

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
		// Reading the version counter ensures derived/$effect recomputes when cache fills.
		void featureCacheVersion;
		const current = $currentTrack;
		if (current && current.id === trackId) {
			return $currentTrackFeatures;
		}
		return featureCache.get(trackId);
	}

	// Eagerly kick off fetches for the window we're about to render so indicators
	// populate without flicker. Re-runs whenever the queue slice changes.
	$effect(() => {
		const visible = queueUpcoming.slice(0, INDICATOR_WINDOW);
		for (const item of visible) {
			requestFeatures(item.track.id);
		}
	});

	function bpmDeltaLabel(delta: number | null): string | null {
		if (delta === null) return null;
		const sign = delta > 0 ? '+' : '';
		return `${sign}${delta.toFixed(1)} BPM`;
	}
</script>

<svelte:head>
	<title>Automix | NOOR</title>
</svelte:head>

<div class="page-shell automix-page animate-in">
	<PageHeader
		eyebrow="Automix"
		title="DJ-style continuous playback with automatic transitions."
		subtitle="Enable automix to keep the queue topped up with genre-matched tracks and blend between them with a configurable crossfade."
	>
		{#snippet actions()}
			<button
				class="discover-toggle {$automixDiscoverNew ? 'active' : ''}"
				onclick={toggleDiscoverNew}
				disabled={saving}
				title={$automixDiscoverNew ? 'Include New: on — finding tracks outside your library' : 'Include New: off — library only'}
				aria-pressed={$automixDiscoverNew}
			>
				<svg width="15" height="15" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
					<path d="M7.5 1a6.5 6.5 0 1 0 0 13A6.5 6.5 0 0 0 7.5 1zm0 1a5.5 5.5 0 1 1 0 11A5.5 5.5 0 0 1 7.5 2zM7 4.5V7H4.5a.5.5 0 0 0 0 1H7v2.5a.5.5 0 0 0 1 0V8h2.5a.5.5 0 0 0 0-1H8V4.5a.5.5 0 0 0-1 0z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd"/>
				</svg>
				<span>Include New</span>
			</button>
			<button
				class="discover-toggle {$automixUseLearning ? 'active' : ''}"
				onclick={toggleUseLearning}
				disabled={saving}
				aria-pressed={$automixUseLearning}
			>
				<span>Use learned radio</span>
			</button>
			<button
				class="discover-toggle {$automixAllowExternal ? 'active' : ''}"
				onclick={toggleAllowExternal}
				disabled={saving}
				aria-pressed={$automixAllowExternal}
			>
				<span>Allow external</span>
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

	<section class="stat-grid">
		<MetricPair
			label="Status"
			value={$automixEnabled ? 'Active' : 'Inactive'}
			copy="Automix continuously extends the queue when enabled."
		/>
		<MetricPair
			label="Crossfade"
			value={crossfadeLabel($crossfadeMs)}
			copy="Duration of the overlap between tracks."
		/>
		<MetricPair
			label="Shuffle"
			value={shuffleModes.find((m) => m.mode === $shuffleMode)?.label ?? $shuffleMode}
			copy="Shuffle mode affects automix track selection."
		/>
	</section>

	{#if errorMsg}
		<div class="error-banner glass-panel">{errorMsg}</div>
	{/if}

	<div class="controls-grid">
		<section class="control-card glass-panel">
			<h3>Crossfade</h3>
			<p class="section-copy">Overlap duration between consecutive tracks. Both fades are applied — the outgoing track fades out while the incoming one fades in.</p>

			<div class="crossfade-steps">
				{#each CROSSFADE_STEPS as step}
					<button
						class="step-btn {draftCrossfade === step ? 'active' : ''}"
						onclick={() => { draftCrossfade = step; }}
					>
						{crossfadeLabel(step)}
					</button>
				{/each}
			</div>

			<div class="slider-row">
				<input
					type="range"
					min="0"
					max="12000"
					step="500"
					bind:value={draftCrossfade}
					class="crossfade-slider"
				/>
				<span class="slider-value">{crossfadeLabel(draftCrossfade)}</span>
			</div>

			<button class="btn btn-primary save-btn" onclick={saveCrossfade} disabled={saving || draftCrossfade === $crossfadeMs}>
				{saving ? 'Saving…' : 'Apply crossfade'}
			</button>
		</section>

		<section class="control-card glass-panel">
			<h3>Shuffle mode</h3>
			<p class="section-copy">Controls how automix selects the next track when extending the queue.</p>

			<div class="shuffle-options">
				{#each shuffleModes as { mode, label, copy }}
					<button
						class="shuffle-opt {$shuffleMode === mode ? 'active' : ''}"
						onclick={() => void setPlayerShuffleMode(mode)}
					>
						<strong>{label}</strong>
						<span>{copy}</span>
					</button>
				{/each}
			</div>
		</section>
	</div>

	<section class="queue-section">
		<h2>Upcoming in queue</h2>
		{#if queueUpcoming.length === 0}
			<EmptyState title="Queue is empty" copy={$automixEnabled ? 'Automix will fill it as tracks finish.' : 'Enable automix or add tracks manually.'} />
		{:else}
			<div class="queue-list">
				{#each queueUpcoming.slice(0, INDICATOR_WINDOW) as item, i (item.id)}
					{#if i > 0}
						{@const prevTrackId = queueUpcoming[i - 1].track.id}
						{@const compat = harmonicCompat(featuresFor(prevTrackId), featuresFor(item.track.id))}
						{#if compat}
							<div class="compat-indicator compat-{compat.level}" role="presentation">
								<span class="compat-dot" aria-hidden="true"></span>
								<span class="compat-labels">
									{#if compat.keyLabel}
										<span class="compat-key">{compat.keyLabel}</span>
									{/if}
									{#if compat.bpmDelta !== null}
										<span class="compat-bpm">{bpmDeltaLabel(compat.bpmDelta)}</span>
									{/if}
								</span>
							</div>
						{/if}
					{:else}
						{@const currentId = $currentTrack?.id}
						{@const compatFromCurrent =
							currentId != null
								? harmonicCompat(featuresFor(currentId), featuresFor(item.track.id))
								: null}
						{#if compatFromCurrent}
							<div class="compat-indicator compat-{compatFromCurrent.level}" role="presentation">
								<span class="compat-dot" aria-hidden="true"></span>
								<span class="compat-labels">
									{#if compatFromCurrent.keyLabel}
										<span class="compat-key">{compatFromCurrent.keyLabel}</span>
									{/if}
									{#if compatFromCurrent.bpmDelta !== null}
										<span class="compat-bpm">{bpmDeltaLabel(compatFromCurrent.bpmDelta)}</span>
									{/if}
								</span>
							</div>
						{/if}
					{/if}
					<div class="queue-row glass-panel">
						{#if item.track.artwork_url}
							<img class="queue-art" src={item.track.artwork_url} alt="" />
						{:else}
							<div class="queue-art placeholder">♫</div>
						{/if}
						<div class="queue-meta">
							<strong>{item.track.title}</strong>
							<span>{item.track.artist_name ?? 'Unknown artist'}</span>
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
</div>

<style>
	.controls-grid {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
		gap: var(--space-4);
		margin-bottom: var(--space-4);
	}

	.control-card {
		padding: 24px;
		display: flex;
		flex-direction: column;
		gap: 16px;
	}

	.control-card h3 {
		font-size: 1rem;
		font-weight: 600;
		margin: 0;
	}

	.section-copy {
		color: var(--text-secondary);
		font-size: 0.875rem;
		margin: 0;
		line-height: 1.5;
	}

	.crossfade-steps {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.step-btn {
		padding: 6px 14px;
		border-radius: 20px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-secondary);
		font-size: 0.8125rem;
		cursor: pointer;
		transition: all 0.15s;
	}

	.step-btn.active,
	.step-btn:hover {
		border-color: rgba(124, 128, 255, 0.5);
		background: rgba(124, 128, 255, 0.12);
		color: var(--text-primary);
	}

	.slider-row {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.crossfade-slider {
		flex: 1;
		accent-color: rgba(124, 128, 255, 0.8);
	}

	.slider-value {
		min-width: 36px;
		text-align: right;
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
		font-size: 0.875rem;
	}

	.save-btn {
		align-self: flex-start;
	}

	.shuffle-options {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.shuffle-opt {
		padding: 12px 16px;
		border-radius: var(--radius);
		border: 1px solid rgba(255, 255, 255, 0.06);
		background: rgba(255, 255, 255, 0.03);
		text-align: left;
		cursor: pointer;
		display: flex;
		flex-direction: column;
		gap: 4px;
		transition: all 0.15s;
	}

	.shuffle-opt strong {
		font-size: 0.875rem;
		color: var(--text-primary);
	}

	.shuffle-opt span {
		font-size: 0.8125rem;
		color: var(--text-secondary);
	}

	.shuffle-opt.active {
		border-color: rgba(124, 128, 255, 0.4);
		background: rgba(124, 128, 255, 0.1);
	}

	.shuffle-opt:hover {
		background: rgba(255, 255, 255, 0.06);
	}

	.queue-section {
		display: flex;
		flex-direction: column;
		gap: var(--space-3);
	}

	.queue-section h2 {
		font-size: 1.125rem;
		font-weight: 600;
	}

	.queue-list {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.queue-row {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 10px 16px;
	}

	.queue-art {
		width: 38px;
		height: 38px;
		border-radius: 8px;
		object-fit: cover;
		flex-shrink: 0;
		background: rgba(255, 255, 255, 0.04);
	}

	.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
		font-size: 1rem;
		border: 1px solid rgba(255, 255, 255, 0.06);
	}

	.queue-meta {
		flex: 1;
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.queue-meta strong {
		font-size: 0.875rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.queue-meta span {
		font-size: 0.8rem;
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.queue-overflow {
		color: var(--text-secondary);
		font-size: 0.875rem;
		text-align: center;
		padding: 8px;
	}

	/* Harmonic compatibility indicators between queue rows */
	.compat-indicator {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 2px 20px;
		margin: -4px 0;
		font-size: 0.72rem;
		line-height: 1;
		opacity: 0.85;
	}

	.compat-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		flex-shrink: 0;
		box-shadow: 0 0 6px currentColor;
	}

	.compat-labels {
		display: flex;
		gap: 10px;
		color: var(--text-secondary);
		font-variant-numeric: tabular-nums;
	}

	.compat-key {
		opacity: 0.95;
	}

	.compat-bpm {
		opacity: 0.7;
	}

	.compat-good .compat-dot {
		background: #4ade80;
		color: #4ade80;
	}

	.compat-good .compat-key {
		color: #86efac;
	}

	.compat-okay .compat-dot {
		background: #fbbf24;
		color: #fbbf24;
	}

	.compat-okay .compat-key {
		color: #fcd34d;
	}

	.compat-clash .compat-dot {
		background: #f87171;
		color: #f87171;
	}

	.compat-clash .compat-key {
		color: #fca5a5;
	}

	.error-banner {
		padding: 12px 16px;
		color: #ff6b6b;
		font-size: 0.875rem;
	}

	.discover-toggle {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		padding: 7px 14px;
		border-radius: 20px;
		border: 1px solid rgba(255, 255, 255, 0.1);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-secondary);
		font-size: 0.8125rem;
		cursor: pointer;
		transition: all 0.15s;
	}

	.discover-toggle:hover {
		border-color: rgba(124, 128, 255, 0.4);
		background: rgba(124, 128, 255, 0.08);
		color: var(--text-primary);
	}

	.discover-toggle.active {
		border-color: rgba(124, 128, 255, 0.55);
		background: rgba(124, 128, 255, 0.15);
		color: rgba(180, 182, 255, 1);
	}

	.discover-toggle:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}
</style>
