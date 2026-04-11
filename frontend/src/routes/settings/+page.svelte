<script lang="ts">
	import { onMount } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import {
		api,
		getApiBase,
		type DiscoveryStatus,
		type MusicBrainzStatus,
		type PlaybackRuntimeInfo,
		type PortableMusicBrainzSnapshotStatus
	} from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import {
		tidalStatus,
		tidalUserId,
		syncStatus,
		syncProgress,
		syncInfo,
		loadTidalStatus as refreshTidalStatus,
		loadSyncInfo,
		setAutoSyncDaily
	} from '$lib/stores/tidal';
	import {
		audioAnalysis,
		startAnalysis,
		stopAnalysis,
		clearAllAnalysis,
		loadAudioStats
	} from '$lib/stores/audio_analysis';
	import {
		acrCloud,
		loadAcrCloudStatus,
		configureAcrCloud,
		deleteAcrCloudConfig,
		startAcrCloudScan
	} from '$lib/stores/acrcloud';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';

	const SERVER_UNREACHABLE_MESSAGE =
		'NOOR cannot reach the local server on port 3334, so it cannot verify your current TIDAL session.';
	type BadgeTone = 'default' | 'active' | 'success' | 'warning' | 'error' | 'muted';

	let serverStatus = $state<'checking' | 'online' | 'offline'>('checking');
	let userCode = $state('');
	let verifyUrl = $state('');
	let errorMsg = $state('');
	let playbackRuntime = $state<PlaybackRuntimeInfo | null>(null);
	let runtimeAvailable = $state(false);
	let pollTimer: ReturnType<typeof setInterval> | null = null;
	let mbPollTimer: ReturnType<typeof setInterval> | null = null;
	let wsUnsubscribe: Unsubscriber | null = null;

	let mbStatus = $state<'idle' | 'running' | 'done'>('idle');
	let mbLiveProgress = $state<number | null>(null);
	let mbProgressLabel = $state('');
	let mbStats = $state<MusicBrainzStatus | null>(null);
	let portableSnapshot = $state<PortableMusicBrainzSnapshotStatus | null>(null);
	let discoveryStatus = $state<DiscoveryStatus | null>(null);
	let portableAction = $state<'export' | 'import' | null>(null);
	let portableStatusLabel = $state('');
	let galaxyRefreshLabel = $state('');

	// ACRCloud form state
	let acrKey = $state('');
	let acrSecret = $state('');
	let acrRegion = $state('eu-west-1');

	async function connectAcrCloud() {
		await configureAcrCloud(acrKey, acrSecret, acrRegion);
	}

	async function refreshGalaxy() {
		galaxyRefreshLabel = 'Refreshing genre data…';
		try {
			const genres = await api.getGenres();
			const heat = await api.getGenreHeat(90);
			markServerOnline();
			const genreCount = countGenres(genres.genres);
			const activeHeat = heat.heat.filter((e) => e.listen_count > 0).length;
			galaxyRefreshLabel = `Galaxy ready: ${genreCount} genres, ${activeHeat} with recent heat data.`;
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				galaxyRefreshLabel = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				galaxyRefreshLabel = `Galaxy refresh failed: ${error}`;
			}
		}
	}

	function countGenres(genres: any[]): number {
		let count = 0;
		for (const genre of genres) {
			count += 1 + countGenres(genre.children ?? []);
		}
		return count;
	}

	onMount(() => {
		wsUnsubscribe = wsMessages.subscribe((messages) => {
			const latest = messages.at(-1);
			if (!latest) return;

			if (latest.type === 'connected') {
				markServerOnline();
				void refreshTidalStatus();
				void loadSyncInfo();
				void loadPlaybackRuntime();
				void loadMbStatus();
				void loadPortableSnapshot();
				void loadDiscoveryStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'musicbrainz') {
				mbStatus = 'running';
				mbLiveProgress = typeof latest.progress === 'number' ? latest.progress : mbLiveProgress;
				void loadMbStatus();
			}

			if (
				latest.type === 'playback_changed' ||
				latest.type === 'track_changed' ||
				latest.type === 'playback_failed'
			) {
				void loadPlaybackRuntime();
			}
		});

		void refreshTidalStatus();
		void loadSyncInfo();
		void loadPlaybackRuntime();
		void loadMbStatus();
		void loadPortableSnapshot();
		void loadDiscoveryStatus();
		void loadAudioStats();
		void loadAcrCloudStatus();

		return () => {
			if (pollTimer) clearInterval(pollTimer);
			if (mbPollTimer) clearInterval(mbPollTimer);
			wsUnsubscribe?.();
		};
	});

	function isFetchConnectionError(error: unknown): boolean {
		return (
			error instanceof Error &&
			(error.name === 'TypeError' || /failed to fetch|networkerror|load failed/i.test(error.message))
		);
	}

	function markServerOnline() {
		serverStatus = 'online';
		if (errorMsg === SERVER_UNREACHABLE_MESSAGE) errorMsg = '';
		if (mbProgressLabel === SERVER_UNREACHABLE_MESSAGE) mbProgressLabel = '';
	}

	function markServerOffline() {
		serverStatus = 'offline';
	}

	async function connectTidal() {
		tidalStatus.set('connecting');
		errorMsg = '';
		try {
			const resp = await fetch(`${getApiBase()}/api/tidal/login`, { method: 'POST' });
			markServerOnline();
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			const data = await resp.json();
			userCode = data.user_code;
			verifyUrl = data.verify_url;

			window.open(verifyUrl, '_blank');

			pollTimer = setInterval(async () => {
				try {
					const pollResp = await fetch(`${getApiBase()}/api/tidal/login/poll`, { method: 'POST' });
					markServerOnline();
					const pollData = await pollResp.json();
					if (pollData.status === 'authenticated') {
						tidalStatus.set('connected');
						tidalUserId.set(pollData.user_id);
						userCode = '';
						verifyUrl = '';
						if (pollTimer) clearInterval(pollTimer);
						pollTimer = null;
					}
				} catch (error) {
					if (isFetchConnectionError(error)) markServerOffline();
				}
			}, 3000);
		} catch (e) {
			tidalStatus.set('disconnected');
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				errorMsg = `Failed to connect: ${e}`;
			}
		}
	}

	function formatSyncDate(isoString: string): string {
		if (!isoString) return 'Never';
		// Handle both formats: with and without timezone
		const date = isoString.endsWith('Z') || isoString.includes('+') 
			? new Date(isoString) 
			: new Date(isoString + 'Z');
		const now = new Date();
		const diffMs = now.getTime() - date.getTime();
		const diffHours = Math.floor(diffMs / (1000 * 60 * 60));
		const diffDays = Math.floor(diffMs / (1000 * 60 * 60 * 24));

		if (diffHours < 1) return 'Just now';
		if (diffHours < 24) return `${diffHours}h ago`;
		if (diffDays < 7) return `${diffDays}d ago`;
		return date.toLocaleDateString();
	}

	async function toggleAutoSync() {
		const current = $syncInfo?.auto_sync_daily ?? false;
		await setAutoSyncDaily(!current);
	}

	async function syncLibrary() {
		syncStatus.set('syncing');
		syncProgress.set(0);
		errorMsg = '';
		try {
			const resp = await fetch(`${getApiBase()}/api/tidal/sync`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json().catch(() => ({}));
			if (!resp.ok) throw new Error(data.message ?? `Server returned ${resp.status}`);
			if (data.status && data.status !== 'sync_started') {
				throw new Error(data.message ?? 'Sync could not start');
			}
		} catch (e) {
			syncStatus.set('idle');
			syncProgress.set(null);
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				errorMsg = `Sync failed: ${e}`;
			}
		}
	}

	async function disconnectTidal() {
		try {
			const resp = await fetch(`${getApiBase()}/api/tidal/logout`, { method: 'POST' });
			markServerOnline();
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			tidalStatus.set('disconnected');
			tidalUserId.set('');
			userCode = '';
			verifyUrl = '';
			syncStatus.set('idle');
			syncProgress.set(null);
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
				return;
			}
			markServerOnline();
			errorMsg = `Failed to disconnect: ${error}`;
		}
	}

	async function loadPlaybackRuntime() {
		try {
			const response = await api.getPlaybackRuntime();
			markServerOnline();
			runtimeAvailable = response.available;
			playbackRuntime = response.runtime;
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
			}
		}
	}

	async function loadMbStatus() {
		try {
			mbStats = await api.getMusicBrainzStatus();
			markServerOnline();
			if (!mbStats) return;
			if (mbStats.remaining === 0) {
				mbStatus = 'done';
				mbLiveProgress = 1;
				if (mbPollTimer) {
					clearInterval(mbPollTimer);
					mbPollTimer = null;
				}
				return;
			}

			if (mbStatus === 'running' && mbStats.total_tracks > 0) {
				mbLiveProgress = mbStats.checked_tracks / mbStats.total_tracks;
			}
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
			}
		}
	}

	async function loadPortableSnapshot() {
		try {
			portableSnapshot = await api.getPortableMusicBrainzSnapshot();
			markServerOnline();
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				return;
			}
			portableStatusLabel = `Snapshot status failed: ${error}`;
		}
	}

	async function loadDiscoveryStatus() {
		try {
			const response = await api.getDiscoveryStatus();
			discoveryStatus = response.status;
			markServerOnline();
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
			}
		}
	}

	async function startDiscoveryTraining(mode: 'full' | 'incremental') {
		try {
			await api.startDiscoveryTraining(mode);
			await loadDiscoveryStatus();
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				errorMsg = `Discovery training failed: ${error}`;
			}
		}
	}

	async function startEnrichment() {
		mbStatus = 'running';
		mbProgressLabel = 'Starting the background queue…';
		try {
			const resp = await fetch(`${getApiBase()}/api/library/enrich/musicbrainz`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'already_complete') {
				mbStatus = 'done';
				mbLiveProgress = 1;
				mbProgressLabel = 'Everything already has MusicBrainz coverage.';
				if (mbPollTimer) {
					clearInterval(mbPollTimer);
					mbPollTimer = null;
				}
			} else {
				if (mbPollTimer) clearInterval(mbPollTimer);
				mbPollTimer = setInterval(() => {
					void loadMbStatus();
				}, 3000);
			}
		} catch (e) {
			mbStatus = 'idle';
			if (isFetchConnectionError(e)) {
				markServerOffline();
				mbProgressLabel = SERVER_UNREACHABLE_MESSAGE;
			} else {
				mbProgressLabel = `Failed: ${e}`;
			}
		}
		void loadMbStatus();
	}

	async function exportPortableSnapshot() {
		portableAction = 'export';
		portableStatusLabel = 'Writing the current MusicBrainz coverage into the portable snapshot…';
		try {
			const result = await api.exportPortableMusicBrainzSnapshot();
			markServerOnline();
			portableSnapshot = result.snapshot;
			portableStatusLabel = `Exported ${result.snapshot.checked_rows.toLocaleString()} checked tracks and ${result.snapshot.genre_rows.toLocaleString()} genre rows to ${result.snapshot.path}.`;
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				portableStatusLabel = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				portableStatusLabel = `Export failed: ${error}`;
			}
		} finally {
			portableAction = null;
			void loadPortableSnapshot();
			void loadMbStatus();
		}
	}

	async function importPortableSnapshot() {
		portableAction = 'import';
		portableStatusLabel = 'Applying the portable MusicBrainz snapshot into this library…';
		try {
			const result = await api.importPortableMusicBrainzSnapshot();
			markServerOnline();
			portableSnapshot = result.snapshot;
			portableStatusLabel = `Imported ${result.checked_inserted?.toLocaleString() ?? '0'} checked markers and ${result.genre_inserted?.toLocaleString() ?? '0'} genre rows.`;
			mbProgressLabel = 'Portable snapshot imported into the local library.';
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				portableStatusLabel = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				portableStatusLabel = `Import failed: ${error}`;
			}
		} finally {
			portableAction = null;
			void loadPortableSnapshot();
			void loadMbStatus();
		}
	}

	let tidalBadgeLabel = $derived(
		serverStatus === 'offline'
			? 'TIDAL unknown'
			: $tidalStatus === 'connected'
				? 'TIDAL connected'
				: $tidalStatus === 'connecting'
					? 'Authorizing TIDAL'
					: 'TIDAL offline'
	);
	let tidalBadgeTone = $derived<BadgeTone>(
		serverStatus === 'offline'
			? 'warning'
			: $tidalStatus === 'connected'
				? 'success'
				: $tidalStatus === 'connecting'
					? 'active'
					: 'muted'
	);
	let serverBadgeLabel = $derived(
		serverStatus === 'offline'
			? 'Server offline'
			: serverStatus === 'online'
				? 'Server online'
				: 'Checking server'
	);
	let serverBadgeTone = $derived<BadgeTone>(
		serverStatus === 'offline'
			? 'error'
			: serverStatus === 'online'
				? 'success'
				: 'muted'
	);

	let enrichmentPercent = $derived(
		mbStats && mbStats.total_tracks > 0 ? Math.round((mbStats.checked_tracks / mbStats.total_tracks) * 100) : 0
	);
	let enrichmentRunningPercent = $derived(
		mbLiveProgress !== null
			? Math.round(mbLiveProgress * 100)
			: mbStats && mbStats.total_tracks > 0
				? Math.round((mbStats.checked_tracks / mbStats.total_tracks) * 100)
				: 0
	);
	let enrichmentProcessedLabel = $derived(
		mbStats
			? `${mbStats.checked_tracks.toLocaleString()} processed · ${mbStats.enriched_tracks.toLocaleString()} tagged · ${mbStats.total_tracks.toLocaleString()} total`
			: 'Waiting for enrichment status'
	);
	let enrichmentStatusCopy = $derived(
		mbStatus === 'running'
			? `${enrichmentRunningPercent}% complete. ${mbStats?.remaining?.toLocaleString() ?? '—'} tracks still waiting.`
			: mbStatus === 'done'
				? 'Genre coverage is complete for the current library snapshot.'
				: 'Run enrichment in the background and this panel will keep updating.'
	);
	let portableGeneratedLabel = $derived(
		portableSnapshot?.generated_at
			? new Date(portableSnapshot.generated_at).toLocaleString()
			: 'Not exported yet'
	);
	let portableSnapshotCopy = $derived(
		portableSnapshot?.exists
			? 'Export here after enrichment, commit `data/musicbrainz`, then pull and import on the other machine.'
			: 'No portable snapshot is present yet. Export one here first, then commit and push it.'
	);
</script>

<svelte:head>
	<title>Settings | NOOR</title>
</svelte:head>

<div class="page-shell settings-page animate-in">
	<PageHeader
		eyebrow="Settings"
		title="Connections, sync, and playback runtime."
		subtitle="Everything operational lives here: service auth, library sync, genre enrichment, and the audio engine."
	>
		{#snippet meta()}
			<StateBadge label={tidalBadgeLabel} tone={tidalBadgeTone} />
			<StateBadge label={serverBadgeLabel} tone={serverBadgeTone} />
			<StateBadge label={runtimeAvailable ? 'Runtime active' : 'Runtime idle'} tone={runtimeAvailable ? 'active' : 'muted'} />
		{/snippet}
	</PageHeader>

	{#if errorMsg}
		<EmptyState title="Something needs attention" copy={errorMsg} />
	{/if}

	<section class="stat-grid">
		<MetricPair label="Sync" value={$syncStatus === 'syncing' ? `${$syncProgress ?? 0}%` : $syncStatus === 'done' ? 'Done' : 'Ready'} copy="Current TIDAL library sync state." />
		<MetricPair label="Enrichment" value={`${enrichmentPercent}%`} copy="Tracks with MusicBrainz genre coverage." />
		<MetricPair label="Output" value={playbackRuntime?.device_name ?? 'Waiting'} copy="Current playback target." />
	</section>

	<section class="settings-grid">
		<div class="settings-main">
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Streaming" title="Connect TIDAL" subtitle="Sign in once, then NOOR can sync favorites, playlists, and playback-ready metadata." />

				{#if serverStatus === 'offline' && $tidalStatus !== 'connecting'}
					<div class="auth-card glass">
						<p class="page-copy">
							NOOR cannot reach the backend on port 3334, so it cannot confirm whether your saved
							TIDAL session is still active.
						</p>
						<div class="action-row">
							<button class="btn btn-glass" onclick={() => void refreshTidalStatus()}>Retry status</button>
						</div>
					</div>
				{:else if $tidalStatus === 'disconnected'}
					<div class="action-row">
						<button class="btn btn-primary" onclick={connectTidal}>Connect TIDAL</button>
					</div>
				{:else if $tidalStatus === 'connecting'}
					<div class="auth-card glass">
						<p class="page-copy">Finish authorization in the browser, then return here.</p>
						<a class="verify-link" href={verifyUrl} target="_blank">{verifyUrl}</a>
						<div class="user-code">{userCode}</div>
						<p class="page-copy">Waiting for the device-code flow to complete.</p>
					</div>
				{:else}
					<div class="info-list">
						<div class="info-row">
							<span>Signed in as</span>
							<strong>{$tidalUserId}</strong>
						</div>
						<div class="info-row">
							<span>Last sync</span>
							<strong>
								{#if $syncStatus === 'syncing'}
									{$syncProgress ?? 0}% complete
								{:else if $syncInfo?.last_sync_at}
									{formatSyncDate($syncInfo.last_sync_at)}
									{#if $syncInfo.last_sync_track_count > 0}
										<span class="sync-count">
											({$syncInfo.last_sync_track_count.toLocaleString()} tracks)
										</span>
									{/if}
								{:else if $syncStatus === 'done'}
									Just completed
								{:else}
									Never synced
								{/if}
							</strong>
						</div>
						<div class="info-row">
							<span>Auto-sync daily</span>
							<strong>
								<label class="toggle-switch">
									<input
										type="checkbox"
										checked={$syncInfo?.auto_sync_daily ?? false}
										onchange={() => void toggleAutoSync()}
									/>
									<span class="toggle-slider"></span>
								</label>
							</strong>
						</div>
					</div>
					<div class="action-row">
						<button class="btn btn-primary" onclick={syncLibrary} disabled={$syncStatus === 'syncing'}>
							{$syncStatus === 'syncing' ? 'Syncing…' : $syncStatus === 'done' ? 'Sync again' : 'Sync library'}
						</button>
						<button class="btn btn-glass" onclick={disconnectTidal}>Disconnect</button>
					</div>
				{/if}
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Metadata" title="Run MusicBrainz enrichment" subtitle="Fill out genre coverage in the background so browsing, shuffle, and discovery have more signal." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Tagged" value={mbStats ? mbStats.enriched_tracks.toLocaleString() : '0'} copy="Tracks with genres found." />
					<MetricPair label="Remaining" value={mbStats ? mbStats.remaining.toLocaleString() : '—'} copy="Not yet queried from MusicBrainz." />
				</div>

				<div class="enrichment-progress">
					<div class="enrichment-progress-copy">
						<p>{enrichmentProcessedLabel}</p>
						<span>{enrichmentStatusCopy}</span>
					</div>
					<div class="enrichment-progress-rail" aria-hidden="true">
						<div class="enrichment-progress-fill" style={`width: ${enrichmentRunningPercent}%`}></div>
					</div>
					{#if mbProgressLabel}
						<p class="page-copy">{mbProgressLabel}</p>
					{/if}
				</div>

				<div class="action-row">
					<button class="btn btn-primary" onclick={startEnrichment} disabled={mbStatus === 'running' || mbStats?.remaining === 0}>
						{mbStatus === 'running'
							? 'Running…'
							: mbStats?.remaining === 0
								? 'All enriched'
								: mbStats && mbStats.checked_tracks > 0
									? 'Resume enrichment'
									: 'Start enrichment'}
					</button>
					<button class="btn btn-glass" onclick={refreshGalaxy}>Refresh genre galaxy</button>
				</div>
				{#if galaxyRefreshLabel}
					<p class="galaxy-refresh-label">{galaxyRefreshLabel}</p>
				{/if}
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Learning" title="Discovery engine" subtitle="Track how much of the library the learned radio engine has covered, and refresh it when listening behavior changes." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Coverage" value={discoveryStatus ? `${Math.round(discoveryStatus.coverage_ratio * 100)}%` : '—'} copy="Playable tracks with learned neighborhoods." />
					<MetricPair label="Embedded" value={discoveryStatus?.embedded_tracks?.toLocaleString() ?? '0'} copy="Tracks with stored embedding vectors." />
				</div>

				<div class="portable-card glass">
					<div class="info-list">
						<div class="info-row">
							<span>Active model</span>
							<strong>{discoveryStatus?.active_model?.model_key ?? 'Fallback only'}</strong>
						</div>
						<div class="info-row">
							<span>Last trained</span>
							<strong>{discoveryStatus?.active_model?.trained_at ? new Date(discoveryStatus.active_model.trained_at + 'Z').toLocaleString() : '—'}</strong>
						</div>
						<div class="info-row">
							<span>Clip features</span>
							<strong>{discoveryStatus?.clip_cache_tracks?.toLocaleString() ?? '0'}</strong>
						</div>
						<div class="info-row">
							<span>Latest run</span>
							<strong>
								{#if discoveryStatus?.latest_run}
									{discoveryStatus.latest_run.status} · {discoveryStatus.latest_run.stage} · {Math.round(discoveryStatus.latest_run.progress * 100)}%
								{:else}
									idle
								{/if}
							</strong>
						</div>
					</div>
				</div>

				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void startDiscoveryTraining('incremental')}>Incremental refresh</button>
					<button class="btn btn-glass" onclick={() => void startDiscoveryTraining('full')}>Full retrain</button>
				</div>
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Transfer" title="Portable MusicBrainz snapshot" subtitle="Move enrichment between machines through the repo snapshot in `data/musicbrainz`." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Snapshot checked" value={portableSnapshot?.checked_rows?.toLocaleString() ?? '0'} copy="Tracks marked as already processed." />
					<MetricPair label="Snapshot genres" value={portableSnapshot?.genre_rows?.toLocaleString() ?? '0'} copy="Genre rows ready to import elsewhere." />
				</div>

				<div class="portable-card glass">
					<div class="info-list">
						<div class="info-row">
							<span>Snapshot state</span>
							<strong>{portableSnapshot?.exists ? 'Available' : 'Missing'}</strong>
						</div>
						<div class="info-row">
							<span>Generated</span>
							<strong>{portableGeneratedLabel}</strong>
						</div>
						<div class="info-row">
							<span>Path</span>
							<strong class="path-value">{portableSnapshot?.path ?? 'data/musicbrainz'}</strong>
						</div>
					</div>
					<p class="page-copy">{portableSnapshotCopy}</p>
					{#if portableStatusLabel}
						<p class="page-copy">{portableStatusLabel}</p>
					{/if}
				</div>

				<div class="action-row">
					<button class="btn btn-primary" onclick={exportPortableSnapshot} disabled={portableAction !== null}>
						{portableAction === 'export' ? 'Exporting…' : 'Export snapshot'}
					</button>
					<button
						class="btn btn-glass"
						onclick={importPortableSnapshot}
						disabled={portableAction !== null || !portableSnapshot?.exists}
					>
						{portableAction === 'import' ? 'Importing…' : 'Import snapshot'}
					</button>
				</div>
			</section>
		</div>

		<div class="settings-side">
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Playback" title="Audio runtime" subtitle="Current device and output format." />
				<div class="info-list">
					<div class="info-row">
						<span>Device</span>
						<strong>{playbackRuntime?.device_name ?? 'No device reported yet'}</strong>
					</div>
					<div class="info-row">
						<span>Format</span>
						<strong>
							{#if playbackRuntime}
								{playbackRuntime.sample_rate} Hz · {playbackRuntime.channels} ch
							{:else}
								Waiting for runtime
							{/if}
						</strong>
					</div>
					<div class="info-row">
						<span>Active track</span>
						<strong>{playbackRuntime?.active_track_id ?? 'None'}</strong>
					</div>
				</div>

				{#if playbackRuntime?.last_error}
					<p class="runtime-error">{playbackRuntime.last_error}</p>
				{/if}
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Later" title="Additional services" subtitle="Planned expansion beyond the current TIDAL workflow." />
				<div class="roadmap-list">
					<div class="roadmap-item">
						<h4>YouTube Music</h4>
						<p>Library sync and metadata, without playback.</p>
					</div>
					<div class="roadmap-item">
						<h4>SoundCloud</h4>
						<p>Experimental discovery support and lighter-weight source coverage.</p>
					</div>
				</div>
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="DSP" title="Audio Analysis" subtitle="Extract BPM, key, energy, and danceability from your library." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Analyzed" value={$audioAnalysis.analyzed.toLocaleString()} copy="Tracks with DSP features." />
					<MetricPair label="Avg BPM" value={$audioAnalysis.stats?.avg_bpm?.toFixed(1) ?? '—'} copy="Average tempo across analyzed tracks." />
					<MetricPair label="Top Key" value={$audioAnalysis.stats?.top_key ?? '—'} copy="Most common key signature." />
					<MetricPair label="Avg Energy" value={$audioAnalysis.stats?.avg_energy?.toFixed(2) ?? '—'} copy="Average energy level (0–1)." />
				</div>

				{#if $audioAnalysis.isRunning}
					<div class="progress-bar">
						<div class="progress-fill" style="width: {($audioAnalysis.total > 0 ? $audioAnalysis.analyzed / $audioAnalysis.total : 0) * 100}%"></div>
					</div>
					<p class="analysis-progress-label">
						Analyzing... {$audioAnalysis.analyzed.toLocaleString()} / {$audioAnalysis.total.toLocaleString()} tracks ({Math.round(($audioAnalysis.total > 0 ? $audioAnalysis.analyzed / $audioAnalysis.total : 0) * 100)}%)
					</p>
				{/if}

				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void startAnalysis('preview')} disabled={$audioAnalysis.isRunning}>
						{$audioAnalysis.isRunning ? 'Analyzing…' : 'Analyze Library (TIDAL)'}
					</button>
					<button class="btn btn-glass" onclick={stopAnalysis} disabled={!$audioAnalysis.isRunning}>Stop</button>
					<button class="btn btn-glass danger" onclick={clearAllAnalysis}>Clear All</button>
				</div>

				<details class="advanced-details">
					<summary>Advanced Settings</summary>
					<div class="setting-row">
						<label>Max duration per track (seconds)</label>
						<input type="number" value="30" min="10" max="120" />
					</div>
					<div class="setting-row">
						<label>Re-analyze interval (days)</label>
						<input type="number" value="30" min="1" max="365" />
					</div>
				</details>
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Recognition" title="Sample Recognition (ACRCloud)" subtitle="Identify samples and covers in your library via ACRCloud." />

				{#if !$acrCloud.connected}
					<p class="page-copy">Connect ACRCloud to identify samples in your library.</p>
					<div class="form-row">
						<input type="text" placeholder="Access Key" bind:value={acrKey} />
						<input type="password" placeholder="Access Secret" bind:value={acrSecret} />
						<select bind:value={acrRegion}>
							<option value="eu-west-1">EU (Ireland)</option>
							<option value="us-east-1">US (Virginia)</option>
						</select>
						<button class="btn btn-primary" onclick={connectAcrCloud}>Connect</button>
					</div>
				{:else}
					<div class="status-row">
						<StateBadge label="Connected" tone="success" />
						<span class="acrcloud-daily-count">{$acrCloud.scanned_today.toLocaleString()} / {$acrCloud.daily_limit.toLocaleString()} requests today</span>
					</div>
					{#if $acrCloud.isScanning}
						<div class="progress-bar">
							<div class="progress-fill" style="width: {($acrCloud.total > 0 ? $acrCloud.scanned / $acrCloud.total : 0) * 100}%"></div>
						</div>
						<p class="analysis-progress-label">
							Scanning... {$acrCloud.scanned.toLocaleString()} / {$acrCloud.total.toLocaleString()} ({$acrCloud.matches_found} matches)
						</p>
					{/if}
					<div class="action-row">
						<button class="btn btn-primary" onclick={() => void startAcrCloudScan()} disabled={$acrCloud.isScanning}>
							{$acrCloud.isScanning ? 'Scanning…' : 'Scan Library'}
						</button>
						<button class="btn btn-glass danger" onclick={() => void deleteAcrCloudConfig()}>Disconnect</button>
					</div>
				{/if}
			</section>
		</div>
	</section>
</div>

<style>
	.settings-grid {
		display: grid;
		grid-template-columns: minmax(0, 1.15fr) minmax(300px, 0.85fr);
		gap: var(--space-4);
	}

	.settings-main,
	.settings-side {
		display: flex;
		flex-direction: column;
		gap: var(--space-4);
	}

	.section-panel {
		padding: 22px;
		display: flex;
		flex-direction: column;
		gap: 18px;
	}

	.inner-metrics {
		grid-template-columns: repeat(2, minmax(0, 1fr));
	}

	.enrichment-progress {
		display: grid;
		gap: 12px;
	}

	.enrichment-progress-copy {
		display: grid;
		gap: 4px;
	}

	.enrichment-progress-copy p {
		margin: 0;
		font-size: 0.96rem;
		color: rgba(255, 255, 255, 0.92);
	}

	.enrichment-progress-copy span {
		font-size: 0.86rem;
		color: rgba(255, 255, 255, 0.62);
	}

	.enrichment-progress-rail {
		position: relative;
		height: 10px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		border: 1px solid rgba(255, 255, 255, 0.08);
		overflow: hidden;
	}

	.enrichment-progress-fill {
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, rgba(151, 126, 255, 0.85), rgba(120, 160, 255, 0.72));
		transition: width 200ms ease;
	}

	.action-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.auth-card {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.portable-card {
		padding: 16px;
		display: grid;
		gap: 12px;
	}

	.verify-link {
		color: var(--accent-strong);
		word-break: break-all;
	}

	.path-value {
		word-break: break-all;
	}

	.user-code {
		padding: 16px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.08);
		font-family: monospace;
		font-size: 1.9rem;
		letter-spacing: 0.18em;
		text-align: center;
	}

	.roadmap-list {
		display: grid;
		gap: 12px;
	}

	.runtime-error {
		color: var(--state-error);
	}

	.galaxy-refresh-label {
		margin: 4px 0 0;
		font-size: 0.82rem;
		color: var(--signal-text);
	}

	.roadmap-item {
		padding: 14px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.06);
	}

	.roadmap-item p {
		color: var(--text-secondary);
		margin-top: 6px;
	}

	@media (max-width: 960px) {
		.settings-grid {
			grid-template-columns: 1fr;
		}
	}

	@media (max-width: 640px) {
		.inner-metrics {
			grid-template-columns: 1fr;
		}

		.action-row {
			flex-direction: column;
		}

		.action-row :global(.btn) {
			width: 100%;
		}
	}

	/* Toggle switch for auto-sync */
	.toggle-switch {
		position: relative;
		display: inline-block;
		width: 44px;
		height: 24px;
		cursor: pointer;
	}

	.toggle-switch input {
		opacity: 0;
		width: 0;
		height: 0;
	}

	.toggle-slider {
		position: absolute;
		top: 0;
		left: 0;
		right: 0;
		bottom: 0;
		background: rgba(255, 255, 255, 0.12);
		border-radius: 24px;
		transition: background 0.2s ease;
	}

	.toggle-slider::before {
		content: '';
		position: absolute;
		height: 18px;
		width: 18px;
		left: 3px;
		bottom: 3px;
		background: white;
		border-radius: 50%;
		transition: transform 0.2s ease;
	}

	.toggle-switch input:checked + .toggle-slider {
		background: var(--accent);
	}

	.toggle-switch input:checked + .toggle-slider::before {
		transform: translateX(20px);
	}

	.toggle-switch input:disabled + .toggle-slider {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.sync-count {
		display: inline-block;
		margin-left: 4px;
		font-size: 0.85em;
		color: rgba(255, 255, 255, 0.5);
		font-weight: normal;
	}

	/* Audio analysis progress bar */
	.progress-bar {
		position: relative;
		height: 10px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		border: 1px solid rgba(255, 255, 255, 0.08);
		overflow: hidden;
	}

	.progress-fill {
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, rgba(151, 126, 255, 0.85), rgba(120, 160, 255, 0.72));
		transition: width 200ms ease;
	}

	.analysis-progress-label {
		font-size: 0.82rem;
		color: var(--text-secondary);
		margin: 6px 0 0;
	}

	/* Danger button */
	.btn.danger {
		background: rgba(232, 135, 138, 0.12);
		border: 1px solid rgba(232, 135, 138, 0.24);
		color: var(--state-error);
	}

	.btn.danger:hover:not(:disabled) {
		background: rgba(232, 135, 138, 0.2);
		border-color: rgba(232, 135, 138, 0.4);
	}

	/* Advanced details */
	.advanced-details {
		margin-top: 4px;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		padding: 10px 14px;
	}

	.advanced-details summary {
		cursor: pointer;
		font-size: 0.82rem;
		font-weight: 600;
		color: var(--text-secondary);
	}

	.setting-row {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-3);
		padding: 8px 0;
	}

	.setting-row label {
		font-size: 0.82rem;
		color: var(--text-secondary);
	}

	.setting-row input {
		width: 80px;
	}

	/* ACRCloud form row */
	.form-row {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
		align-items: center;
	}

	.form-row input,
	.form-row select {
		flex: 1;
		min-width: 140px;
	}

	/* ACRCloud status row */
	.status-row {
		display: flex;
		align-items: center;
		gap: var(--space-3);
	}

	.acrcloud-daily-count {
		font-size: 0.82rem;
		color: var(--text-secondary);
	}
</style>
