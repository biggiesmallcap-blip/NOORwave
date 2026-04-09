<script lang="ts">
	import { onMount } from 'svelte';
	import type { Unsubscriber } from 'svelte/store';
	import { api, getApiBase, type PlaybackRuntimeInfo } from '$lib/api/client';
	import { wsMessages } from '$lib/api/ws';
	import PageHeader from '$lib/components/ui/PageHeader.svelte';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';

	const SERVER_UNREACHABLE_MESSAGE =
		'NOOR cannot reach the local server on port 3333, so it cannot verify your current TIDAL session.';
	type BadgeTone = 'default' | 'active' | 'success' | 'warning' | 'error' | 'muted';

	let tidalStatus = $state<'disconnected' | 'connecting' | 'connected'>('disconnected');
	let serverStatus = $state<'checking' | 'online' | 'offline'>('checking');
	let userCode = $state('');
	let verifyUrl = $state('');
	let tidalUserId = $state('');
	let syncStatus = $state<'idle' | 'syncing' | 'done'>('idle');
	let syncProgress = $state<number | null>(null);
	let errorMsg = $state('');
	let playbackRuntime = $state<PlaybackRuntimeInfo | null>(null);
	let runtimeAvailable = $state(false);
	let pollTimer: ReturnType<typeof setInterval> | null = null;
	let mbPollTimer: ReturnType<typeof setInterval> | null = null;
	let wsUnsubscribe: Unsubscriber | null = null;

	let mbStatus = $state<'idle' | 'running' | 'done'>('idle');
	let mbLiveProgress = $state<number | null>(null);
	let mbProgressLabel = $state('');
	let mbStats = $state<{ total_tracks: number; checked_tracks: number; enriched_tracks: number; remaining: number } | null>(null);

	onMount(() => {
		wsUnsubscribe = wsMessages.subscribe((messages) => {
			const latest = messages.at(-1);
			if (!latest) return;

			if (latest.type === 'connected') {
				markServerOnline();
				void loadTidalStatus();
				void loadPlaybackRuntime();
				void loadMbStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'tidal' && syncStatus === 'syncing') {
				syncProgress = Math.max(0, Math.min(100, Math.round((latest.progress ?? 0) * 100)));
			}

			if (latest.type === 'library_synced' && syncStatus === 'syncing') {
				syncStatus = 'done';
				syncProgress = 100;
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

		void loadTidalStatus();
		void loadPlaybackRuntime();
		void loadMbStatus();

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

	async function loadTidalStatus() {
		try {
			const resp = await fetch(`${getApiBase()}/api/tidal/status`);
			markServerOnline();
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			const data = await resp.json();
			if (data.connected) {
				tidalStatus = 'connected';
				tidalUserId = data.user_id;
				userCode = '';
				verifyUrl = '';
			} else {
				tidalStatus = 'disconnected';
				tidalUserId = '';
			}
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
			}
		}
	}

	async function connectTidal() {
		tidalStatus = 'connecting';
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
						tidalStatus = 'connected';
						tidalUserId = pollData.user_id;
						userCode = '';
						verifyUrl = '';
						if (pollTimer) clearInterval(pollTimer);
						pollTimer = null;
					}
				} catch (error) {
					if (isFetchConnectionError(error)) {
						markServerOffline();
					}
				}
			}, 3000);
		} catch (e) {
			tidalStatus = 'disconnected';
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				errorMsg = `Failed to connect: ${e}`;
			}
		}
	}

	async function syncLibrary() {
		syncStatus = 'syncing';
		syncProgress = 0;
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
			syncStatus = 'idle';
			syncProgress = null;
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
			tidalStatus = 'disconnected';
			tidalUserId = '';
			userCode = '';
			verifyUrl = '';
			syncStatus = 'idle';
			syncProgress = null;
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
			const resp = await fetch(`${getApiBase()}/api/library/enrich/musicbrainz/status`);
			markServerOnline();
			mbStats = await resp.json();
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

	let tidalBadgeLabel = $derived(
		serverStatus === 'offline'
			? 'TIDAL unknown'
			: tidalStatus === 'connected'
				? 'TIDAL connected'
				: tidalStatus === 'connecting'
					? 'Authorizing TIDAL'
					: 'TIDAL offline'
	);
	let tidalBadgeTone = $derived<BadgeTone>(
		serverStatus === 'offline'
			? 'warning'
			: tidalStatus === 'connected'
				? 'success'
				: tidalStatus === 'connecting'
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
		<MetricPair label="Sync" value={syncStatus === 'syncing' ? `${syncProgress ?? 0}%` : syncStatus === 'done' ? 'Done' : 'Ready'} copy="Current TIDAL library sync state." />
		<MetricPair label="Enrichment" value={`${enrichmentPercent}%`} copy="Tracks with MusicBrainz genre coverage." />
		<MetricPair label="Output" value={playbackRuntime?.device_name ?? 'Waiting'} copy="Current playback target." />
	</section>

	<section class="settings-grid">
		<div class="settings-main">
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Streaming" title="Connect TIDAL" subtitle="Sign in once, then NOOR can sync favorites, playlists, and playback-ready metadata." />

				{#if serverStatus === 'offline' && tidalStatus !== 'connecting'}
					<div class="auth-card glass">
						<p class="page-copy">
							NOOR cannot reach the backend on port 3333, so it cannot confirm whether your saved
							TIDAL session is still active.
						</p>
						<div class="action-row">
							<button class="btn btn-glass" onclick={() => void loadTidalStatus()}>Retry status</button>
						</div>
					</div>
				{:else if tidalStatus === 'disconnected'}
					<div class="action-row">
						<button class="btn btn-primary" onclick={connectTidal}>Connect TIDAL</button>
					</div>
				{:else if tidalStatus === 'connecting'}
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
							<strong>{tidalUserId}</strong>
						</div>
						<div class="info-row">
							<span>Sync state</span>
							<strong>
								{#if syncStatus === 'syncing'}
									{syncProgress ?? 0}% complete
								{:else if syncStatus === 'done'}
									Library synced
								{:else}
									Ready to sync
								{/if}
							</strong>
						</div>
					</div>
					<div class="action-row">
						<button class="btn btn-primary" onclick={syncLibrary} disabled={syncStatus === 'syncing'}>
							{syncStatus === 'syncing' ? 'Syncing…' : syncStatus === 'done' ? 'Sync again' : 'Sync library'}
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

	.verify-link {
		color: var(--accent-strong);
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
</style>
