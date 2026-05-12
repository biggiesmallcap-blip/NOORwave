<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import type { Unsubscriber } from 'svelte/store';
	import {
		api,
		getApiBase,
		authFetch,
		getStoredToken,
		setStoredToken,
		type AudioDevice,
		type AudioQuality,
		type VideoQualityMode,
		type DiscoveryEngine,
		type DiscoveryStatus,
		type DiscoveryTrainingSafetyProfile,
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
		syncError,
		loadTidalStatus as refreshTidalStatus,
		loadSyncInfo,
		setAutoSyncDaily,
		cancelTidalSync,
		startTidalSync
	} from '$lib/stores/tidal';
	import {
		audioAnalysis,
		clearAllAnalysis,
		loadAudioStats,
		loadPassiveDspState,
		setPassiveDspEnabled,
		syncAnalysisStatus
	} from '$lib/stores/audio_analysis';
	import {
		acrCloud,
		loadAcrCloudStatus,
		configureAcrCloud,
		deleteAcrCloudConfig,
		startAcrCloudScan
	} from '$lib/stores/acrcloud';
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import ShaderWallpaper from '$lib/components/wallpaper/ShaderWallpaper.svelte';
	import { WALLPAPERS, type WallpaperOption } from '$lib/components/wallpaper/shaders';
	import { wallpaper, setWallpaper } from '$lib/stores/wallpaper';
	import { PALETTES, type PaletteId } from '$lib/components/wallpaper/palettes';
	import { palette, setPalette } from '$lib/stores/palette';
	import { uiZoom, setZoom, zoomIn, zoomOut, resetZoom, MIN as ZOOM_MIN, MAX as ZOOM_MAX, WHEEL_STEP as ZOOM_STEP } from '$lib/stores/uiZoom';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { exclusiveStatus } from '$lib/stores/exclusive_status';
	import { openExternal } from '$lib/util/external';
	import { isValidTidalRedirectUrl, readTidalRedirectFromClipboard } from '$lib/tidal/login';

	const SERVER_UNREACHABLE_MESSAGE =
		'NOOR cannot reach the local server on port 3334, so it cannot verify your current TIDAL session.';
	type BadgeTone = 'default' | 'active' | 'success' | 'warning' | 'error' | 'muted';

	let serverStatus = $state<'checking' | 'online' | 'offline'>('checking');
	let verifyUrl = $state('');
	let tidalRedirectUrl = $state('');
	let tidalRedirectError = $state('');
	let tidalExternalOpenError = $state('');
	let errorMsg = $state('');
	let playbackRuntime = $state<PlaybackRuntimeInfo | null>(null);
	let runtimeAvailable = $state(false);
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

	let spotifyConfigured = $state(false);
	let spotifyClientId = $state('');
	let spotifyClientSecret = $state('');
	let spotifySaving = $state(false);
	let spotifyError = $state('');
	let spotifyEnrichedCount = $state(0);
	let spotifyRemaining = $state(0);
	let spotifyIsRunning = $state(false);
	let spotifyRunTotal = $state(0);
	let spotifyRunProcessed = $state(0);
	const SPOTIFY_BATCH_SIZE = 2000;

	let lastfmConfigured = $state(false);
	let lastfmApiKey = $state('');
	let lastfmSaving = $state(false);
	let lastfmError = $state('');
	let lastfmTotal = $state(0);
	let lastfmChecked = $state(0);
	let lastfmEnrichedCount = $state(0);
	let lastfmRemaining = $state(0);
	let lastfmIsRunning = $state(false);
	let lastfmRunTotal = $state(0);
	let lastfmRunProcessed = $state(0);
	let lastfmPrefetchTotal = $state(0);
	let lastfmPrefetchDone = $state(0);
	let lastfmRunStartedAt = $state(0);
	// Tick a "now" reference every second so the ETA derivation re-evaluates
	// while a run is in progress without needing extra status fetches.
	let nowEpochSeconds = $state(Math.floor(Date.now() / 1000));

	// Access token — initialised in onMount (localStorage unavailable during SSR)
	let serverToken = $state('');
	let tokenVisible = $state(false);
	let tokenCopied = $state(false);
	let tokenRegenerating = $state(false);
	let tokenRegenError = $state('');

	async function handleRegenerateToken() {
		if (!confirm('Regenerating the PIN will disconnect all other devices until they re-enter the new PIN. Continue?')) return;
		tokenRegenerating = true;
		tokenRegenError = '';
		try {
			const { token } = await api.regenerateServerToken();
			serverToken = token;
			setStoredToken(token);
		} catch {
			tokenRegenError = 'Failed to regenerate token.';
		} finally {
			tokenRegenerating = false;
		}
	}

	function copyToken() {
		navigator.clipboard.writeText(serverToken).then(() => {
			tokenCopied = true;
			setTimeout(() => (tokenCopied = false), 2000);
		});
	}

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

	// Seconds-variant (NOT milliseconds). Renamed from `formatDuration` so it can't be
	// silently switched with the canonical ms-based `formatDuration` in $lib/utils/format.
	function formatDurationSeconds(seconds: number | null | undefined): string {
		if (seconds === null || seconds === undefined) return '—';
		if (!isFinite(seconds) || seconds <= 0) return '—';
		const total = Math.round(seconds);
		const h = Math.floor(total / 3600);
		const m = Math.floor((total % 3600) / 60);
		const s = total % 60;
		if (h > 0) return `${h}h ${m}m`;
		if (m > 0) return `${m}m ${s}s`;
		return `${s}s`;
	}

	// Constant fall-back rate when a run hasn't produced enough samples yet.
	// Mirrors PER_TRACK_DELAY_MS in services/lastfm/enrichment.rs.
	const LASTFM_FALLBACK_SECONDS_PER_TRACK = 0.5;
	const DISCOVERY_SAFETY_TIMEOUT_MESSAGE = 'Laptop safety timeout stopped discovery training.';
	let lastfmRunRemaining = $derived(Math.max(0, lastfmRunTotal - lastfmRunProcessed));

	let lastfmEtaSeconds = $derived.by(() => {
		if (lastfmRunRemaining === 0) return 0;
		// While running, compute observed rate from elapsed wall time.
		if (lastfmIsRunning && lastfmRunStartedAt > 0 && lastfmRunProcessed > 0) {
			const elapsed = Math.max(1, nowEpochSeconds - lastfmRunStartedAt);
			const secondsPerTrack = elapsed / lastfmRunProcessed;
			return lastfmRunRemaining * secondsPerTrack;
		}
		// Pre-run estimate (or post-stop, before fresh status load): use the
		// total queue (`lastfmRemaining`) and the constant rate.
		const queue = lastfmIsRunning ? lastfmRunRemaining : lastfmRemaining;
		return queue * LASTFM_FALLBACK_SECONDS_PER_TRACK;
	});
	let lastfmEtaLabel = $derived(formatDurationSeconds(lastfmEtaSeconds));

	onMount(() => {
		const tick = setInterval(() => {
			nowEpochSeconds = Math.floor(Date.now() / 1000);
		}, 1000);
		const discoveryTrainingPoll = setInterval(() => {
			if (discoveryIsRunning) void loadDiscoveryStatus();
		}, 3000);
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
				void loadDiscoveryEngine();
				void loadDiscoverySafetyProfile();
				void loadSpotifyStatus();
				void loadLastfmStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'musicbrainz') {
				mbStatus = 'running';
				mbLiveProgress = typeof latest.progress === 'number' ? latest.progress : mbLiveProgress;
				void loadMbStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'spotify') {
				void loadSpotifyStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'lastfm') {
				void loadLastfmStatus();
			}

			if (latest.type === 'musicbrainz_enriched' && (spotifyIsRunning || lastfmIsRunning)) {
				void loadSpotifyStatus();
				void loadLastfmStatus();
			}

			if (latest.type === 'training_progress' && discoveryStatus?.latest_run) {
				discoveryStatus = {
					...discoveryStatus,
					latest_run: {
						...discoveryStatus.latest_run,
						progress: typeof latest.progress === 'number' ? latest.progress : discoveryStatus.latest_run.progress,
						stage: typeof latest.stage === 'string' ? latest.stage : discoveryStatus.latest_run.stage,
						items_done: typeof latest.tracks_done === 'number' ? latest.tracks_done : discoveryStatus.latest_run.items_done,
						items_total: typeof latest.tracks_total === 'number' ? latest.tracks_total : discoveryStatus.latest_run.items_total,
					}
				};
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
		void loadDiscoveryEngine();
		void loadDiscoveryIntensity();
		void loadDiscoverySafetyProfile();
		void loadDiscoverySafety();
		void loadAudioStats();
		void syncAnalysisStatus();
		void loadPassiveDspState();
		void loadAudioOutput();
		void loadAcrCloudStatus();
		void loadSpotifyStatus();
		void loadLastfmStatus();
		serverToken = getStoredToken() ?? '';
		return () => {
			if (mbPollTimer) clearInterval(mbPollTimer);
			clearInterval(discoveryTrainingPoll);
			clearInterval(tick);
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
		tidalRedirectError = '';
		tidalExternalOpenError = '';
		tidalRedirectUrl = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/login`, { method: 'POST' });
			markServerOnline();
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			const data = await resp.json();
			verifyUrl = data.verify_url ?? '';

			await openTidalVerifyUrl();
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

	async function openTidalVerifyUrl() {
		tidalExternalOpenError = '';
		if (!verifyUrl) return;
		const result = await openExternal(verifyUrl);
		if (!result.ok) {
			tidalExternalOpenError = `Browser did not open: ${result.error}. Copy this TIDAL sign-in link into your browser.`;
		}
	}

	async function completeTidalLogin() {
		errorMsg = '';
		tidalRedirectError = '';
		tidalExternalOpenError = '';
		if (!isValidTidalRedirectUrl(tidalRedirectUrl)) {
			tidalRedirectError = 'Paste the final TIDAL redirect URL to finish login.';
			return;
		}
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/login/complete`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ redirect_url: tidalRedirectUrl.trim() }),
			});
			markServerOnline();
			const data = await resp.json().catch(() => ({}));
			if (!resp.ok) throw new Error(data.error ?? `Server returned ${resp.status}`);
			tidalStatus.set('connected');
			tidalUserId.set(data.user_id ?? '');
			void refreshTidalStatus();
			verifyUrl = '';
			tidalExternalOpenError = '';
			tidalRedirectUrl = '';
		} catch (e) {
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				errorMsg = `Failed to finish TIDAL login: ${e}`;
			}
		}
	}

	async function pasteTidalRedirectUrl() {
		tidalRedirectError = '';
		const result = await readTidalRedirectFromClipboard();
		if (result.ok && result.redirectUrl) {
			tidalRedirectUrl = result.redirectUrl;
			return;
		}
		tidalRedirectError = result.error ?? 'Clipboard access failed. Paste the URL manually.';
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

	async function syncLibrary(mode: 'auto' | 'full' = 'auto') {
		// Don't flip syncStatus to 'syncing' until the server actually accepts
		// the request — otherwise an immediate network error or 409 leaves the
		// UI showing "Syncing…" for the duration of the failed POST.
		errorMsg = '';
		syncError.set(null);
		try {
			const resp = await startTidalSync(mode);
			markServerOnline();
			const data = await resp.json().catch(() => ({}));
			if (!resp.ok) throw new Error(data.message ?? `Server returned ${resp.status}`);
			if (data.status && data.status !== 'sync_started') {
				throw new Error(data.message ?? 'Sync could not start');
			}
			syncStatus.set('syncing');
			syncProgress.set(0);
		} catch (e) {
			syncStatus.set('error');
			syncProgress.set(null);
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
				syncError.set(SERVER_UNREACHABLE_MESSAGE);
			} else {
				markServerOnline();
				const msg = `Sync failed: ${e}`;
				errorMsg = msg;
				syncError.set(msg);
			}
		}
	}

	async function handleCancelSync() {
		await cancelTidalSync();
	}

	async function disconnectTidal() {
		tidalExternalOpenError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/logout`, { method: 'POST' });
			markServerOnline();
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			tidalStatus.set('disconnected');
			tidalUserId.set('');
			verifyUrl = '';
			tidalExternalOpenError = '';
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

	async function loadSpotifyStatus() {
		const [configResp, enrichResp] = await Promise.allSettled([
			authFetch(`${getApiBase()}/api/spotify/status`),
			authFetch(`${getApiBase()}/api/library/enrich/spotify/status`)
		]);
		if (configResp.status === 'fulfilled') {
			markServerOnline();
			const data = await configResp.value.json();
			spotifyConfigured = data.configured === true;
		} else if (isFetchConnectionError(configResp.reason)) {
			markServerOffline();
		}

		if (enrichResp.status === 'fulfilled') {
			const data2 = await enrichResp.value.json();
			spotifyEnrichedCount = data2.enriched_tracks ?? 0;
			spotifyRemaining = data2.remaining_tracks ?? 0;
			spotifyIsRunning = data2.is_running === true;
			spotifyRunTotal = data2.run_total ?? 0;
			spotifyRunProcessed = data2.run_processed ?? 0;
		}
	}

	async function saveSpotifyConfig() {
		spotifySaving = true;
		spotifyError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/spotify/config`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({
					client_id: spotifyClientId,
					client_secret: spotifyClientSecret
				})
			});
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'ok') {
				spotifyConfigured = true;
				spotifyClientId = '';
				spotifyClientSecret = '';
			} else {
				spotifyError = data.message ?? 'Failed to save Spotify credentials.';
			}
		} catch (e) {
			spotifyError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
		} finally {
			spotifySaving = false;
		}
	}

	async function clearSpotifyConfig() {
		try {
			await authFetch(`${getApiBase()}/api/spotify/config`, { method: 'DELETE' });
			markServerOnline();
		} catch {}
		spotifyConfigured = false;
		spotifyError = '';
	}

	async function startSpotifyEnrichment() {
		spotifyError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/enrich/spotify`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'error') {
				spotifyError = data.message ?? 'Spotify enrichment failed.';
			}
			await loadSpotifyStatus();
		} catch (e) {
			spotifyError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
		}
	}

	async function resetSpotifyEnrichment() {
		if (!confirm('Clear all Spotify check markers and tags? Tracks will be re-queried on the next run.')) return;
		spotifyError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/enrich/spotify/reset`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'error') {
				spotifyError = data.message ?? 'Reset failed.';
			}
			await loadSpotifyStatus();
		} catch (e) {
			spotifyError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
		}
	}

	let purgeRunning = $state(false);
	let purgeError = $state('');
	let purgeLastDeleted = $state<number | null>(null);

	async function purgeOrphanTidalStream() {
		if (
			!confirm(
				'Delete tidal_stream tracks that have no listen history, are not favorited, and are not in any queue or playlist?\n\nThis cascades to trained data referencing those tracks (embeddings, neighbours, transitions). It will not affect anything you have actually played or favorited. Storage saved is small (~200 bytes per track) — only run for tidiness.'
			)
		)
			return;
		purgeRunning = true;
		purgeError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/tidal-stream/purge`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'error') {
				purgeError = data.message ?? 'Purge failed.';
			} else {
				purgeLastDeleted = data.deleted ?? 0;
			}
		} catch (e) {
			purgeError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
		} finally {
			purgeRunning = false;
		}
	}

	async function loadLastfmStatus() {
		const [configResp, enrichResp] = await Promise.allSettled([
			authFetch(`${getApiBase()}/api/lastfm/status`),
			authFetch(`${getApiBase()}/api/library/enrich/lastfm/status`)
		]);
		if (configResp.status === 'fulfilled') {
			markServerOnline();
			const data = await configResp.value.json();
			lastfmConfigured = data.configured === true;
		} else if (isFetchConnectionError(configResp.reason)) {
			markServerOffline();
		}

		if (enrichResp.status === 'fulfilled') {
			const data2 = await enrichResp.value.json();
			lastfmTotal = data2.total_tracks ?? 0;
			lastfmChecked = data2.checked_tracks ?? 0;
			lastfmEnrichedCount = data2.enriched_tracks ?? 0;
			lastfmRemaining = data2.remaining_tracks ?? 0;
			lastfmIsRunning = data2.is_running === true;
			lastfmRunTotal = data2.run_total ?? 0;
			lastfmRunProcessed = data2.run_processed ?? 0;
			lastfmPrefetchTotal = data2.prefetch_total ?? 0;
			lastfmPrefetchDone = data2.prefetch_done ?? 0;
			lastfmRunStartedAt = data2.run_started_at ?? 0;
		}
	}

	async function saveLastfmConfig() {
		lastfmSaving = true;
		lastfmError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/lastfm/config`, {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify({ api_key: lastfmApiKey })
			});
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'ok') {
				lastfmConfigured = true;
				lastfmApiKey = '';
			} else {
				lastfmError = data.message ?? 'Failed to save Last.fm API key.';
			}
		} catch (e) {
			lastfmError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
		} finally {
			lastfmSaving = false;
		}
	}

	async function clearLastfmConfig() {
		try {
			await authFetch(`${getApiBase()}/api/lastfm/config`, { method: 'DELETE' });
			markServerOnline();
		} catch {}
		lastfmConfigured = false;
		lastfmError = '';
	}

	async function startLastfmEnrichment() {
		lastfmError = '';
		try {
			const refresh = lastfmRemaining === 0 && lastfmTotal > 0;
			const path = `/api/library/enrich/lastfm${refresh ? '?mode=refresh' : ''}`;
			const resp = await authFetch(`${getApiBase()}${path}`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'error') {
				lastfmError = data.message ?? 'Last.fm enrichment failed.';
			} else if (data.status === 'no_eligible_tracks') {
				lastfmError = 'Favorite tracks or albums before running Last.fm tags.';
			}
			await loadLastfmStatus();
		} catch (e) {
			lastfmError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
		}
	}

	async function stopLastfmEnrichment() {
		try {
			await authFetch(`${getApiBase()}/api/library/enrich/lastfm/stop`, { method: 'POST' });
			markServerOnline();
			await loadLastfmStatus();
		} catch (e) {
			if (isFetchConnectionError(e)) markServerOffline();
		}
	}

	async function resetLastfmEnrichment() {
		if (!confirm('Clear all Last.fm check markers and tags? Tracks will be re-queried on the next run.')) return;
		lastfmError = '';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/enrich/lastfm/reset`, { method: 'POST' });
			markServerOnline();
			const data = await resp.json();
			if (data.status === 'error') {
				lastfmError = data.message ?? 'Reset failed.';
			}
			await loadLastfmStatus();
		} catch (e) {
			lastfmError = e instanceof Error ? e.message : String(e);
			if (isFetchConnectionError(e)) markServerOffline();
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
			discoveryEngine = response.status.selected_engine;
			discoveryEngineTrainable = response.status.selected_engine_trainable;
			markServerOnline();
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
			}
		}
	}

	async function startDiscoveryTraining(mode: 'full' | 'incremental', rebuildAudio = false) {
		try {
			const response = await api.startDiscoveryTraining(mode, rebuildAudio);
			if (response.status === 'legacy_trainer_unavailable') {
				errorMsg = response.message ?? 'Switch to V2 to train discovery.';
			} else {
				errorMsg = '';
			}
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

	async function stopDiscoveryTraining() {
		try {
			await api.stopDiscoveryTraining();
			markServerOnline();
			await loadDiscoveryStatus();
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	let discoveryIsRunning = $derived(
		discoveryStatus?.latest_run?.status === 'running'
	);

	// Intensity tier + safety estimate. Both load once on mount and refresh
	// after the intensity changes so the safety preview reflects the new
	// setting before the user clicks Start.
	let discoveryIntensity: 'max' | 'medium' | 'low' = $state('medium');
	let discoveryEngine: DiscoveryEngine = $state('v2');
	let discoveryEngineTrainable = $state(true);
	let discoverySafety: Awaited<ReturnType<typeof api.getDiscoverySafety>> | null = $state(null);
	let discoverySafetyProfile: DiscoveryTrainingSafetyProfile = $state('balanced');
	let safetyProfileBusy = $state(false);
	let intensityBusy = $state(false);
	let engineBusy = $state(false);
	let dismissedSafetyRunId: number | null = $state(null);
	let discoverySafetyWatchdogRun = $derived.by(() => {
		const run = discoveryStatus?.latest_run;
		if (!run) return null;
		if (run.id === dismissedSafetyRunId) return null;
		if (run.status !== 'cancelled') return null;
		if (run.error_text !== DISCOVERY_SAFETY_TIMEOUT_MESSAGE) return null;
		return run;
	});

	async function loadDiscoveryIntensity() {
		try {
			const r = await api.getDiscoveryIntensity();
			discoveryIntensity = r.intensity;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function loadDiscoveryEngine() {
		try {
			const r = await api.getDiscoveryEngine();
			discoveryEngine = r.engine;
			discoveryEngineTrainable = r.trainable;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function loadDiscoverySafety() {
		try {
			discoverySafety = await api.getDiscoverySafety();
			discoverySafetyProfile = discoverySafety.safety_profile;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function loadDiscoverySafetyProfile() {
		try {
			const r = await api.getDiscoverySafetyProfile();
			discoverySafetyProfile = r.profile;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function changeDiscoveryEngine(next: DiscoveryEngine) {
		if (engineBusy) return;
		const previous = discoveryEngine;
		engineBusy = true;
		try {
			const r = await api.setDiscoveryEngine(next);
			discoveryEngine = r.engine;
			discoveryEngineTrainable = r.trainable;
			await loadDiscoveryStatus();
		} catch (err) {
			discoveryEngine = previous;
			if (isFetchConnectionError(err)) markServerOffline();
		} finally {
			engineBusy = false;
		}
	}

	async function changeIntensity(next: 'max' | 'medium' | 'low') {
		if (intensityBusy || next === discoveryIntensity) return;
		intensityBusy = true;
		try {
			await api.setDiscoveryIntensity(next);
			discoveryIntensity = next;
			await loadDiscoverySafety();
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		} finally {
			intensityBusy = false;
		}
	}

	async function changeSafetyProfile(next: DiscoveryTrainingSafetyProfile) {
		if (safetyProfileBusy || next === discoverySafetyProfile) return;
		safetyProfileBusy = true;
		try {
			const r = await api.setDiscoverySafetyProfile(next);
			discoverySafetyProfile = r.profile;
			await loadDiscoverySafety();
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		} finally {
			safetyProfileBusy = false;
		}
	}

	function stageLabel(stage: string | undefined): string {
		switch (stage) {
			case 'behavioral': return 'Learning listening patterns';
			case 'audio':      return 'Processing audio features';
			case 'fusion':     return 'Blending features';
			case 'neighbors':  return 'Computing neighbors';
			case 'in_degree':  return 'Ranking connections';
			case 'evaluate':   return 'Evaluating';
			default:           return 'Computing';
		}
	}


	const INTENSITY_PRESETS: Record<
		'max' | 'medium' | 'low',
		{ title: string; tagline: string; detail: string }
	> = {
		max: {
			title: 'Max',
			tagline: 'Best radio quality. Slowest training.',
			detail:
				'96-dim model with 64 neighbors per track and an 8-track context window. Cold tracks get full audio + metadata anchoring. Recommended for libraries under ~10k tracks or for overnight runs.',
		},
		medium: {
			title: 'Medium',
			tagline: 'Balanced. The default.',
			detail:
				'64-dim, 32 neighbors, 5-track window. Audio-proxy stage runs at smaller dimension. Indistinguishable from Max for most listening; ~50% of the wall-clock time.',
		},
		low: {
			title: 'Low',
			tagline: 'Fastest. Pure behavioral.',
			detail:
				'48-dim, 24 neighbors, 3-track window. Skips the audio-proxy stage entirely — cold tracks lose their metadata anchor, but the engine stays usable on modest hardware. Roughly 25% of Max’s time.',
		},
	};

	const DISCOVERY_ENGINE_PRESETS: Record<
		DiscoveryEngine,
		{ title: string; tagline: string; detail: string }
	> = {
		v2: {
			title: 'V2 recommended',
			tagline: 'Default engine. Directional, skip-aware, and external-aware.',
			detail:
				'Uses transition direction, weighted skips, expanded DSP tokens, support diagnostics, and sidecar external candidates. Recommended for automix and radio.',
		},
		v1: {
			title: 'V1 legacy',
			tagline: 'Optional fallback for older trained models.',
			detail:
				'Reads existing library-only V1 models for comparison or fallback. This build does not train V1, so V2 stays the default training path.',
		},
	};

	const DISCOVERY_SAFETY_PROFILES: Record<
		DiscoveryTrainingSafetyProfile,
		{ title: string; tagline: string; detail: string }
	> = {
		laptop_safe: {
			title: 'Laptop-safe',
			tagline: 'Cooler. Leaves more headroom.',
			detail: 'Uses up to 4 workers and keeps at least one core free. Best for battery, heat, and thin laptops.',
		},
		balanced: {
			title: 'Balanced',
			tagline: 'Default. Protects headroom without wasting desktops.',
			detail: 'Uses up to 8 workers and keeps two cores free when available. Recommended for most computers.',
		},
		performance: {
			title: 'Performance',
			tagline: 'Fastest. Opt in for strong cooling.',
			detail: 'Uses up to 16 workers and keeps one core free. Best for desktops, plugged-in workstations, and overnight runs.',
		},
	};

	async function startEnrichment() {
		mbStatus = 'running';
		mbProgressLabel = 'Starting the background queue…';
		try {
			const resp = await authFetch(`${getApiBase()}/api/library/enrich/musicbrainz`, { method: 'POST' });
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

	// ─── Category rail ───────────────────────────────────────────────────
	// Splits the previously stacked panels into focused pages. Each panel
	// belongs to exactly one category; empty columns are hidden by CSS.
	type SettingsCategory = 'appearance' | 'sources' | 'discovery' | 'audio' | 'data' | 'account';
	let activeCategory = $state<SettingsCategory>('appearance');
	let handledTidalLoginRequest = $state('');
	$effect(() => {
		const requested = page.url.searchParams.get('tidalLogin');
		if (requested !== '1') return;
		const key = page.url.href;
		if (handledTidalLoginRequest === key) return;
		handledTidalLoginRequest = key;
		activeCategory = 'sources';
		window.history.replaceState({}, '', '/settings');
		void connectTidal();
	});
	// Single shared preview — shader prop changes reuse the same GL context,
	// avoiding WebGL context churn from per-tile mount/unmount cycles.
	let previewShader = $state<string | null>(null);
	let previewTileId = $state<string | null>(null);
	let showExtendedShaders = $state(false);
	let previewUnmountTimer: ReturnType<typeof setTimeout> | null = null;

	function onTileEnter(option: WallpaperOption) {
		if (previewUnmountTimer) { clearTimeout(previewUnmountTimer); previewUnmountTimer = null; }
		previewShader = option.shader;
		previewTileId = option.id;
	}
	function onTileLeave() {
		previewUnmountTimer = setTimeout(() => {
			previewShader = null;
			previewTileId = null;
		}, 1200);
	}

	// ─── Audio output settings (TIDAL playback runtime) ─────────────────
	let audioDevices = $state<AudioDevice[]>([]);
	let isWindows = $derived(typeof navigator !== 'undefined' && /Win/i.test(navigator.platform));

	const AUDIO_QUALITY_OPTIONS: { value: AudioQuality; label: string }[] = [
		{ value: 'LOW', label: 'Low (96 kbps AAC)' },
		{ value: 'HIGH', label: 'High (320 kbps AAC)' },
		{ value: 'LOSSLESS', label: 'Lossless (CD quality FLAC)' },
		{ value: 'HI_RES_LOSSLESS', label: 'Hi-Res Lossless (up to 24-bit / 192 kHz FLAC)' }
	];

	const VIDEO_QUALITY_OPTIONS: { value: VideoQualityMode; label: string }[] = [
		{ value: 'MAX', label: 'Max available' },
		{ value: 'AUTO', label: 'Auto adaptive' }
	];

	async function loadAudioOutput() {
		await audioSettings.load();
		try {
			const resp = await api.listAudioDevices();
			audioDevices = resp.devices;
		} catch (err) {
			console.error('Failed to load audio devices', err);
		}
	}

	function onAudioQualityChange(e: Event) {
		const value = (e.target as HTMLSelectElement).value as AudioQuality;
		void audioSettings.patch({ quality: value });
	}

	function onAudioDeviceChange(e: Event) {
		const value = (e.target as HTMLSelectElement).value;
		void audioSettings.patch({ output_device: value === '__default__' ? null : value });
	}

	function onAudioExclusiveToggle(e: Event) {
		void audioSettings.patch({ exclusive_mode: (e.target as HTMLInputElement).checked });
	}

	function onAudioSrFollowToggle(e: Event) {
		void audioSettings.patch({ sample_rate_follow: (e.target as HTMLInputElement).checked });
	}

	function onExclusiveGraceChange(e: Event) {
		const v = parseInt((e.target as HTMLInputElement).value, 10);
		if (Number.isFinite(v)) {
			void audioSettings.patch({ exclusive_release_grace_secs: v });
		}
	}

	let retryingExclusive = $state(false);
	async function retryExclusive() {
		retryingExclusive = true;
		try {
			await api.retryAudioExclusive();
		} catch {
			// Server-side errors surface as ws audio_exclusive_failed events;
			// the banner stays red. No extra UI needed here.
		} finally {
			retryingExclusive = false;
		}
	}

	function disableExclusive() {
		void audioSettings.patch({ exclusive_mode: false });
	}

	// "Bit-perfect mode" is the audiophile defaults flipped on at once: max
	// available quality from Tidal, exclusive WASAPI grab so the OS mixer is
	// out of the path, and sample-rate-follow so the device runs at the FLAC's
	// native rate. Off mode reverts to the safer defaults that Just Work on
	// flaky DACs (CD-quality, shared output, fixed device rate).
	let bitPerfectActive = $derived(
		$audioSettings.settings?.quality === 'HI_RES_LOSSLESS' &&
		$audioSettings.settings?.exclusive_mode === true &&
		$audioSettings.settings?.sample_rate_follow === true
	);

	function onBitPerfectToggle(e: Event) {
		const enable = (e.target as HTMLInputElement).checked;
		if (enable) {
			void audioSettings.patch({
				quality: 'HI_RES_LOSSLESS',
				exclusive_mode: true,
				sample_rate_follow: true,
			});
		} else {
			void audioSettings.patch({
				quality: 'LOSSLESS',
				exclusive_mode: false,
				sample_rate_follow: false,
			});
		}
	}

	function onVideoQualityModeChange(e: Event) {
		const value = (e.target as HTMLSelectElement).value as VideoQualityMode;
		void audioSettings.patch({ video_quality_mode: value });
	}

	const settingsCategories: { id: SettingsCategory; label: string; icon: string; hint: string }[] = [
		{ id: 'appearance', label: 'Appearance', icon: '◐', hint: 'Theme + wallpaper' },
		{ id: 'sources', label: 'Sources', icon: '⟐', hint: 'Services + data' },
		{ id: 'discovery', label: 'Discovery', icon: '✦', hint: 'Learned radio engine' },
		{ id: 'audio', label: 'Audio', icon: '♪', hint: 'Runtime + DSP analysis' },
		{ id: 'data', label: 'Data', icon: '⇅', hint: 'Portable snapshots' },
		{ id: 'account', label: 'Account', icon: '⚙', hint: 'Server token' }
	];

	let visibleSettingsCategories = $derived(
		settingsCategories.filter((c) => c.id !== 'data' && c.id !== 'discovery')
	);

	let activeCategoryMeta = $derived(
		visibleSettingsCategories.find((category) => category.id === activeCategory) ?? visibleSettingsCategories[0]
	);

	function rgbCss(c: [number, number, number]): string {
		return `rgb(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)})`;
	}

	let activePalette = $derived(PALETTES.find((p) => p.id === $palette) ?? PALETTES[0]);
	let activeSwatches = $derived([
		activePalette.shader.c1,
		activePalette.shader.c2,
		activePalette.shader.c3,
		activePalette.shader.c4
	]);
</script>

<svelte:head>
	<title>Settings | NOOR</title>
</svelte:head>

<div class="page-shell settings-page animate-in">
	<header class="settings-command">
		<div class="settings-title">
			<p class="eyebrow">Settings</p>
			<h1>Settings</h1>
			<p>Sources, appearance, audio, and access.</p>
		</div>
		<div class="settings-status">
			<StateBadge label={tidalBadgeLabel} tone={tidalBadgeTone} />
			<StateBadge label={serverBadgeLabel} tone={serverBadgeTone} />
			<StateBadge label={runtimeAvailable ? 'Runtime active' : 'Runtime idle'} tone={runtimeAvailable ? 'active' : 'muted'} />
		</div>
	</header>

	{#if errorMsg}
		<EmptyState title="Something needs attention" copy={errorMsg} />
	{/if}

	{#if discoverySafetyWatchdogRun}
		<div class="safety-watchdog-popup glass-panel" role="status">
			<div>
				<strong>Discovery training paused for laptop safety.</strong>
				<p>
					Your computer was protected from a long high-CPU run. Try Medium or Low, keep the laptop plugged in, or run it later.
				</p>
			</div>
			<button class="btn btn-glass" type="button" onclick={() => dismissedSafetyRunId = discoverySafetyWatchdogRun?.id ?? null}>
				Close
			</button>
		</div>
	{/if}

	<section class="settings-status-strip">
		<div>
			<span>Sync</span>
			<strong>{$syncStatus === 'syncing' ? `${$syncProgress ?? 0}%` : $syncStatus === 'done' ? 'Done' : $syncStatus === 'error' ? 'Failed' : $syncStatus === 'cancelled' ? 'Cancelled' : 'Ready'}</strong>
		</div>
		<div>
			<span>Enrichment</span>
			<strong>{enrichmentPercent}%</strong>
		</div>
		<div>
			<span>Output</span>
			<strong>{playbackRuntime?.device_name ?? 'Waiting'}</strong>
		</div>
		<div>
			<span>Active panel</span>
			<strong>{activeCategoryMeta.label}</strong>
		</div>
	</section>

	<nav class="settings-rail" aria-label="Settings categories">
		{#each visibleSettingsCategories as cat (cat.id)}
			<button
				type="button"
				class="settings-rail-btn"
				class:active={activeCategory === cat.id}
				onclick={() => (activeCategory = cat.id)}
				aria-pressed={activeCategory === cat.id}
			>
				<span class="settings-rail-icon" aria-hidden="true">
					{#if cat.id === 'appearance'}
						<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8" /><path d="M12 4v16M4 12h16" /></svg>
					{:else if cat.id === 'sources'}
						<svg viewBox="0 0 24 24"><path d="M7 7h10v10H7z" /><path d="M12 2v5M12 17v5M2 12h5M17 12h5" /></svg>
					{:else if cat.id === 'discovery'}
						<svg viewBox="0 0 24 24"><path d="M12 3l2.3 6.2L21 12l-6.7 2.8L12 21l-2.3-6.2L3 12l6.7-2.8z" /></svg>
					{:else if cat.id === 'audio'}
						<svg viewBox="0 0 24 24"><path d="M9 18V5l10-2v13" /><circle cx="6" cy="18" r="3" /><circle cx="16" cy="16" r="3" /></svg>
					{:else if cat.id === 'data'}
						<svg viewBox="0 0 24 24"><path d="M4 7h16M4 12h16M4 17h16" /><path d="M8 4v16M16 4v16" /></svg>
					{:else}
						<svg viewBox="0 0 24 24"><circle cx="12" cy="8" r="4" /><path d="M5 21c1.5-4 4-6 7-6s5.5 2 7 6" /></svg>
					{/if}
				</span>
				<span class="settings-rail-copy">
					<strong>{cat.label}</strong>
					<span class="settings-rail-hint">{cat.hint}</span>
				</span>
			</button>
		{/each}
	</nav>

	<section
		class="settings-grid"
		class:single-column={activeCategory === 'appearance' || activeCategory === 'account'}
	>
		<div class="settings-main">
			{#if activeCategory === 'appearance'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Palette" title="Colour scheme" subtitle="UI accent and wallpaper colors." />
				<div class="palette-row">
					<select
						class="palette-select"
						value={$palette}
						onchange={(e) => setPalette((e.currentTarget as HTMLSelectElement).value as PaletteId)}
					>
						{#each PALETTES as p (p.id)}
							<option value={p.id}>{p.label} — {p.sublabel}</option>
						{/each}
					</select>
					<div class="palette-swatches" aria-hidden="true">
						{#each activeSwatches as c, i (i)}
							<span class="palette-swatch" style={`background: ${rgbCss(c)}`}></span>
						{/each}
					</div>
				</div>
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader
					eyebrow="Scale"
					title="Interface size"
					subtitle="Zoom the entire UI. Also: Ctrl + scroll, Ctrl + / − , Ctrl + 0 to reset."
				/>
				<div class="zoom-row">
					<button
						type="button"
						class="btn btn-glass btn-sm zoom-step"
						onclick={zoomOut}
						aria-label="Decrease interface size"
						disabled={$uiZoom <= ZOOM_MIN + 1e-6}
					>−</button>
					<input
						type="range"
						class="zoom-slider"
						min={ZOOM_MIN}
						max={ZOOM_MAX}
						step={ZOOM_STEP}
						value={$uiZoom}
						oninput={(e) => setZoom(parseFloat((e.currentTarget as HTMLInputElement).value))}
						aria-label="Interface size"
					/>
					<button
						type="button"
						class="btn btn-glass btn-sm zoom-step"
						onclick={zoomIn}
						aria-label="Increase interface size"
						disabled={$uiZoom >= ZOOM_MAX - 1e-6}
					>+</button>
					<span class="zoom-readout" aria-live="polite">{Math.round($uiZoom * 100)}%</span>
					<button
						type="button"
						class="btn btn-glass btn-sm"
						onclick={resetZoom}
						disabled={Math.abs($uiZoom - 1) < 1e-6}
					>Reset</button>
				</div>
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Wallpaper" title="Background" subtitle="Preview, then apply." />

				<div class="wallpaper-big-preview">
					{#if previewShader}
						<ShaderWallpaper shader={previewShader} maxDpr={1} interactive={true} />
					{:else if !previewShader && $wallpaper !== 'none'}
						<div class="wallpaper-big-preview-hint">
							<span>Hover a tile to preview</span>
						</div>
					{:else}
						<div class="wallpaper-big-preview-hint">
							<span>Hover a tile to preview</span>
						</div>
					{/if}
				</div>

				<div class="wallpaper-grid">
					{#each WALLPAPERS.filter(o => !o.extended) as option (option.id)}
						<button
							type="button"
							class="wallpaper-tile"
							class:active={$wallpaper === option.id}
							class:previewing={previewTileId === option.id}
							onclick={() => setWallpaper(option.id)}
							aria-pressed={$wallpaper === option.id}
							onpointerenter={() => onTileEnter(option)}
							onpointerleave={onTileLeave}
						>
							{#if !option.shader}
								<div class="wallpaper-tile-swatch wallpaper-tile-swatch-none"></div>
							{:else}
								<div class="wallpaper-tile-swatch"></div>
							{/if}
							<div class="wallpaper-tile-label">
								<strong>{option.label}</strong>
								{#if $wallpaper === option.id}
									<span class="wallpaper-active-badge">Active</span>
								{/if}
							</div>
						</button>
					{/each}
				</div>

				<button
					type="button"
					class="wallpaper-more-btn"
					onclick={() => { showExtendedShaders = !showExtendedShaders; }}
					aria-expanded={showExtendedShaders}
				>
					<span class="wallpaper-more-icon" class:open={showExtendedShaders}>▸</span>
					{showExtendedShaders ? 'Fewer shaders' : 'More shaders (11)'}
				</button>

				{#if showExtendedShaders}
					<div class="wallpaper-grid wallpaper-grid-extended">
						{#each WALLPAPERS.filter(o => o.extended) as option (option.id)}
							<button
								type="button"
								class="wallpaper-tile wallpaper-tile-mono"
								class:active={$wallpaper === option.id}
								class:previewing={previewTileId === option.id}
								onclick={() => setWallpaper(option.id)}
								aria-pressed={$wallpaper === option.id}
								onpointerenter={() => onTileEnter(option)}
								onpointerleave={onTileLeave}
							>
								<div class="wallpaper-tile-swatch wallpaper-tile-swatch-mono"></div>
								<div class="wallpaper-tile-label">
									<strong>{option.label}</strong>
									{#if $wallpaper === option.id}
										<span class="wallpaper-active-badge">Active</span>
									{/if}
								</div>
							</button>
						{/each}
					</div>
				{/if}
			</section>
			{/if}

			{#if activeCategory === 'account'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Access" title="Access PIN" subtitle="Use this PIN on another device." />
				<div class="token-row">
					<code class="token-value">{tokenVisible ? serverToken : '•'.repeat(serverToken.length || 6)}</code>
					<button class="btn btn-glass btn-sm" onclick={() => (tokenVisible = !tokenVisible)}>
						{tokenVisible ? 'Hide' : 'Show'}
					</button>
					<button class="btn btn-glass btn-sm" onclick={copyToken}>
						{tokenCopied ? 'Copied!' : 'Copy'}
					</button>
				</div>
				<div class="action-row">
					<button class="btn btn-glass" disabled={tokenRegenerating} onclick={() => void handleRegenerateToken()}>
						{tokenRegenerating ? 'Regenerating…' : 'Regenerate PIN'}
					</button>
					{#if tokenRegenError}<span class="field-error">{tokenRegenError}</span>{/if}
				</div>
				<p class="page-copy setting-caption">
					Regenerating disconnects all other devices — they'll need to re-enter the new PIN.
				</p>
			</section>
			{/if}

			{#if activeCategory === 'sources'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Streaming" title="Connect TIDAL" subtitle="Auth, sync, and playback metadata." />

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
						<p class="page-copy">A TIDAL sign-in page opened.</p>
						<p class="page-copy">After sign-in, copy the address from the final TIDAL page. Paste it here to finish.</p>
						<div class="action-row">
							<button type="button" class="btn btn-glass" onclick={() => void openTidalVerifyUrl()} disabled={!verifyUrl}>
								Open TIDAL sign-in
							</button>
						</div>
						{#if tidalExternalOpenError}
							<p class="error" role="alert">{tidalExternalOpenError}</p>
							<input class="text-field" type="url" readonly value={verifyUrl} aria-label="TIDAL sign-in URL" />
						{/if}
						<input
							class="text-field"
							type="url"
							bind:value={tidalRedirectUrl}
							placeholder="https://tidal.com/android/login/auth?code=..."
						/>
						{#if tidalRedirectError}
							<p class="error" role="alert">{tidalRedirectError}</p>
						{/if}
						<div class="action-row">
							<button class="btn btn-glass" onclick={pasteTidalRedirectUrl}>
								Paste from clipboard
							</button>
							<button class="btn btn-primary" onclick={completeTidalLogin} disabled={!tidalRedirectUrl.trim()}>
								Finish login
							</button>
						</div>
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
								{:else if $syncStatus === 'error'}
									Failed
								{:else if $syncStatus === 'cancelled'}
									Cancelled
								{:else if $syncInfo?.last_sync_at}
									{formatSyncDate($syncInfo.last_sync_at)}
									{#if $syncInfo.last_sync_kind}
										<span class="sync-count">
											{$syncInfo.last_sync_kind === 'incremental' ? 'fast' : 'full'}
										</span>
									{/if}
									{#if $syncInfo.last_sync_track_count > 0}
										<span class="sync-count">
											({$syncInfo.last_sync_track_count.toLocaleString()} tracks{#if $syncInfo.last_sync_album_count > 0}, {$syncInfo.last_sync_album_count.toLocaleString()} albums{/if})
										</span>
									{/if}
								{:else if $syncStatus === 'done'}
									Just completed
								{:else}
									Never synced
								{/if}
							</strong>
						</div>
						{#if $syncError && ($syncStatus === 'error' || $syncStatus === 'cancelled')}
							<div class="info-row">
								<span>Error</span>
								<strong class="sync-error">{$syncError}</strong>
							</div>
						{/if}
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
						<button class="btn btn-primary" onclick={() => void syncLibrary()} disabled={$syncStatus === 'syncing'}>
							{#if $syncStatus === 'syncing'}
								Syncing…
							{:else if $syncStatus === 'done'}
								Sync again
							{:else if $syncStatus === 'error' || $syncStatus === 'cancelled'}
								Retry sync
							{:else}
								Sync library
							{/if}
						</button>
						<button class="btn btn-glass" onclick={() => void syncLibrary('full')} disabled={$syncStatus === 'syncing'}>
							Full resync
						</button>
						{#if $syncStatus === 'syncing'}
							<button class="btn btn-glass" onclick={handleCancelSync}>Cancel</button>
						{/if}
						<button class="btn btn-glass" onclick={disconnectTidal}>Disconnect</button>
					</div>
				{/if}
			</section>

			{/if}

			{#if activeCategory === 'sources'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Metadata" title="MusicBrainz enrichment" subtitle="Genre coverage for browsing and discovery." />

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
				<SectionHeader eyebrow="Metadata" title="Spotify tags" subtitle="Unavailable: Premium-only API access." />
				<p class="page-copy">
					The integration code is still in place — if you ever subscribe to Spotify Premium, the existing app credentials should start working again and this panel will reactivate. For now, use Last.fm below as the second genre source.
				</p>
			</section>

			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Metadata" title="Last.fm tags" subtitle="Crowd tags from a local API key." />

				{#if lastfmError}
					<p class="page-copy" style="color: var(--state-error)">{lastfmError}</p>
				{/if}

				{#if !lastfmConfigured}
					<div class="info-list">
						<div class="info-row">
							<span>API Key</span>
							<input
								type="password"
								class="text-input"
								bind:value={lastfmApiKey}
								placeholder="32-character hex string"
								autocomplete="off"
							/>
						</div>
					</div>
					<div class="action-row">
						<button
							class="btn btn-primary"
							onclick={saveLastfmConfig}
							disabled={lastfmSaving || !lastfmApiKey}
						>
							{lastfmSaving ? 'Verifying…' : 'Save API key'}
						</button>
					</div>
				{:else}
					<div class="stat-grid inner-metrics">
						<MetricPair label="Tagged" value={lastfmEnrichedCount.toLocaleString()} copy="Tracks with Last.fm genre or context tags." />
						<MetricPair label="Checked" value={`${lastfmChecked.toLocaleString()} / ${lastfmTotal.toLocaleString()}`} copy="Eligible tracks already queried." />
						<MetricPair
							label="Remaining"
							value={lastfmRemaining.toLocaleString()}
							copy="Favorited tracks still pending Last.fm lookup. Last.fm allows ~5 req/sec, so a full pass takes ~30 min per 10k tracks."
						/>
					</div>

					<div class="enrichment-progress">
						<div class="enrichment-progress-copy">
							<p>
								{#if lastfmIsRunning && lastfmPrefetchTotal > 0 && lastfmPrefetchDone < lastfmPrefetchTotal}
									Pre-fetching artist tags… {lastfmPrefetchDone.toLocaleString()} / {lastfmPrefetchTotal.toLocaleString()} artists. Track pass starts after.
								{:else if lastfmIsRunning && lastfmRunTotal > 0}
									Querying Last.fm… {lastfmRunRemaining.toLocaleString()} tracks left in this run (~{lastfmEtaLabel} left).
								{:else if !lastfmIsRunning && lastfmRemaining === 0 && lastfmTotal > 0}
									All {lastfmTotal.toLocaleString()} eligible tracks checked. Recheck tags to refresh Last.fm coverage.
								{:else if !lastfmIsRunning && lastfmRemaining > 0}
									{lastfmRemaining.toLocaleString()} favorited tracks pending. Full pass ~{lastfmEtaLabel}. Click Enrich to start; runs in the background even if you close this tab.
								{:else}
									No favorited tracks or albums ready for Last.fm enrichment.
								{/if}
							</p>
							<span>
								{#if lastfmIsRunning && lastfmPrefetchTotal > 0 && lastfmPrefetchDone < lastfmPrefetchTotal}
									{Math.round((lastfmPrefetchDone / lastfmPrefetchTotal) * 100)}% artists cached
								{:else if lastfmIsRunning && lastfmRunTotal > 0}
									{lastfmRunProcessed.toLocaleString()} / {lastfmRunTotal.toLocaleString()} this run
								{/if}
							</span>
						</div>
						<div class="enrichment-progress-rail" aria-hidden="true">
							<div
								class="enrichment-progress-fill"
								style={`width: ${
									lastfmIsRunning && lastfmPrefetchTotal > 0 && lastfmPrefetchDone < lastfmPrefetchTotal
										? Math.round((lastfmPrefetchDone / lastfmPrefetchTotal) * 100)
										: lastfmRunTotal > 0
											? Math.round((lastfmRunProcessed / lastfmRunTotal) * 100)
											: 0
								}%`}
							></div>
						</div>
					</div>

					<div class="action-row">
						<button
							class="btn btn-primary"
							onclick={startLastfmEnrichment}
							disabled={lastfmIsRunning || lastfmTotal === 0}
						>
							{lastfmIsRunning
								? 'Running…'
								: lastfmRemaining === 0
									? 'Recheck tags'
									: lastfmChecked > 0
										? 'Resume enrichment'
										: 'Enrich genres'}
						</button>
						{#if lastfmIsRunning}
							<button class="btn btn-glass" onclick={stopLastfmEnrichment}>Stop</button>
						{/if}
						<button class="btn btn-glass" onclick={resetLastfmEnrichment} disabled={lastfmIsRunning}>Reset checked state</button>
						<button class="btn btn-glass" onclick={clearLastfmConfig} disabled={lastfmIsRunning}>Clear API key</button>
					</div>
				{/if}
			</section>
			{/if}


			{#if activeCategory === 'audio'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Output" title="Audio output" subtitle="Audio routing and video playback defaults." />
				{#if $audioSettings.settings}
					{@const s = $audioSettings.settings}
					<div class="info-list">
						{#if isWindows}
							<div class="info-row">
								<span>
									Bit-perfect mode
									{#if bitPerfectActive}<em class="setting-em-reset">&nbsp;active</em>{/if}
								</span>
								<strong>
									<label class="toggle-switch">
										<input
											type="checkbox"
											checked={bitPerfectActive}
											onchange={onBitPerfectToggle}
										/>
										<span class="toggle-slider"></span>
									</label>
								</strong>
							</div>
							<p class="page-copy setting-caption">
								Sets quality to Hi-Res Lossless, takes exclusive control of the output device, and matches the device rate to each track's native rate. Equivalent to enabling the three audiophile toggles below at once. Some DACs misbehave under exclusive mode — turn off if you hear dropouts.
							</p>
						{/if}
						<div class="info-row">
							<span>Quality</span>
							<strong>
								<select
									class="audio-select"
									value={s.quality}
									onchange={onAudioQualityChange}
								>
									{#each AUDIO_QUALITY_OPTIONS as opt (opt.value)}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
							</strong>
						</div>
						<div class="info-row">
							<span>Output device</span>
							<strong>
								<select
									class="audio-select"
									value={s.output_device ?? '__default__'}
									onchange={onAudioDeviceChange}
								>
									<option value="__default__">System default</option>
									{#each audioDevices as d (d.id)}
										<option value={d.id}>
											{d.name}{d.is_default ? ' (default)' : ''}
										</option>
									{/each}
								</select>
							</strong>
						</div>
						{#if isWindows}
							<div class="info-row">
								<span>Exclusive output (WASAPI)</span>
								<strong>
									<label class="toggle-switch">
										<input
											type="checkbox"
											checked={s.exclusive_mode}
											onchange={onAudioExclusiveToggle}
										/>
										<span class="toggle-slider"></span>
									</label>
								</strong>
							</div>
							<p class="page-copy setting-caption">
								Engaged only while audio plays. Releases the device after the
								idle window below so other apps can use it; re-grabs on next play.
								Crossfade and gapless pre-decode are disabled in exclusive mode.
							</p>
							{#if s.exclusive_mode && !$exclusiveStatus.engaged && $exclusiveStatus.failureReason}
								<div
									class="exclusive-failed-banner"
									role="alert"
									style="margin: 0.6rem 0; padding: 0.85rem 1rem; border: 2px solid #ef4444; border-left-width: 6px; background: rgba(239, 68, 68, 0.12); color: #fecaca; border-radius: 6px;"
								>
									<strong style="display: block; margin-bottom: 0.25rem; color: #fff; letter-spacing: 0.02em;">
										Exclusive mode unavailable
									</strong>
									<span class="setting-status-line">
										{$exclusiveStatus.failureReason} Audio is currently routed
										through Windows shared mixing.
									</span>
									<div style="margin-top: 0.6rem; display: flex; gap: 0.5rem;">
										<button
											type="button"
											class="btn btn-primary"
											style="padding: 0.35rem 0.9rem;"
											disabled={retryingExclusive}
											onclick={retryExclusive}
										>
											{retryingExclusive ? 'Retrying…' : 'Retry'}
										</button>
										<button
											type="button"
											class="btn"
											style="padding: 0.35rem 0.9rem;"
											onclick={disableExclusive}
										>
											Disable exclusive
										</button>
									</div>
								</div>
							{/if}
							{#if s.exclusive_mode && $exclusiveStatus.engaged && $exclusiveStatus.transportFormat}
								<div class="info-row">
									<span>Exclusive transport</span>
									<strong>{$exclusiveStatus.transportFormat}</strong>
								</div>
							{/if}
							<div class="info-row">
								<span>Idle release</span>
								<strong>
									<input
										type="range"
										min="5"
										max="120"
										step="5"
										value={s.exclusive_release_grace_secs}
										oninput={onExclusiveGraceChange}
										style="vertical-align: middle; width: 140px;"
									/>
									<span class="setting-numeric">
										{s.exclusive_release_grace_secs}s
									</span>
								</strong>
							</div>
							<p class="page-copy setting-caption">
								Seconds of pause / silence before NOORwave releases the device
								for other apps. Lower = friendlier; higher = sticks around.
							</p>
						{/if}
						<div class="info-row">
							<span>Sample rate follows source</span>
							<strong>
								<label class="toggle-switch">
									<input
										type="checkbox"
										checked={s.sample_rate_follow}
										onchange={onAudioSrFollowToggle}
									/>
									<span class="toggle-slider"></span>
								</label>
							</strong>
						</div>
						<p class="page-copy setting-caption">
							Reconfigures the output device to each track's native rate (44.1 / 48 / 96 / 192 kHz). Recommended with exclusive mode.
						</p>
						<div class="info-row">
							<span>Video quality</span>
							<strong>
								<select
									class="audio-select"
									value={s.video_quality_mode}
									onchange={onVideoQualityModeChange}
								>
									{#each VIDEO_QUALITY_OPTIONS as opt (opt.value)}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
							</strong>
						</div>
						<p class="page-copy setting-caption">
							Max chooses the highest HLS rendition exposed by each video. Auto lets the player adapt to bandwidth.
						</p>
					</div>

					{#if $audioSettings.pendingApply}
						<p class="page-copy setting-caption" style="color: var(--text-secondary)">Output reconfiguring…</p>
					{/if}
					{#if $audioSettings.error}
						<p class="page-copy" style="color: var(--state-error, #f87171)">{$audioSettings.error}</p>
					{/if}
				{:else if $audioSettings.loading}
					<p class="page-copy">Loading audio settings…</p>
				{:else if $audioSettings.error}
					<p class="page-copy" style="color: var(--state-error, #f87171)">{$audioSettings.error}</p>
				{/if}
			</section>
			{/if}

			{#if activeCategory === 'sources'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Transfer" title="Portable snapshot" subtitle="Export/import MusicBrainz enrichment." />

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
			{/if}

			{#if activeCategory === 'sources'}
			<section class="glass-panel section-panel">
				<SectionHeader
					eyebrow="Cleanup"
					title="Clear non-library entries"
					subtitle="Prune tidal_stream tracks that left no trace."
				/>
				<p class="page-copy">
					Last.fm radio recommendations get resolved into <code>tidal_stream</code> rows in the
					tracks table so playback, listen history, and the resolution cache all keep working.
					This action removes any such row that you never played, never favorited, and isn't in
					any queue or playlist.
				</p>
				<p class="page-copy" style="color: var(--state-warning, #f3c969)">
					Cascades to trained data referencing those tracks (embeddings, neighbours, transitions).
					Storage savings are tiny — even 1,000 tracks/month for 10 years is ~20MB. Run for
					tidiness only.
				</p>
				{#if purgeLastDeleted !== null}
					<p class="page-copy">
						Last run deleted <strong>{purgeLastDeleted.toLocaleString()}</strong> orphan track{purgeLastDeleted === 1 ? '' : 's'}.
					</p>
				{/if}
				{#if purgeError}
					<p class="page-copy" style="color: var(--state-error, #f87171)">{purgeError}</p>
				{/if}
				<div class="action-row">
					<button class="btn btn-glass danger" onclick={purgeOrphanTidalStream} disabled={purgeRunning}>
						{purgeRunning ? 'Purging…' : 'Clear non-library entries'}
					</button>
				</div>
			</section>
			{/if}

			{#if activeCategory === 'audio'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Learning" title="Discovery engine" subtitle="Learned radio coverage and training." />

				<div class="discovery-warning glass-panel">
					<h4>⚠ Heads up — this runs hot.</h4>
					<p>
						A retrain pegs every CPU core for 10–30 seconds on a typical library, longer on bigger ones. Your fans will spin up. If you're on a laptop or somewhere thermally constrained, expect heat. Hit <strong>Stop</strong> any time.
					</p>
				</div>

				<details class="discovery-guide glass-panel">
					<summary>How discovery works — when to retrain, what activates a model</summary>
					<div class="guide-body">
						<p>
							The discovery engine learns a similarity space over your library: every track gets a vector, and each track's top neighbours are pre-computed and stored. Radio, automix, "more like this", and the discover page all read those neighbours. Until a trained model is active, those surfaces fall back to a metadata-only heuristic (same artist / genre / decade), which is flat — same handful of tracks every time.
						</p>

						<h5>When to retrain</h5>
						<ul>
							<li><strong>First time</strong> after syncing your library — there's no model yet.</li>
							<li><strong>After a big sync</strong> — new tracks have no neighbours until you retrain.</li>
							<li><strong>After a few weeks of listening</strong> — the model improves with real plays. New transitions teach it which tracks belong together.</li>
							<li><strong>You don't need to retrain often.</strong> Once a week or so when you've added music or listened a lot. Daily is overkill.</li>
						</ul>

						<h5>Incremental refresh vs Full retrain</h5>
						<p>
							<strong>Incremental refresh</strong> reuses the cached audio-proxy features from the last run and only rebuilds the behavioural + similarity stages. Faster — typically 30–50% of a Full Retrain wall-clock. Use this for routine refreshes.
						</p>
						<p>
							<strong>Full retrain</strong> bypasses cached audio-proxy features and recomputes current library tracks from scratch. Use this if you've changed intensity tier, suspect the cache is stale, or it's been a long time since the last clean rebuild.
						</p>
						<p>
							On the very first run there's nothing cached, so both buttons do identical work.
						</p>

						<h5>Intensity tiers</h5>
						<p>
							<strong>Max</strong> (96-dim, 64 neighbours, 8-track context) — best radio quality, slowest. Recommended for libraries under ~10k or overnight runs.
							<br /><strong>Medium</strong> (64-dim, 32 neighbours, 5-track context) — the default. Indistinguishable from Max for most listening; ~50% of the wall-clock.
							<br /><strong>Low</strong> (48-dim, 24 neighbours, 3-track context) — skips the audio-proxy stage entirely. Cold tracks lose their metadata anchor, but the engine stays usable on modest hardware.
						</p>

						<h5>Why a model might not activate</h5>
						<p>
							A run can complete with full coverage but still leave Active model on <strong>Fallback only</strong>. The activation gate scales with how much you've actually listened:
						</p>
						<ul>
							<li><strong>0 plays</strong> — needs ≥ 50% coverage. Cold-start mode.</li>
							<li><strong>1–49 plays</strong> — needs ≥ 70% coverage. Recall@10 isn't reliable on a tiny held-out set, so the gate looks at coverage only.</li>
							<li><strong>50+ plays</strong> — needs ≥ 85% coverage AND ≥ 15% recall@10. Full strict gate.</li>
						</ul>
						<p>
							If you complete a run and the model doesn't activate, you'll usually see Coverage well above the relevant threshold but Active model still says Fallback. That means recall didn't clear — keep listening, retrain again in a week, and the gate will pass naturally.
						</p>
					</div>
				</details>

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

				<div class="engine-block">
					<div class="engine-copy">
						<label class="engine-label" for="discovery-engine-select">Discovery engine</label>
						<p>
							V2 is the recommended default. V1 is optional and only reads existing legacy models.
						</p>
					</div>
					<select
						id="discovery-engine-select"
						class="engine-select"
						bind:value={discoveryEngine}
						disabled={discoveryIsRunning || engineBusy}
						onchange={(event) => void changeDiscoveryEngine((event.currentTarget as HTMLSelectElement).value as DiscoveryEngine)}
					>
						<option value="v2">V2 recommended</option>
						<option value="v1">V1 legacy</option>
					</select>
					<div class="engine-detail glass-tile">
						<strong>{DISCOVERY_ENGINE_PRESETS[discoveryEngine].title}</strong>
						<span>{DISCOVERY_ENGINE_PRESETS[discoveryEngine].tagline}</span>
						<p>{DISCOVERY_ENGINE_PRESETS[discoveryEngine].detail}</p>
					</div>
					{#if !discoveryEngineTrainable}
						<div class="legacy-engine-note">
							V1 is read-only in this build. Switch to V2 to train or refresh discovery.
						</div>
					{/if}
				</div>

				<div class="intensity-block">
					<div class="intensity-header">
						<span class="intensity-eyebrow">Training intensity</span>
						<span class="intensity-tagline">{INTENSITY_PRESETS[discoveryIntensity].tagline}</span>
					</div>
					<div class="safety-profile-row">
						<div>
							<label class="engine-label" for="discovery-safety-profile">CPU safety profile</label>
							<p>{DISCOVERY_SAFETY_PROFILES[discoverySafetyProfile].detail}</p>
						</div>
						<select
							id="discovery-safety-profile"
							class="engine-select"
							bind:value={discoverySafetyProfile}
							disabled={discoveryIsRunning || safetyProfileBusy}
							onchange={(event) => void changeSafetyProfile((event.currentTarget as HTMLSelectElement).value as DiscoveryTrainingSafetyProfile)}
						>
							<option value="laptop_safe">Laptop-safe</option>
							<option value="balanced">Balanced</option>
							<option value="performance">Performance</option>
						</select>
					</div>
					<div class="intensity-grid">
						{#each (['max', 'medium', 'low'] as const) as tier (tier)}
							<button
								type="button"
								class="intensity-option"
								class:selected={discoveryIntensity === tier}
								disabled={discoveryIsRunning || intensityBusy}
								onclick={() => void changeIntensity(tier)}
							>
								<span class="intensity-title">{INTENSITY_PRESETS[tier].title}</span>
								<span class="intensity-detail">{INTENSITY_PRESETS[tier].detail}</span>
							</button>
						{/each}
					</div>
					{#if discoverySafety}
						{@const safety = discoverySafety}
						<div
							class="safety-panel"
							class:safety-safe={safety.recommendation === 'safe'}
							class:safety-moderate={safety.recommendation === 'moderate'}
							class:safety-high={safety.recommendation === 'high_cost'}
						>
							<div class="safety-headline">
								{#if safety.recommendation === 'safe'}
									Safe to run — about {formatDurationSeconds(safety.estimated_seconds)} expected.
								{:else if safety.recommendation === 'moderate'}
									Moderate cost — about {formatDurationSeconds(safety.estimated_seconds)} expected.
								{:else}
									Heavy run — about {formatDurationSeconds(safety.estimated_seconds)} expected. Consider Medium or Low.
								{/if}
							</div>
							<div class="safety-detail">
								<span>{safety.track_count.toLocaleString()} tracks</span>
								<span>·</span>
								<span>~{safety.estimated_ram_mb} MB peak RAM</span>
								<span>·</span>
								<span>{safety.worker_threads} worker{safety.worker_threads === 1 ? '' : 's'}</span>
								<span>·</span>
								<span>{formatDurationSeconds(safety.safety_timeout_seconds)} safety cap</span>
								{#if safety.last_run_seconds !== null}
									<span>·</span>
									<span>last run {formatDurationSeconds(safety.last_run_seconds)}</span>
								{/if}
							</div>
						</div>
					{/if}
					{#if discoveryIsRunning && discoveryStatus?.latest_run}
						{@const run = discoveryStatus.latest_run}
						{@const pct = Math.round((run.progress ?? 0) * 100)}
						<div class="discovery-progress">
							<div class="discovery-bar-track">
								<div class="discovery-bar-fill" style:width="{pct}%"></div>
							</div>
							<div class="discovery-stage">
								{stageLabel(run.stage)} <span class="discovery-pct">{pct}%</span>
							</div>
						</div>
					{/if}
				</div>

				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void startDiscoveryTraining('incremental')} disabled={discoveryIsRunning || !discoveryEngineTrainable}>Incremental refresh</button>
					<button class="btn btn-glass" onclick={() => void startDiscoveryTraining('full', true)} disabled={discoveryIsRunning || !discoveryEngineTrainable}>Full retrain</button>
					{#if discoveryIsRunning}
						<button class="btn btn-glass" onclick={() => void stopDiscoveryTraining()}>Stop</button>
					{/if}
				</div>
			</section>
			{/if}
		</div>

		<div class="settings-side">
			{#if activeCategory === 'audio'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Playback" title="Audio runtime" subtitle="Current device and format." />
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
			{/if}

			{#if activeCategory === 'sources'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Later" title="Additional services" subtitle="Planned source coverage." />
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
			{/if}

			{#if activeCategory === 'audio'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="DSP" title="Audio analysis" subtitle="BPM, key, energy, danceability." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Analyzed" value={$audioAnalysis.analyzed.toLocaleString()} copy="Tracks with DSP features." />
					<MetricPair label="Avg BPM" value={$audioAnalysis.stats?.avg_bpm?.toFixed(1) ?? '—'} copy="Average tempo across analyzed tracks." />
					<MetricPair label="Top Key" value={$audioAnalysis.stats?.top_key ?? '—'} copy="Most common key signature." />
					<MetricPair label="Avg Energy" value={$audioAnalysis.stats?.avg_energy?.toFixed(2) ?? '—'} copy="Average energy level (0–1)." />
				</div>

				<p class="analysis-note">
					Tracks analyse automatically as you play them. The first 30 seconds of audio
					from the live playback stream is captured into memory, run through the
					BPM / key / energy detector, and saved alongside the track — no extra
					network requests, no separate download.
				</p>
				<p class="analysis-note">
					This means the data fills in over time as you listen rather than all at
					once. There's no bulk-scan button: scanning the entire library would
					require thousands of TIDAL preview requests in quick succession, which
					trips TIDAL's rate limiter and breaks playback for the whole account
					until the backoff clears.
				</p>

				<div class="info-row">
					<div>
						<span>Passive analysis</span>
						<p class="info-row-hint">
							{$audioAnalysis.passiveEnabled
								? 'New tracks you play will be analysed and added to the library DSP table.'
								: 'Disabled — playback is not being analysed. Existing data stays untouched.'}
						</p>
					</div>
					<strong>
						<label class="toggle-switch">
							<input
								type="checkbox"
								checked={$audioAnalysis.passiveEnabled}
								onchange={(e) => void setPassiveDspEnabled((e.currentTarget as HTMLInputElement).checked)}
							/>
							<span class="toggle-slider"></span>
						</label>
					</strong>
				</div>

				<div class="action-row">
					<button class="btn btn-glass danger" onclick={clearAllAnalysis}>Clear All</button>
				</div>

				<details class="advanced-details">
					<summary>Advanced Settings</summary>
					<div class="setting-row">
						<label for="dsp-max-duration">Max duration per track (seconds)</label>
						<input id="dsp-max-duration" type="number" value="30" min="10" max="120" />
					</div>
					<div class="setting-row">
						<label for="dsp-reanalyze-interval">Re-analyze interval (days)</label>
						<input id="dsp-reanalyze-interval" type="number" value="30" min="1" max="365" />
					</div>
				</details>
			</section>
			{/if}

			{#if activeCategory === 'sources'}
			<section class="glass-panel section-panel">
				<SectionHeader eyebrow="Recognition" title="ACRCloud" subtitle="Sample and cover detection." />

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
			{/if}
		</div>
	</section>
</div>

<style>
	/* Caption + status helpers — extracted from template inline styles */
	.setting-caption {
		font-size: var(--font-size-sm);
	}
	.setting-em-reset {
		font-style: normal;
		opacity: 0.6;
		font-size: var(--font-size-xs);
	}
	.setting-status-line {
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}
	.setting-numeric {
		margin-left: 0.5rem;
		font-variant-numeric: tabular-nums;
	}

	.engine-block {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(16rem, 100%), 1fr));
		gap: var(--gap);
		align-items: start;
		padding: var(--space-4);
		margin-bottom: var(--space-3);
		border-radius: var(--radius-md);
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
	}

	.engine-copy {
		display: grid;
		gap: var(--space-2);
	}

	.engine-label {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-tight);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
	}

	.engine-copy p,
	.engine-detail p {
		margin: 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	.engine-select {
		width: 100%;
		padding: var(--space-3) var(--space-4);
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-muted);
		background: var(--bg-elevated);
		color: var(--text-primary);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}

	.engine-select:disabled {
		cursor: not-allowed;
		opacity: 0.6;
	}

	.engine-detail {
		grid-column: 1 / -1;
		display: grid;
		gap: var(--space-2);
		padding: var(--space-3);
	}

	.engine-detail strong {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
	}

	.engine-detail span,
	.legacy-engine-note {
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	.legacy-engine-note {
		grid-column: 1 / -1;
		padding: var(--space-3);
		border-left: 3px solid var(--state-warning);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-warning) 12%, transparent);
	}

	.safety-profile-row {
		display: grid;
		grid-template-columns: repeat(auto-fit, minmax(min(16rem, 100%), 1fr));
		gap: var(--gap);
		align-items: start;
	}

	.safety-profile-row p {
		margin: var(--space-2) 0 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	.safety-watchdog-popup {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: var(--gap);
		margin-bottom: var(--space-4);
		padding: var(--space-4);
		border-left: 3px solid var(--state-warning);
	}

	.safety-watchdog-popup strong {
		display: block;
		margin-bottom: var(--space-2);
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		line-height: var(--line-height-snug);
	}

	.safety-watchdog-popup p {
		margin: 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	/* Discovery intensity selector + safety preview */
	.intensity-block {
		display: grid;
		gap: 14px;
		padding: 16px;
		margin-bottom: 12px;
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.02);
		border: 1px solid var(--border-subtle);
	}

	.intensity-header {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: 12px;
	}

	.intensity-eyebrow {
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
	}

	.intensity-tagline {
		font-size: var(--font-size-sm);
		color: var(--text-tertiary, var(--text-secondary));
	}

	.intensity-grid {
		display: grid;
		grid-template-columns: repeat(3, 1fr);
		gap: 10px;
	}

	.intensity-option {
		display: grid;
		gap: 8px;
		text-align: left;
		padding: 14px;
		border-radius: 8px;
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid rgba(255, 255, 255, 0.07);
		color: inherit;
		cursor: pointer;
		transition: background 0.15s ease, border-color 0.15s ease;
	}

	.intensity-option:hover:not(:disabled) {
		background: rgba(255, 255, 255, 0.05);
		border-color: rgba(255, 255, 255, 0.14);
	}

	.intensity-option:disabled {
		cursor: not-allowed;
		opacity: 0.55;
	}

	.intensity-option.selected {
		background: rgba(110, 168, 255, 0.10);
		border-color: rgba(110, 168, 255, 0.45);
	}

	.intensity-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-md);
	}

	.intensity-detail {
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	.safety-panel {
		display: grid;
		gap: 4px;
		padding: 10px 12px;
		border-radius: 8px;
		border-left: 3px solid;
		font-size: var(--font-size-sm);
	}

	.safety-panel.safety-safe {
		background: rgba(75, 200, 130, 0.07);
		border-left-color: rgba(75, 200, 130, 0.6);
	}

	.safety-panel.safety-moderate {
		background: rgba(250, 200, 90, 0.07);
		border-left-color: rgba(250, 200, 90, 0.6);
	}

	.safety-panel.safety-high {
		background: rgba(240, 110, 90, 0.07);
		border-left-color: rgba(240, 110, 90, 0.6);
	}

	.safety-headline {
		font-weight: var(--font-weight-medium);
	}

	.safety-detail {
		display: flex;
		flex-wrap: wrap;
		gap: 6px;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.discovery-progress {
		display: flex;
		flex-direction: column;
		gap: 6px;
	}

	.discovery-bar-track {
		height: 4px;
		border-radius: 2px;
		background: var(--surface-raised, rgba(255, 255, 255, 0.08));
		overflow: hidden;
	}

	.discovery-bar-fill {
		height: 100%;
		border-radius: 2px;
		background: var(--accent, var(--color-accent));
		transition: width 0.6s ease;
	}

	.discovery-stage {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}

	.discovery-pct {
		opacity: 0.6;
	}

	@media (max-width: 720px) {
		.intensity-grid {
			grid-template-columns: 1fr;
		}
	}

	.settings-page {
		gap: 14px;
	}

	.settings-command {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 18px;
		padding: 4px 0 2px;
	}

	.settings-title {
		display: grid;
		gap: 5px;
		min-width: 0;
	}

	.settings-title h1 {
		font-family: var(--font-body);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-bold);
		line-height: var(--line-height-tight);
		letter-spacing: 0;
	}

	.settings-title p:not(.eyebrow) {
		color: var(--text-secondary);
		max-width: 64ch;
	}

	.settings-status {
		display: flex;
		justify-content: flex-end;
		flex-wrap: wrap;
		gap: 8px;
	}

	.settings-status-strip {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 1px;
		overflow: hidden;
		border: 1px solid var(--border-subtle);
		border-radius: 12px;
		background: var(--border-subtle);
	}

	.settings-status-strip div {
		min-width: 0;
		display: grid;
		gap: 3px;
		padding: 10px 12px;
		background: color-mix(in srgb, var(--bg-elevated) 72%, transparent);
	}

	.settings-status-strip span {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.09em;
		text-transform: uppercase;
	}

	.settings-status-strip strong {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
		font-size: var(--font-size-sm);
	}

	.settings-grid {
		display: grid;
		grid-template-columns: minmax(0, 1.35fr) minmax(300px, 0.65fr);
		gap: var(--space-4);
		align-items: start;
	}

	.settings-grid.single-column {
		grid-template-columns: minmax(0, 1fr);
	}

	.settings-main,
	.settings-side {
		display: grid;
		gap: 12px;
	}

	.settings-main:empty,
	.settings-side:empty {
		display: none;
	}

	.settings-rail {
		display: grid;
		grid-template-columns: repeat(4, minmax(0, 1fr));
		gap: 6px;
		padding: 6px;
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-md);
		background: rgba(255, 255, 255, 0.022);
	}

	.settings-rail-btn {
		all: unset;
		display: flex;
		align-items: center;
		gap: 9px;
		min-width: 0;
		min-height: 46px;
		padding: 8px 12px;
		border-radius: 9px;
		border: 1px solid transparent;
		background: transparent;
		cursor: pointer;
		transition: background 180ms ease, border-color 180ms ease, transform 180ms ease;
	}

	.settings-rail-btn:hover {
		background: rgba(255, 255, 255, 0.05);
		border-color: rgba(255, 255, 255, 0.12);
	}

	.settings-rail-btn.active {
		background: color-mix(in srgb, var(--accent-strong, #6366f1) 12%, transparent);
		border-color: color-mix(in srgb, var(--accent-strong, #6366f1) 36%, transparent);
	}

	.settings-rail-icon {
		flex: 0 0 auto;
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border-radius: 7px;
		background: rgba(255, 255, 255, 0.04);
		color: rgba(255, 255, 255, 0.82);
	}

	.settings-rail-icon svg {
		width: 17px;
		height: 17px;
		fill: none;
		stroke: currentColor;
		stroke-width: 1.8;
		stroke-linecap: round;
		stroke-linejoin: round;
	}

	.settings-rail-btn.active .settings-rail-icon {
		background: color-mix(in srgb, var(--accent-strong, #6366f1) 25%, transparent);
		color: var(--text-primary);
	}

	.settings-rail-copy {
		display: flex;
		flex-direction: column;
		min-width: 0;
		max-width: 22ch;
	}

	.settings-rail-copy strong {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.settings-rail-hint {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		white-space: normal;
		display: -webkit-box;
		line-clamp: 1;
		-webkit-line-clamp: 1;
		-webkit-box-orient: vertical;
		overflow: hidden;
	}

	.palette-row {
		display: flex;
		align-items: center;
		gap: 14px;
		flex-wrap: wrap;
	}

	.palette-select {
		flex: 1;
		min-width: 220px;
	}

	.palette-swatches {
		display: flex;
		gap: 6px;
	}

	.palette-swatch {
		width: 22px;
		height: 22px;
		border-radius: 999px;
		border: 1px solid rgba(255, 255, 255, 0.18);
		box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.3) inset;
	}

	.zoom-row {
		display: flex;
		align-items: center;
		gap: var(--gap);
		flex-wrap: wrap;
	}

	.zoom-step {
		min-width: 36px;
		font-variant-numeric: tabular-nums;
	}

	.zoom-slider {
		flex: 1;
		min-width: 200px;
		accent-color: var(--accent);
	}

	.zoom-readout {
		min-width: 4ch;
		text-align: right;
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	/* ── Shared preview panel ── */
	.wallpaper-big-preview {
		position: relative;
		aspect-ratio: 16 / 5;
		max-height: 260px;
		border-radius: 10px;
		overflow: hidden;
		background: #08080c;
		border: 1px solid rgba(255, 255, 255, 0.07);
	}

	.wallpaper-big-preview-hint {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.wallpaper-big-preview-hint span {
		font-size: var(--font-size-xs);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: rgba(255, 255, 255, 0.20);
	}

	/* ── Tile grid ── */
	.wallpaper-grid {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(140px, 1fr));
		gap: 8px;
	}

	.wallpaper-tile {
		all: unset;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 9px 10px;
		border-radius: 10px;
		border: 1px solid var(--border-subtle);
		background: rgba(255, 255, 255, 0.02);
		cursor: pointer;
		transition: border-color 140ms ease, background 140ms ease, box-shadow 140ms ease;
	}

	.wallpaper-tile:hover,
	.wallpaper-tile.previewing {
		border-color: rgba(255, 255, 255, 0.18);
		background: rgba(255, 255, 255, 0.05);
	}

	.wallpaper-tile.active {
		border-color: color-mix(in srgb, var(--accent-strong, #6366f1) 70%, transparent);
		background: color-mix(in srgb, var(--accent-strong, #6366f1) 12%, transparent);
		box-shadow: 0 0 0 1px color-mix(in srgb, var(--accent, #6366f1) 30%, transparent) inset;
	}

	.wallpaper-tile-swatch {
		flex-shrink: 0;
		width: 28px;
		height: 28px;
		border-radius: 6px;
		background: linear-gradient(135deg,
			color-mix(in srgb, var(--accent, #7c80ff) 60%, #0a0a14),
			color-mix(in srgb, var(--accent, #7c80ff) 20%, #050508));
		border: 1px solid var(--panel-border);
	}

	.wallpaper-tile-swatch-none {
		background:
			radial-gradient(circle at 30% 30%, rgba(151, 126, 255, 0.35), transparent 60%),
			radial-gradient(circle at 70% 70%, rgba(120, 160, 255, 0.25), transparent 60%),
			#0a0a0e;
	}

	.wallpaper-tile-label {
		display: flex;
		flex-direction: column;
		gap: 3px;
		min-width: 0;
	}

	.wallpaper-tile-label strong {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.wallpaper-active-badge {
		display: inline-block;
		padding: 1px 6px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--accent-strong, #6366f1) 90%, transparent);
		color: white;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.05em;
		text-transform: uppercase;
		width: fit-content;
	}

	.wallpaper-tile-swatch-mono {
		background: linear-gradient(160deg, #e8e8ea 0%, #1a1a1c 55%, #0a0a0b 100%);
	}

	.wallpaper-more-btn {
		all: unset;
		display: inline-flex;
		align-items: center;
		gap: 6px;
		cursor: pointer;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		color: var(--text-secondary, rgba(255,255,255,0.45));
		padding: 5px 2px;
		transition: color 140ms ease;
		align-self: flex-start;
	}

	.wallpaper-more-btn:hover {
		color: var(--text-primary, rgba(255,255,255,0.85));
	}

	.wallpaper-more-icon {
		display: inline-block;
		font-size: var(--font-size-2xs);
		transition: transform 180ms ease;
	}

	.wallpaper-more-icon.open {
		transform: rotate(90deg);
	}

	.wallpaper-grid-extended {
		border-top: 1px solid rgba(255,255,255,0.05);
		padding-top: 4px;
	}

	.section-panel {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 14px;
		border-radius: var(--radius-md);
	}

	.settings-grid.single-column .settings-main {
		max-width: none;
	}

	.settings-grid.single-column .section-panel {
		max-width: none;
	}

	.inner-metrics {
		grid-template-columns: repeat(2, minmax(0, 1fr));
	}

	.inner-metrics :global(.metric-pair) {
		padding: 12px;
		border-radius: 10px;
	}

	.inner-metrics :global(.metric-pair strong) {
		font-family: var(--font-body);
		font-size: var(--font-size-md);
		letter-spacing: 0;
	}

	.inner-metrics :global(.metric-pair p) {
		font-size: var(--font-size-xs);
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
		font-size: var(--font-size-md);
		color: rgba(255, 255, 255, 0.92);
	}

	.enrichment-progress-copy span {
		font-size: var(--font-size-sm);
		color: rgba(255, 255, 255, 0.62);
	}

	.enrichment-progress-rail {
		position: relative;
		height: 10px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		border: 1px solid var(--panel-border);
		overflow: hidden;
	}

	.enrichment-progress-fill {
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, rgba(151, 126, 255, 0.85), rgba(120, 160, 255, 0.72));
		transition: width 200ms ease;
	}

	.discovery-warning {
		border-left: 4px solid rgba(220, 70, 70, 0.6);
		padding: 1rem 1.25rem;
		margin-bottom: 1.25rem;
	}

	.discovery-warning h4 {
		font-size: var(--font-size-md);
		font-weight: var(--font-weight-semibold);
		margin: 0 0 0.4rem;
	}

	.discovery-warning p {
		margin: 0;
		line-height: var(--line-height-normal);
	}

	.discovery-guide {
		padding: 14px 18px;
		margin-top: 12px;
	}

	.discovery-guide > summary {
		cursor: pointer;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-secondary);
		list-style: none;
	}

	.discovery-guide > summary::marker,
	.discovery-guide > summary::-webkit-details-marker {
		display: none;
	}

	.discovery-guide > summary::before {
		content: '▸ ';
		display: inline-block;
		transition: transform 0.15s ease;
		margin-right: 4px;
	}

	.discovery-guide[open] > summary::before {
		transform: rotate(90deg);
	}

	.guide-body {
		margin-top: 14px;
		display: flex;
		flex-direction: column;
		gap: 10px;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-loose);
		color: var(--text-secondary);
	}

	.guide-body h5 {
		margin: 8px 0 0;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}

	.guide-body ul {
		margin: 0;
		padding-left: 18px;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.guide-body p {
		margin: 0;
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

	.text-field {
		width: 100%;
		padding: 10px 12px;
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.04);
		color: var(--text-primary);
		font: inherit;
	}

	.path-value {
		word-break: break-all;
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
		font-size: var(--font-size-sm);
		color: var(--signal-text);
	}

	.roadmap-item {
		padding: 14px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.03);
		border: 1px solid var(--border-subtle);
	}

	.roadmap-item p {
		color: var(--text-secondary);
		margin-top: 6px;
	}

	@media (max-width: 960px) {
		.settings-command {
			flex-direction: column;
		}

		.settings-status {
			justify-content: flex-start;
		}

		.settings-status-strip {
			grid-template-columns: repeat(2, minmax(0, 1fr));
		}

		.settings-grid {
			grid-template-columns: 1fr;
		}

		.settings-rail {
			grid-template-columns: repeat(3, minmax(0, 1fr));
		}
	}

	@media (max-width: 640px) {
		.settings-status-strip,
		.settings-rail {
			grid-template-columns: 1fr;
		}

		.settings-rail-btn {
			padding: 10px 11px;
		}

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
		border-radius: 999px;
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
		font-size: var(--font-size-sm);
		color: rgba(255, 255, 255, 0.5);
		font-weight: normal;
	}
	.sync-error {
		color: var(--state-error);
		font-weight: var(--font-weight-medium);
		word-break: break-word;
	}


	/* Audio analysis progress bar */
	.progress-bar {
		position: relative;
		height: 10px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.08);
		border: 1px solid var(--panel-border);
		overflow: hidden;
	}

	.progress-fill {
		height: 100%;
		border-radius: inherit;
		background: linear-gradient(90deg, rgba(151, 126, 255, 0.85), rgba(120, 160, 255, 0.72));
		transition: width 200ms ease;
	}

	.analysis-progress-label {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		margin: 6px 0 0;
	}

	.analysis-note {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		line-height: var(--line-height-normal);
		margin: var(--space-3) 0;
		max-width: 60ch;
	}

	.info-row-hint {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, var(--text-secondary));
		margin: 4px 0 0;
		line-height: var(--line-height-snug);
		max-width: 60ch;
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
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
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
		font-size: var(--font-size-sm);
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
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
	}

	.token-row {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-wrap: wrap;
	}

	.token-value {
		flex: 1;
		min-width: 0;
		padding: 8px 12px;
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid var(--panel-border);
		font-family: var(--font-mono);
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		word-break: break-all;
		white-space: pre-wrap;
	}

	.field-error {
		font-size: var(--font-size-sm);
		color: #ffb0b0;
	}
</style>
