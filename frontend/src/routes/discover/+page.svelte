<script lang="ts">
	import { onMount } from 'svelte';
	import { get } from 'svelte/store';
	import {
		api,
		type DiscoveryExternalFeed,
		type DiscoveryMode,
		type DiscoveryPreviewResult,
		type DiscoveryRadioResult,
		type DiscoveryStatus,
		type Track
	} from '$lib/api/client';
	import { currentTrack, addTrackToQueue, playTrackNow } from '$lib/stores/player';
	import { training } from '$lib/stores/training';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';

	type DiscoverTab = 'radio' | 'explore';

	let activeTab = $state<DiscoverTab>('radio');
	let seedTrack = $state<Track | null>(null);
	let radioResults = $state<DiscoveryRadioResult[]>([]);
	let radioLoading = $state(false);
	let trainingButtonLoading = $state(false);
	let discoveryStatus = $state<DiscoveryStatus | null>(null);
	let statusInterval = $state<ReturnType<typeof setInterval> | null>(null);
	let trainingState = $state<import('$lib/stores/training').TrainingState>({
		isRunning: false,
		stage: '',
		progress: 0,
		message: '',
		lastCompletedAt: null
	});
	let isTraining = $derived(trainingState.isRunning || discoveryStatus?.latest_run?.status === 'running');
	let creativity = $state(0.25);
	let contextWindow = $state(5);
	let playedIds = $state<number[]>([]);
	let radioFetched = $state(false);
	let computingSimilarity = $state(false);
	let computePolling = $state(false);
	let computePollAttempt = $state(0);
	let computeLabel = $derived(computingSimilarity ? 'Sending request...' : 'Compute similarity now');
	let prompt = $state('glassy synthwave night drive');
	let mode = $state<DiscoveryMode>('mood');
	let previewLoading = $state(false);
	let preview = $state<DiscoveryExternalFeed | null>(null);
	let libraryPreviewResults = $state<DiscoveryPreviewResult[]>([]);
	let libraryAnchors = $state<string[]>([]);
	let error = $state<string | null>(null);

	function formatDuration(ms: number | null): string {
		if (!ms) return '—';
		const totalSec = Math.floor(ms / 1000);
		return `${Math.floor(totalSec / 60)}:${(totalSec % 60).toString().padStart(2, '0')}`;
	}

	function formatPercent(value: number): string {
		return `${Math.round(value * 100)}%`;
	}

	async function loadStatus() {
		try {
			const response = await api.getDiscoveryStatus();
			discoveryStatus = response.status;
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		}
	}

	async function startTraining(mode: 'full' | 'incremental') {
		trainingButtonLoading = true;
		error = null;
		try {
			const result = await api.startDiscoveryTraining(mode);
			if ((result as { status?: string }).status === 'already_running') {
				error = 'Training is already running — wait for it to finish before starting another.';
			}
			await loadStatus();
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		} finally {
			trainingButtonLoading = false;
		}
	}

	async function startRadio() {
		if (!seedTrack) return;
		radioLoading = true;
		error = null;
		try {
			const response = await api.getRadioTracks({
				seed_track_id: seedTrack.id,
				creativity,
				context_window: contextWindow,
				limit: 18,
				exclude_ids: playedIds
			});
			radioResults = response.tracks;
		radioFetched = true;
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
			radioResults = [];
			radioFetched = true;
		} finally {
			radioLoading = false;
		}
	}

	async function computeSimilarity() {
		if (!seedTrack) return;
		computingSimilarity = true;
		computePolling = false;
		error = null;
		try {
			await api.computeRadioSimilarity();
			computePolling = true;
			computePollAttempt = 0;
			// Poll radio every 4 seconds until results arrive (max 10 attempts)
			for (let i = 0; i < 10; i++) {
				computePollAttempt = i + 1;
				await new Promise((r) => setTimeout(r, 4000));
				const res = await api.getRadioTracks({
					seed_track_id: seedTrack!.id,
					creativity,
					context_window: contextWindow,
					limit: 18,
					exclude_ids: playedIds
				});
				if (res.tracks.length > 0) {
					radioResults = res.tracks;
					radioFetched = true;
					break;
				}
			}
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		} finally {
			computingSimilarity = false;
			computePolling = false;
		}
	}

	async function recordFeedback(candidateTrackId: number, action: string) {
		if (!seedTrack) return;
		try {
			await api.recordDiscoveryFeedback(seedTrack.id, candidateTrackId, action, activeTab, {
				creativity,
				contextWindow
			});
		} catch {
			// keep feedback best-effort so the UI never stalls on it
		}
	}

	async function playRadioTrack(track: DiscoveryRadioResult) {
		playedIds = [...playedIds, track.track_id];
		await recordFeedback(track.track_id, 'play');
		await playTrackNow(track.track_id);
		seedTrack = {
			id: track.track_id,
			title: track.title,
			artist_id: 0,
			artist_name: track.artist_name,
			album_id: null,
			album_title: track.album_title,
			duration_ms: track.duration_ms,
			best_quality: track.best_quality,
			is_favorite: false,
			play_count: 0,
			source: 'tidal',
			artwork_url: track.artwork_url,
			tidal_id: null
		} as Track;
		await startRadio();
	}

	async function queueRadioTrack(track: DiscoveryRadioResult) {
		await recordFeedback(track.track_id, 'queue');
		await addTrackToQueue(track.track_id);
	}

	async function rateRadioTrack(track: DiscoveryRadioResult, action: 'like' | 'dislike') {
		await recordFeedback(track.track_id, action);
	}

	async function loadExplore() {
		previewLoading = true;
		error = null;
		try {
			const [libraryResponse, externalResponse] = await Promise.all([
				api.previewDiscovery(prompt, mode, ['tidal'], 8),
				api.discoverNewMusic(prompt, mode, ['tidal'], 8)
			]);
			libraryPreviewResults = libraryResponse.preview.results;
			libraryAnchors = libraryResponse.preview.profile.top_artists;
			preview = externalResponse.feed;
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
			libraryPreviewResults = [];
			libraryAnchors = [];
			preview = null;
		} finally {
			previewLoading = false;
		}
	}

	onMount(() => {
		seedTrack = get(currentTrack);
		void loadStatus();

		// Subscribe to real-time training progress from WebSocket
		const unsubscribe = training.subscribe((state) => {
			trainingState.isRunning = state.isRunning;
			trainingState.stage = state.stage;
			trainingState.progress = state.progress;
			trainingState.message = state.message;
			trainingState.lastCompletedAt = state.lastCompletedAt;
		});

		// Poll every 2s while training is running
		statusInterval = setInterval(() => {
			void loadStatus();
		}, 2000);

		return () => {
			unsubscribe();
			if (statusInterval) clearInterval(statusInterval);
		};
	});
</script>

<div class="discover-page">
	<PageHeader
		eyebrow="Discover"
		title="NOOR is learning your next track."
		subtitle="Radio is the main discovery surface now: NOOR learns from sessions, playlists, what you finish, what you skip, and how you branch."
	/>

	{#if error}
		<div class="error-banner">{error}</div>
	{/if}

	<section class="glass engine-panel">
		<div>
			<h3>Discovery Engine</h3>
			{#if isTraining}
				<p class="training-status">
					<span class="spinner-dot"></span>
					{trainingState.message || discoveryStatus?.latest_run?.stage || 'Training…'}
				</p>
				<div class="progress-bar">
					<div
						class="progress-fill"
						style="width: {(trainingState.progress || discoveryStatus?.latest_run?.progress || 0) * 100}%"
					></div>
				</div>
			{:else if discoveryStatus?.active_model}
				<p>{discoveryStatus.active_model.model_key} is active with {formatPercent(discoveryStatus.coverage_ratio)} learned coverage.</p>
			{:else}
				<p>The learned engine has not been activated yet. Radio is using fallback similarity.</p>
			{/if}
		</div>
		<div class="engine-metrics">
			<span>{discoveryStatus?.embedded_tracks ?? 0} embedded</span>
			<span>{discoveryStatus?.neighbor_tracks ?? 0} neighbor roots</span>
			<span>{discoveryStatus?.clip_cache_tracks ?? 0} audio features</span>
		</div>
		<div class="engine-actions">
			<button class="btn btn-primary" onclick={() => startTraining('incremental')} disabled={isTraining}>
				{isTraining ? 'Training…' : 'Incremental refresh'}
			</button>
			<button class="btn btn-glass" onclick={() => startTraining('full')} disabled={isTraining}>
				Full retrain
			</button>
		</div>
	</section>

	<div class="tab-row">
		<button class:active={activeTab === 'radio'} onclick={() => (activeTab = 'radio')}>Radio</button>
		<button class:active={activeTab === 'explore'} onclick={() => (activeTab = 'explore')}>Explore</button>
	</div>

	{#if activeTab === 'radio'}
		<section class="glass radio-panel">
			<div class="seed-row">
				<div>
					<span class="label">Seed</span>
					<div class="seed-card">
						{#if seedTrack?.artwork_url}
							<img src={seedTrack.artwork_url} alt="" />
						{:else}
							<div class="art-placeholder">♫</div>
						{/if}
						<div>
							<strong>{seedTrack?.title ?? 'Nothing playing yet'}</strong>
							<span>{seedTrack?.artist_name ?? 'Play a track to seed radio'}</span>
						</div>
					</div>
				</div>
				<div class="controls">
					<label>
						<span>Learning strength</span>
						<input type="range" min="0" max="1" step="0.05" bind:value={creativity} />
					</label>
					<label>
						<span>Context memory</span>
						<input type="range" min="1" max="15" step="1" bind:value={contextWindow} />
					</label>
					<button class="btn btn-primary" onclick={startRadio} disabled={!seedTrack || radioLoading}>
						{radioLoading ? 'Listening…' : 'Start radio'}
					</button>
				</div>
			</div>
		</section>

		{#if radioResults.length > 0}
			<div class="results-grid">
				{#each radioResults as track (track.track_id)}
					<article class="glass result-card">
						{#if track.artwork_url}
							<img class="cover" src={track.artwork_url} alt="" />
						{:else}
							<div class="cover placeholder">♫</div>
						{/if}
						<div class="meta">
							<h3>{track.title}</h3>
							<p>{track.artist_name ?? 'Unknown artist'}</p>
							<p>{track.album_title ?? 'No album'} · {formatDuration(track.duration_ms)}</p>
							<div class="pill-row">
								{#each track.reason_tags as tag}
									<span>{tag}</span>
								{/each}
							</div>
						</div>
						<div class="actions">
							<span class="score">{formatPercent(track.similarity_score)}</span>
							<button class="btn btn-primary btn-sm" onclick={() => playRadioTrack(track)}>Play</button>
							<button class="btn btn-glass btn-sm" onclick={() => queueRadioTrack(track)}>Queue</button>
							<button class="btn btn-glass btn-sm" onclick={() => rateRadioTrack(track, 'like')}>More like this</button>
							<button class="btn btn-glass btn-sm" onclick={() => rateRadioTrack(track, 'dislike')}>Less like this</button>
							<button class="btn btn-glass btn-sm" onclick={() => recordFeedback(track.track_id, 'save')}>Save</button>
						</div>
					</article>
				{/each}
			</div>
		{:else}
			<div class="empty-state">
				{#if computePolling}
					<div class="compute-status">
						<div class="spinner"></div>
						<p>Building similarity graph — check {computePollAttempt}/10</p>
						<span class="poll-sub">Polling every 4s until tracks appear</span>
					</div>
				{:else if radioFetched}
					<p>No similar tracks found. Run similarity computation to build the graph.</p>
					<button class="btn btn-glass" onclick={computeSimilarity} disabled={computingSimilarity}>{computeLabel}</button>
				{:else}
					<p>Start radio from the current track to see NOOR's learned neighborhood.</p>
				{/if}
			</div>
		{/if}
	{:else}
		<section class="glass explore-panel">
			<div class="explore-inputs">
				<textarea bind:value={prompt} rows="3" placeholder="Try: dubbed-out night drive, smoky spiritual jazz, ecstatic deep house at sunrise"></textarea>
				<div class="explore-actions">
					<select bind:value={mode}>
						<option value="mood">Mood</option>
						<option value="reference">Reference</option>
						<option value="dj">DJ</option>
						<option value="word-cloud">Word cloud</option>
					</select>
					<button class="btn btn-primary" onclick={loadExplore} disabled={previewLoading}>
						{previewLoading ? 'Exploring…' : 'Explore'}
					</button>
				</div>
			</div>
		</section>

		{#if libraryPreviewResults.length > 0 || (preview && preview.results.length > 0)}
			{#if libraryPreviewResults.length > 0}
				<div class="explore-section">
					<div class="explore-section-header">
						<span class="explore-section-label">From your library</span>
						{#if libraryAnchors.length > 0}
							<span class="explore-anchors">anchored by {libraryAnchors.join(', ')}</span>
						{/if}
					</div>
					<div class="results-grid">
						{#each libraryPreviewResults as result (result.track_id)}
							<article class="glass result-card">
								{#if result.artwork_url}
									<img class="cover" src={result.artwork_url} alt="" />
								{:else}
									<div class="cover placeholder">♫</div>
								{/if}
								<div class="meta">
									<h3>{result.title}</h3>
									<p>{result.artist_name ?? 'Unknown artist'}</p>
									<p>{result.album_title ?? 'No album'} · {formatDuration(result.duration_ms)}</p>
									<div class="pill-row">
										{#each result.tags.slice(0, 4) as tag}
											<span>{tag}</span>
										{/each}
										<span class="score-pill">{result.score}%</span>
									</div>
								</div>
							</article>
						{/each}
					</div>
				</div>
			{/if}

			{#if preview && preview.results.length > 0}
				<div class="explore-section">
					<div class="explore-section-header">
						<span class="explore-section-label">New from TIDAL</span>
					</div>
					<div class="results-grid">
						{#each preview.results as result (result.provider + result.provider_track_id)}
							<article class="glass result-card">
								{#if result.artwork_url}
									<img class="cover" src={result.artwork_url} alt="" />
								{:else}
									<div class="cover placeholder">♫</div>
								{/if}
								<div class="meta">
									<h3>{result.title}</h3>
									<p>{result.artist_name ?? 'Unknown artist'}</p>
									<p>{result.album_title ?? 'No album'} · {formatDuration(result.duration_ms)}</p>
									<div class="pill-row">
										{#each [...result.tags, ...(result.embedding_score ? [`embedding ${(result.embedding_score * 100).toFixed(0)}%`] : [])].slice(0, 5) as tag}
											<span>{tag}</span>
										{/each}
									</div>
								</div>
							</article>
						{/each}
					</div>
				</div>
			{/if}
		{:else}
			<div class="empty-state">
				<p>Prompt explore is now secondary. Use it when you want to steer the learned engine outward.</p>
			</div>
		{/if}
	{/if}
</div>

<style>
	.discover-page {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}

	.error-banner,
	.engine-panel,
	.radio-panel,
	.explore-panel {
		padding: 18px;
		border-radius: var(--radius);
	}

	.error-banner {
		background: rgba(255, 77, 109, 0.12);
		border: 1px solid rgba(255, 77, 109, 0.25);
		color: #ff6b6b;
	}

	.engine-panel,
	.radio-panel,
	.explore-panel,
	.result-card {
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.08);
		backdrop-filter: blur(16px);
	}

	.engine-panel,
	.seed-row,
	.engine-actions,
	.engine-metrics,
	.explore-actions,
	.actions,
	.pill-row,
	.tab-row {
		display: flex;
		gap: 12px;
		flex-wrap: wrap;
	}

	.engine-panel,
	.radio-panel,
	.explore-panel {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.training-status {
		display: flex;
		align-items: center;
		gap: 8px;
		font-size: 0.88rem;
		color: var(--accent, #7c80ff);
		font-weight: 600;
	}

	.spinner-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--accent, #7c80ff);
		animation: pulse 1.4s ease-in-out infinite;
	}

	@keyframes pulse {
		0%, 80%, 100% { opacity: 0.3; transform: scale(0.8); }
		40% { opacity: 1; transform: scale(1.1); }
	}

	.progress-bar {
		width: 100%;
		height: 4px;
		border-radius: 2px;
		background: rgba(255, 255, 255, 0.06);
		overflow: hidden;
	}

	.progress-fill {
		height: 100%;
		background: linear-gradient(90deg, var(--accent, #7c80ff), #a78bfa);
		border-radius: 2px;
		transition: width 1s ease-out;
	}

	.tab-row button {
		padding: 8px 14px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.1);
		color: var(--text-secondary);
	}

	.tab-row button.active {
		background: rgba(124, 128, 255, 0.14);
		border-color: rgba(124, 128, 255, 0.28);
		color: var(--text-primary);
	}

	.explore-section {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.explore-section-header {
		display: flex;
		align-items: baseline;
		gap: 10px;
	}

	.explore-section-label {
		font-size: 0.75rem;
		font-weight: 600;
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-secondary);
	}

	.explore-anchors {
		font-size: 0.75rem;
		color: var(--text-secondary);
		opacity: 0.6;
	}

	.score-pill {
		font-size: 0.7rem;
		opacity: 0.7;
	}

	.label {
		display: block;
		margin-bottom: 6px;
		font-size: 0.8rem;
		color: var(--text-secondary);
	}

	.seed-card {
		display: flex;
		gap: 12px;
		align-items: center;
	}

	.seed-card img,
	.cover {
		width: 56px;
		height: 56px;
		border-radius: 10px;
		object-fit: cover;
		background: rgba(255, 255, 255, 0.04);
	}

	.cover {
		width: 100%;
		height: auto;
		aspect-ratio: 1;
	}

	.art-placeholder,
	.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-secondary);
	}

	.controls {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
		gap: 12px;
		flex: 1;
	}

	.controls label,
	.explore-inputs {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	textarea,
	select,
	input[type='range'] {
		width: 100%;
	}

	.results-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(250px, 1fr));
		gap: var(--space-3);
	}

	.result-card {
		padding: 14px;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.meta {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.meta h3,
	.meta p {
		margin: 0;
	}

	.meta p {
		color: var(--text-secondary);
		font-size: 0.84rem;
	}

	.pill-row span {
		padding: 4px 8px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.05);
		border: 1px solid rgba(255, 255, 255, 0.08);
		font-size: 0.72rem;
		color: var(--text-secondary);
	}

	.actions {
		align-items: center;
	}

	.score {
		margin-right: auto;
		color: var(--accent);
		font-weight: 600;
	}

	.btn-sm {
		padding: 6px 10px;
		font-size: 0.78rem;
	}

	.empty-state {
		padding: 32px 12px;
		text-align: center;
		color: var(--text-secondary);
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 14px;
	}

	.compute-status {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 8px;
	}

	.poll-sub {
		font-size: 0.75rem;
		opacity: 0.5;
	}

	.spinner {
		width: 28px;
		height: 28px;
		border: 2px solid rgba(255, 255, 255, 0.1);
		border-top-color: var(--accent, #7c80ff);
		border-radius: 50%;
		animation: spin 0.9s linear infinite;
	}

	@keyframes spin {
		to { transform: rotate(360deg); }
	}

	@media (max-width: 640px) {
		.results-grid {
			grid-template-columns: 1fr;
		}

		.seed-row {
			flex-direction: column;
			align-items: stretch;
		}
	}
</style>
