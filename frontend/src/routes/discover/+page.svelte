<script lang="ts">
	import { onMount } from 'svelte';
	import { get, type Unsubscriber } from 'svelte/store';
	import {
		api,
		type DiscoveryConnectionTrailItem,
		type DiscoveryExternalFeed,
		type DiscoveryExternalResult,
		type DiscoveryMode,
		type DiscoveryPreset,
		type DiscoveryProviderCapability,
		type DiscoveryService
	} from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import { formatDuration } from '$lib/stores/library';
	import { hydratePlayback, playerError } from '$lib/stores/player';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';

	type ActionTone = 'success' | 'error' | 'info';

	let prompt = $state('late-night drive with glassy synths');
	let mode = $state<DiscoveryMode>('mood');
	let useTidal = $state(true);
	let useSoundcloud = $state(false);
	let useBandcamp = $state(false);
	let useYtmusic = $state(false);
	let feed = $state<DiscoveryExternalFeed | null>(null);
	let presets = $state<DiscoveryPreset[]>([]);
	let trail = $state<DiscoveryConnectionTrailItem[]>([]);
	let loading = $state(true);
	let refreshing = $state(false);
	let savingPreset = $state(false);
	let actingTrackId = $state<string | null>(null);
	let error = $state<string | null>(null);
	let presetName = $state('');
	let isDirty = $state(false);
	let needsRefreshHint = $state(false);
	let actionMessage = $state<string | null>(null);
	let actionTone = $state<ActionTone>('info');
	let wsUnsubscribe: Unsubscriber | null = null;
	let actionMessageTimer: ReturnType<typeof setTimeout> | null = null;

	const modeOptions: Array<{ value: DiscoveryMode; label: string; copy: string }> = [
		{ value: 'mood', label: 'Mood', copy: 'Follow atmosphere and texture.' },
		{ value: 'reference', label: 'Reference', copy: 'Start from artists and adjacent records.' },
		{ value: 'dj', label: 'DJ', copy: 'Bias toward flow and mix-friendly movement.' },
		{ value: 'word-cloud', label: 'Word cloud', copy: 'Cast a broader language net.' }
	];

	const serviceOptions: Array<{ value: DiscoveryService; label: string; copy: string }> = [
		{ value: 'tidal', label: 'TIDAL', copy: 'Live now for search, save, play, and connections.' },
		{ value: 'soundcloud', label: 'SoundCloud', copy: 'Planned next.' },
		{ value: 'bandcamp', label: 'Bandcamp', copy: 'Planned later.' },
		{ value: 'ytmusic', label: 'YouTube Music', copy: 'Metadata-first later.' }
	];

	onMount(() => {
		wsUnsubscribe = wsMessages.subscribe((messages) => {
			const latest = messages.at(-1);
			if (!latest) return;
			if (latest.type !== 'listen_history_updated' && latest.type !== 'library_synced') return;
			if (!feed) return;
			if (isDirty) {
				needsRefreshHint = true;
				return;
			}
			void runDiscovery();
		});

		void loadPage();

		return () => {
			wsUnsubscribe?.();
			if (actionMessageTimer) clearTimeout(actionMessageTimer);
		};
	});

	function selectedServices(): DiscoveryService[] {
		return [
			...(useTidal ? (['tidal'] as DiscoveryService[]) : []),
			...(useSoundcloud ? (['soundcloud'] as DiscoveryService[]) : []),
			...(useBandcamp ? (['bandcamp'] as DiscoveryService[]) : []),
			...(useYtmusic ? (['ytmusic'] as DiscoveryService[]) : [])
		];
	}

	function setServices(services: string[]) {
		useTidal = services.includes('tidal');
		useSoundcloud = services.includes('soundcloud');
		useBandcamp = services.includes('bandcamp');
		useYtmusic = services.includes('ytmusic');
	}

	function markComposerDirty() {
		isDirty = true;
		error = null;
	}

	function setActionMessage(message: string, tone: ActionTone) {
		actionMessage = message;
		actionTone = tone;
		if (actionMessageTimer) clearTimeout(actionMessageTimer);
		actionMessageTimer = setTimeout(() => {
			actionMessage = null;
		}, 3200);
	}

	async function loadPage() {
		await Promise.all([runDiscovery(true), loadPresets()]);
	}

	async function loadPresets() {
		try {
			const response = await api.getDiscoveryPresets();
			presets = response.presets;
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		}
	}

	async function runDiscovery(initial = false) {
		const nextPrompt = prompt.trim();
		if (!nextPrompt) {
			error = 'Add a few words first so NOOR can search beyond your library.';
			return;
		}

		if (initial) loading = true;
		else refreshing = true;

		error = null;
		try {
			const response = await api.discoverNewMusic(nextPrompt, mode, selectedServices(), 10);
			feed = response.feed;
			trail = [];
			isDirty = false;
			needsRefreshHint = false;
		} catch (reason) {
			error = reason instanceof Error ? reason.message : String(reason);
		} finally {
			loading = false;
			refreshing = false;
		}
	}

	async function savePreset() {
		const name = presetName.trim() || prompt.trim();
		const trimmedPrompt = prompt.trim();
		if (!name || !trimmedPrompt) return;

		savingPreset = true;
		error = null;
		try {
			const response = await api.createDiscoveryPreset(name, trimmedPrompt, mode, selectedServices());
			presets = [response.preset, ...presets];
			presetName = '';
			setActionMessage(`Saved “${response.preset.name}”.`, 'success');
		} catch (reason) {
			const message = reason instanceof Error ? reason.message : String(reason);
			error = message;
			setActionMessage(message, 'error');
		} finally {
			savingPreset = false;
		}
	}

	function applyPreset(preset: DiscoveryPreset) {
		prompt = preset.prompt;
		mode = preset.mode;
		setServices(preset.services);
		isDirty = false;
		needsRefreshHint = false;
		void runDiscovery();
	}

	async function handleSave(result: DiscoveryExternalResult) {
		actingTrackId = result.provider_track_id;
		try {
			const response = await api.saveDiscoveryTrack(result);
			updateResultState(result.provider_track_id, { is_saved: true });
			setActionMessage(response.message, 'success');
		} catch (reason) {
			setActionMessage(reason instanceof Error ? reason.message : String(reason), 'error');
		} finally {
			actingTrackId = null;
		}
	}

	async function handlePlay(result: DiscoveryExternalResult) {
		actingTrackId = result.provider_track_id;
		try {
			playerError.set(null);
			const snapshot = await api.playDiscoveryTrack(result);
			hydratePlayback(snapshot);
			const latestError = get(playerError);
			if (latestError) {
				setActionMessage(latestError, 'error');
				return;
			}
			setActionMessage(`Now playing “${result.title}”.`, 'success');
		} catch (reason) {
			setActionMessage(reason instanceof Error ? reason.message : String(reason), 'error');
		} finally {
			actingTrackId = null;
		}
	}

	async function handleFindConnected(result: DiscoveryExternalResult) {
		actingTrackId = result.provider_track_id;
		error = null;
		try {
			const response = await api.findDiscoveryConnections(prompt.trim(), mode, selectedServices(), result, 8);
			feed = response.feed;
			if (response.feed.trail_item) {
				const nextTrail = [...trail];
				const exists = nextTrail.some(
					(item) =>
						item.provider === response.feed?.trail_item?.provider &&
						item.provider_track_id === response.feed?.trail_item?.provider_track_id
				);
				if (!exists) nextTrail.push(response.feed.trail_item);
				trail = nextTrail;
			}
			setActionMessage(`Opened a new connection trail from “${result.title}”.`, 'info');
		} catch (reason) {
			setActionMessage(reason instanceof Error ? reason.message : String(reason), 'error');
		} finally {
			actingTrackId = null;
		}
	}

	function updateResultState(providerTrackId: string, patch: Partial<DiscoveryExternalResult>) {
		if (!feed) return;
		feed = {
			...feed,
			results: feed.results.map((result) =>
				result.provider_track_id === providerTrackId ? { ...result, ...patch } : result
			)
		};
	}

	function capabilityFor(provider: DiscoveryService): DiscoveryProviderCapability | undefined {
		return capabilities.find((item) => item.provider === provider);
	}

	function providerEnabled(provider: DiscoveryService): boolean {
		return provider === 'tidal' || Boolean(capabilityFor(provider)?.can_fetch_connections);
	}

	function toggleService(provider: DiscoveryService) {
		if (!providerEnabled(provider)) return;
		if (provider === 'tidal') useTidal = !useTidal;
		if (provider === 'soundcloud') useSoundcloud = !useSoundcloud;
		if (provider === 'bandcamp') useBandcamp = !useBandcamp;
		if (provider === 'ytmusic') useYtmusic = !useYtmusic;
		markComposerDirty();
	}

	function modeLabel(value: DiscoveryMode): string {
		return modeOptions.find((option) => option.value === value)?.label ?? 'Mood';
	}

	function unique(values: string[]): string[] {
		const seen = new Set<string>();
		return values.filter((value) => {
			const key = value.trim().toLowerCase();
			if (!key || seen.has(key)) return false;
			seen.add(key);
			return true;
		});
	}

	function metadataContext(result: DiscoveryExternalResult): string | null {
		const parts = [
			result.discogs_label,
			result.discogs_year ? String(result.discogs_year) : null,
			result.discogs_styles.length ? result.discogs_styles.slice(0, 2).join(' · ') : null
		].filter(Boolean);
		return parts.length ? parts.join(' · ') : null;
	}

	function signalPills(result: DiscoveryExternalResult): string[] {
		return unique([
			...result.lastfm_tags.slice(0, 2).map((tag) => `Last.fm: ${tag}`),
			...result.discogs_styles.slice(0, 2).map((style) => `Discogs: ${style}`)
		]);
	}

	function resetTrail() {
		trail = [];
		void runDiscovery();
	}

	let capabilities = $derived(
		feed?.capabilities ?? [
			{ provider: 'tidal', can_save: true, can_play_inline: true, can_fetch_connections: true, can_map_genres: true },
			{ provider: 'soundcloud', can_save: false, can_play_inline: false, can_fetch_connections: false, can_map_genres: false },
			{ provider: 'bandcamp', can_save: false, can_play_inline: false, can_fetch_connections: false, can_map_genres: false },
			{ provider: 'ytmusic', can_save: false, can_play_inline: false, can_fetch_connections: false, can_map_genres: false }
		]
	);
	let activeServices = $derived(selectedServices());
	let selectedMode = $derived(modeOptions.find((option) => option.value === mode) ?? modeOptions[0]);
	let leadResult = $derived(feed?.results[0] ?? null);
	let secondaryResults = $derived(feed?.results.slice(1) ?? []);
	let composerState = $derived(
		needsRefreshHint
			? 'The library changed. Refresh when you want the feed updated.'
			: isDirty
				? 'Changes are waiting for a new search.'
				: 'Searching outward from your existing taste profile.'
	);
</script>

<svelte:head>
	<title>Discover | NOOR</title>
</svelte:head>

<div class="page-shell discover-page animate-in">
	<PageHeader
		eyebrow="Discover"
		title="Search outward from the library you already know."
		subtitle="Start with a scene, a reference, or a feeling. NOOR searches outside your collection while filtering out what you already have."
	>
		{#snippet actions()}
			<button class="btn btn-primary" onclick={() => void runDiscovery()} disabled={loading || refreshing || !useTidal}>
				{loading || refreshing ? 'Searching…' : isDirty || needsRefreshHint ? 'Refresh search' : 'Find music'}
			</button>
		{/snippet}
	</PageHeader>

	<section class="composer-panel glass-panel">
		<div class="composer-main">
			<label class="composer-field">
				<span class="eyebrow">Prompt</span>
				<textarea
					bind:value={prompt}
					rows="4"
					placeholder="Try: ecstatic deep house after midnight, hazy shoegaze with warmth, or dusty cosmic jazz"
					oninput={markComposerDirty}
				></textarea>
			</label>

			<div class="control-stack">
				<div class="chip-row">
					{#each modeOptions as option}
						<button
							class:selected={mode === option.value}
							class="mode-chip"
							onclick={() => {
								mode = option.value;
								markComposerDirty();
							}}
						>
							{option.label}
						</button>
					{/each}
				</div>

				<div class="provider-list">
					{#each serviceOptions as option}
						<button
							class:selected={activeServices.includes(option.value)}
							class:disabled={!providerEnabled(option.value)}
							class="provider-row"
							onclick={() => toggleService(option.value)}
						>
							<span>{option.label}</span>
							<p>{option.copy}</p>
						</button>
					{/each}
				</div>
			</div>
		</div>

		<div class="composer-side">
			<StateBadge label={composerState} tone={needsRefreshHint ? 'warning' : isDirty ? 'active' : 'muted'} />
			<StateBadge label={`Mode: ${selectedMode.label}`} tone="muted" />
			{#if actionMessage}
				<StateBadge label={actionMessage} tone={actionTone === 'success' ? 'success' : actionTone === 'error' ? 'error' : 'active'} />
			{/if}
			<input bind:value={presetName} type="text" placeholder="Optional scene name" />
			<button class="btn btn-glass" onclick={savePreset} disabled={savingPreset || !prompt.trim()}>
				{savingPreset ? 'Saving…' : 'Save scene'}
			</button>
		</div>
	</section>

	{#if error}
		<EmptyState title="Discovery paused" copy={error}>
			{#snippet actions()}
				<button class="btn btn-glass" onclick={() => void runDiscovery()} disabled={loading || refreshing}>Try again</button>
			{/snippet}
		</EmptyState>
	{/if}

	<div class="discover-layout">
		<section class="results-column">
			{#if loading}
				<div class="feed-loading">
					<span class="feed-loading-ring"></span>
					<p>Searching outside the library…</p>
				</div>
			{:else if leadResult}
				<article class="lead-card glass-panel">
					{#if leadResult.artwork_url}
						<img class="lead-art" src={leadResult.artwork_url} alt="" />
					{:else}
						<div class="lead-art placeholder">NOOR</div>
					{/if}

					<div class="lead-copy">
						<div class="lead-meta">
							<StateBadge label={leadResult.provider} tone="muted" compact={true} />
							{#if leadResult.in_library}
								<StateBadge label="Already in library" tone="warning" compact={true} />
							{:else}
								<StateBadge label="New to you" tone="success" compact={true} />
							{/if}
							{#if leadResult.is_saved}
								<StateBadge label="Saved" tone="active" compact={true} />
							{/if}
						</div>

						<div>
							<h2>{leadResult.title}</h2>
							<p class="lead-subtitle">{leadResult.artist_name ?? 'Unknown artist'}{leadResult.album_title ? ` · ${leadResult.album_title}` : ''}</p>
							<p class="lead-subtitle">
								{#if leadResult.duration_ms}
									{formatDuration(leadResult.duration_ms)}
								{/if}
								{#if leadResult.audio_quality}
									<span> · {leadResult.audio_quality}</span>
								{/if}
								{#if metadataContext(leadResult)}
									<span> · {metadataContext(leadResult)}</span>
								{/if}
							</p>
						</div>

						<div class="tag-row">
							{#each [...leadResult.tags, ...leadResult.normalized_genres].slice(0, 8) as tag}
								<span class="tag">{tag}</span>
							{/each}
						</div>

						<div class="lead-actions">
							<button class="btn btn-primary" onclick={() => void handleSave(leadResult)} disabled={actingTrackId === leadResult.provider_track_id || leadResult.is_saved}>
								{leadResult.is_saved ? 'Saved' : actingTrackId === leadResult.provider_track_id ? 'Working…' : 'Save'}
							</button>
							<button class="btn btn-glass" onclick={() => void handlePlay(leadResult)} disabled={actingTrackId === leadResult.provider_track_id || !leadResult.is_playable}>
								Play
							</button>
							<button class="btn btn-glass" onclick={() => void handleFindConnected(leadResult)} disabled={actingTrackId === leadResult.provider_track_id}>
								Find connected
							</button>
						</div>
					</div>
				</article>

				<div class="result-list">
					{#each secondaryResults as result}
						<article class="result-card glass">
							<div class="result-main">
								<div>
									<h3>{result.title}</h3>
									<p>{result.artist_name ?? 'Unknown artist'}{result.album_title ? ` · ${result.album_title}` : ''}</p>
									{#if metadataContext(result)}
										<p>{metadataContext(result)}</p>
									{/if}
								</div>
								<div class="tag-row compact">
									{#each [...result.tags, ...result.normalized_genres].slice(0, 5) as tag}
										<span class="tag">{tag}</span>
									{/each}
								</div>
							</div>
							<div class="result-side">
								<span class="score">{result.score}%</span>
								<div class="result-actions">
									<button class="btn btn-glass" onclick={() => void handleSave(result)} disabled={actingTrackId === result.provider_track_id || result.is_saved}>
										{result.is_saved ? 'Saved' : 'Save'}
									</button>
									<button class="btn btn-glass" onclick={() => void handlePlay(result)} disabled={actingTrackId === result.provider_track_id || !result.is_playable}>
										Play
									</button>
									<button class="btn btn-glass" onclick={() => void handleFindConnected(result)} disabled={actingTrackId === result.provider_track_id}>
										Connect
									</button>
								</div>
							</div>
						</article>
					{/each}
				</div>
			{:else}
				<EmptyState title="No external feed yet" copy="Start from a mood, reference, or scene and run the search." />
			{/if}
		</section>

		<aside class="support-column">
			<section class="glass-panel support-panel">
				<SectionHeader eyebrow="Trail" title="Connection trail" subtitle="Each time you ask for a connected result, the path builds here.">
					{#snippet actions()}
						{#if trail.length > 0}
							<button class="btn btn-glass" onclick={resetTrail}>Reset</button>
						{/if}
					{/snippet}
				</SectionHeader>
				{#if trail.length === 0}
					<EmptyState title="No trail yet" copy="Choose a result and follow it deeper." />
				{:else}
					<div class="support-list">
						{#each trail as item, index}
							<article class="support-card">
								<p class="eyebrow">Hop {index + 1}</p>
								<h4>{item.title}</h4>
								<p>{item.artist_name ?? 'Unknown artist'}</p>
								<p>{item.connection_reason}</p>
							</article>
						{/each}
					</div>
				{/if}
			</section>

			<section class="glass-panel support-panel">
				<SectionHeader eyebrow="Signals" title="Why the lead result fits" subtitle="Quiet metadata and similarity cues supporting the current pick." />
				{#if leadResult}
					<div class="support-list">
						{#if signalPills(leadResult).length}
							<div class="tag-row compact">
								{#each signalPills(leadResult) as signal}
									<span class="tag">{signal}</span>
								{/each}
							</div>
						{/if}
						{#each feed?.reasons ?? [] as reason}
							<article class="support-card">
								<div class="reason-top">
									<h4>{reason.label}</h4>
									<span>{reason.weight}%</span>
								</div>
								<p>{reason.detail}</p>
							</article>
						{/each}
					</div>
				{:else}
					<EmptyState title="Signals will appear here" copy="Run a search to see why a result surfaced." />
				{/if}
			</section>

			<section class="glass-panel support-panel">
				<SectionHeader eyebrow="Scenes" title="Saved searches" subtitle="Return to prompts that already worked well." />
				{#if presets.length === 0}
					<EmptyState title="No saved scenes yet" copy="Save a search when the prompt and provider mix feels right." />
				{:else}
					<div class="support-list">
						{#each presets as preset}
							<button class="preset-card" onclick={() => applyPreset(preset)}>
								<div>
									<h4>{preset.name}</h4>
									<p>{preset.prompt}</p>
								</div>
								<span>{modeLabel(preset.mode)} · {preset.services.join(' · ')}</span>
							</button>
						{/each}
					</div>
				{/if}
			</section>
		</aside>
	</div>
</div>

<style>
	.composer-panel {
		padding: 22px;
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(260px, 0.8fr);
		gap: var(--space-4);
	}

	.composer-main,
	.composer-side,
	.control-stack,
	.support-panel,
	.support-list {
		display: flex;
		flex-direction: column;
		gap: 14px;
	}

	.composer-field {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.chip-row {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.mode-chip {
		padding: 8px 12px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
	}

	.mode-chip.selected {
		background: rgba(124, 128, 255, 0.12);
		border-color: rgba(124, 128, 255, 0.22);
		color: var(--text-primary);
	}

	.provider-list {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 8px;
	}

	.provider-row {
		padding: 12px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.08);
		text-align: left;
	}

	.provider-row.selected {
		background: rgba(124, 128, 255, 0.1);
		border-color: rgba(124, 128, 255, 0.22);
	}

	.provider-row:hover:not(.disabled) {
		background: rgba(255, 255, 255, 0.06);
		border-color: rgba(255, 255, 255, 0.14);
	}

	.provider-row.disabled {
		opacity: 0.42;
		cursor: not-allowed;
	}

	.provider-row p {
		margin-top: 4px;
		color: var(--text-secondary);
	}

	.discover-layout {
		display: grid;
		grid-template-columns: minmax(0, 1.2fr) minmax(320px, 0.8fr);
		gap: var(--space-4);
	}

	.results-column,
	.support-column {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}

	.lead-card {
		padding: 22px;
		display: grid;
		grid-template-columns: 220px minmax(0, 1fr);
		gap: var(--space-4);
	}

	.lead-art {
		width: 100%;
		aspect-ratio: 1;
		border-radius: var(--radius);
		object-fit: cover;
		background: rgba(255, 255, 255, 0.03);
	}

	.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	.lead-copy,
	.result-main,
	.lead-actions,
	.result-actions {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.lead-meta {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.lead-subtitle,
	.result-card p,
	.preset-card span {
		color: var(--text-secondary);
	}

	.tag-row {
		display: flex;
		flex-wrap: wrap;
		gap: 8px;
	}

	.tag {
		padding: 6px 9px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.08);
		color: var(--text-secondary);
		font-size: 0.76rem;
	}

	.result-list {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.result-card {
		padding: 16px;
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--space-4);
	}

	.result-side {
		display: flex;
		flex-direction: column;
		align-items: flex-end;
		gap: 10px;
	}

	.score {
		font-family: var(--font-display);
		font-size: 1.6rem;
	}

	.result-actions {
		align-items: flex-end;
	}

	.support-panel {
		padding: 20px;
	}

	.support-card,
	.preset-card {
		padding: 14px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.06);
		text-align: left;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.preset-card:hover {
		background: rgba(255, 255, 255, 0.06);
		border-color: rgba(255, 255, 255, 0.12);
	}

	.tag-row.compact {
		gap: 5px;
	}

	.tag-row.compact .tag {
		padding: 4px 7px;
		font-size: 0.72rem;
	}

	.feed-loading {
		display: flex;
		flex-direction: column;
		align-items: center;
		gap: 16px;
		padding: 60px 0;
		color: var(--text-secondary);
		font-size: 0.9rem;
	}

	.feed-loading-ring {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		border: 2px solid var(--border-subtle);
		border-top-color: var(--accent);
		animation: spin 700ms linear infinite;
	}

	@keyframes spin {
		to { transform: rotate(360deg); }
	}

	.reason-top {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 8px;
		margin-bottom: 6px;
	}

	@media (max-width: 1060px) {
		.composer-panel,
		.discover-layout {
			grid-template-columns: 1fr;
		}
	}

	@media (max-width: 760px) {
		.provider-list {
			grid-template-columns: 1fr;
		}

		.lead-card,
		.result-card {
			grid-template-columns: 1fr;
			flex-direction: column;
		}

		.result-side,
		.result-actions {
			align-items: flex-start;
		}
	}
</style>
