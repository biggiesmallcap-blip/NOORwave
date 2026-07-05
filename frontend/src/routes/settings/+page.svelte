<script lang="ts">
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import type { Unsubscriber } from 'svelte/store';
	import { showToast } from '$lib/stores/toast';
	import {
		api,
		getApiBase,
		authFetch,
		getStoredToken,
		setStoredToken,
		type AudioDevice,
		type AudioQuality,
		type ExclusiveLatencyMode,
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
	import SectionHeader from '$lib/components/ui/SectionHeader.svelte';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import EmptyState from '$lib/components/ui/EmptyState.svelte';
	import MetricPair from '$lib/components/ui/MetricPair.svelte';
	import Toggle from '$lib/components/ui/Toggle.svelte';
	import { searchSettings, type SettingsSearchEntry } from '$lib/components/settings/settingsSearch';
	import IntegrationsPanel from '$lib/components/settings/IntegrationsPanel.svelte';
	import {
		discoveryLastTrainedAt,
		shouldContinueDiscoveryCompletionRefresh,
		shouldRefreshAfterTerminalDiscoveryProgress
	} from '$lib/components/settings/discovery_status';
	import ShaderWallpaper from '$lib/components/wallpaper/ShaderWallpaper.svelte';
	import { WALLPAPERS, WALLPAPER_GROUPS, type WallpaperOption } from '$lib/components/wallpaper/shaders';
	import {
		wallpaper,
		wallpaperBlur,
		wallpaperFps,
		wallpaperReactive,
		wallpaperReactivity,
		wallpaperBeatSmoothing,
		wallpaperReduceMotion,
		wallpaperColorSource,
		wallpaperQuality,
		wallpaperIdle,
		setWallpaper,
		setWallpaperBlur,
		setWallpaperFps,
		setWallpaperReactive,
		setWallpaperReactivity,
		setWallpaperBeatSmoothing,
		setWallpaperReduceMotion,
		setWallpaperColorSource,
		setWallpaperQuality,
		setWallpaperIdle,
		WALLPAPER_BLUR_MAX,
		WALLPAPER_BLUR_MIN,
		WALLPAPER_FPS_MAX,
		WALLPAPER_FPS_MIN,
		WALLPAPER_REACTIVITY_MAX,
		WALLPAPER_REACTIVITY_MIN,
		WALLPAPER_SMOOTHING_MAX,
		WALLPAPER_SMOOTHING_MIN,
		type WallpaperReduceMotion,
		type WallpaperColorSource,
		type WallpaperQuality,
		type WallpaperIdle
	} from '$lib/stores/wallpaper';
	import { PALETTES, rgbCss, type Palette, type PaletteId } from '$lib/components/wallpaper/palettes';
	import { artPalette, artPaletteStatus } from '$lib/stores/artPalette';
	import { currentTrack } from '$lib/stores/player';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';
	import { palette, setPalette } from '$lib/stores/palette';
	import { uiZoom, setZoom, zoomIn, zoomOut, resetZoom, MIN as ZOOM_MIN, MAX as ZOOM_MAX, WHEEL_STEP as ZOOM_STEP } from '$lib/stores/uiZoom';
	import { audioSettings } from '$lib/stores/audio_settings';
	import { exclusiveStatus } from '$lib/stores/exclusive_status';
	import {
		defaultDownloadFormat,
		defaultFlacQuality,
		defaultMp3Source,
		loadDownloadSettings,
		saveDownloadSettings,
		type DownloadFormat,
		type FlacQuality,
		type Mp3Source
	} from '$lib/stores/downloads';
	import { open as openDirectoryDialog } from '@tauri-apps/plugin-dialog';
	import { isTauri, openExternal } from '$lib/util/external';
	import { isValidTidalRedirectUrl, readTidalRedirectFromClipboard } from '$lib/tidal/login';
	import { cachedApi } from '$lib/cache/api_queries';
	import { dataCache } from '$lib/cache/query';
	import {
		browserUpdateState,
		loadingDesktopUpdateState,
		unavailableDesktopUpdateState
	} from '$lib/desktop/update_state';

	const SERVER_UNREACHABLE_MESSAGE =
		'NOOR cannot reach the local server, so it cannot verify your current TIDAL session.';
	const APP_VERSION = String(import.meta.env.NOOR_APP_VERSION ?? '0.0.0');
	const DISCOVERY_COMPLETION_REFRESH_DELAY_MS = 1000;
	const DISCOVERY_COMPLETION_REFRESH_MAX_ATTEMPTS = 12;
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
	let desktopAppAvailable = $state(false);
	let appVersion = $state(APP_VERSION);
	let installModeLabel = $state('Browser');
	let updateStatus = $state('Available in the desktop app');
	let updateAvailableVersion = $state<string | null>(null);
	let updateChecking = $state(false);
	let minimizeToTray = $state(false);
	let updateError = $state('');

	let mbStatus = $state<'idle' | 'running' | 'done'>('idle');
	let mbLiveProgress = $state<number | null>(null);
	let mbProgressLabel = $state('');
	let mbStats = $state<MusicBrainzStatus | null>(null);
	let portableSnapshot = $state<PortableMusicBrainzSnapshotStatus | null>(null);
	let discoveryStatus = $state<DiscoveryStatus | null>(null);
	let discoveryStatusLastTrainedAt = $derived(discoveryLastTrainedAt(discoveryStatus));
	let portableAction = $state<'export' | 'import' | null>(null);
	let portableStatusLabel = $state('');
	let galaxyRefreshLabel = $state('');

	let radioSimilarityRowCount = $state<number | null>(null);
	let radioSimilarityBuiltAt = $state<string | null>(null);
	let radioSimilarityBusy = $state(false);
	let radioSimilarityLabel = $state('');
	// Set on unmount so the build poll loop can't outlive the component.
	let componentUnmounted = false;

	let lastfmConfigured = $state(false);
	let lastfmApiKey = $state('');
	let lastfmSaving = $state(false);
	let lastfmError = $state('');
	let lastfmTotal = $state(0);
	let lastfmChecked = $state(0);
	let lastfmEnrichedCount = $state(0);
	let lastfmRemaining = $state(0);
	let lastfmCheckedUntagged = $derived(Math.max(0, lastfmChecked - lastfmEnrichedCount));
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
			dataCache.clear();
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

	async function refreshGalaxy() {
		galaxyRefreshLabel = 'Refreshing genre data…';
		try {
			const genres = await cachedApi.getGenres();
			const heat = await cachedApi.getGenreHeat(90);
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

	async function loadDesktopAppInfo() {
		desktopAppAvailable = isTauri();
		if (!desktopAppAvailable) {
			const browserState = browserUpdateState(APP_VERSION);
			appVersion = browserState.appVersion;
			installModeLabel = browserState.installModeLabel;
			updateStatus = browserState.updateStatus;
			updateAvailableVersion = browserState.updateAvailableVersion;
			updateError = browserState.updateError;
			return;
		}

		const loadingState = loadingDesktopUpdateState(appVersion);
		installModeLabel = loadingState.installModeLabel;
		updateStatus = loadingState.updateStatus;
		updateError = loadingState.updateError;

		try {
			const [{ getVersion }, { invoke }] = await Promise.all([
				import('@tauri-apps/api/app'),
				import('@tauri-apps/api/core'),
			]);
			appVersion = await getVersion();
			installModeLabel = await invoke<string>('get_install_mode');
			const pending = await invoke<string | null>('get_update_state');
			updateAvailableVersion = pending;
			updateStatus = pending ? `v${pending} available` : 'Up to date';
			minimizeToTray = await invoke<boolean>('get_minimize_to_tray');
		} catch (err) {
			const unavailableState = unavailableDesktopUpdateState(appVersion, err);
			installModeLabel = unavailableState.installModeLabel;
			updateStatus = unavailableState.updateStatus;
			updateAvailableVersion = unavailableState.updateAvailableVersion;
			updateError = unavailableState.updateError;
		}
	}

	async function setMinimizeToTray(next: boolean) {
		const prev = minimizeToTray;
		minimizeToTray = next;
		try {
			const { invoke } = await import('@tauri-apps/api/core');
			await invoke('set_minimize_to_tray', { value: next });
		} catch {
			minimizeToTray = prev;
			showToast('Could not save the close behavior. Try again.', 'error');
		}
	}

	async function setupDesktopUpdateListeners(unlisteners: Array<() => void>) {
		if (!isTauri()) return;
		try {
			const { listen } = await import('@tauri-apps/api/event');
			const unlistenAvailable = await listen<string>('update-available', (event) => {
				updateAvailableVersion = event.payload;
				updateStatus = `v${event.payload} available`;
				updateError = '';
			});
			if (componentUnmounted) {
				unlistenAvailable();
				return;
			}
			unlisteners.push(unlistenAvailable);

			const unlistenError = await listen<string>('update-error', (event) => {
				updateError = event.payload;
				updateStatus = 'Update check failed';
			});
			if (componentUnmounted) {
				unlistenError();
				return;
			}
			unlisteners.push(unlistenError);
		} catch (err) {
			updateError = err instanceof Error ? err.message : String(err);
		}
	}

	async function checkForUpdatesNow() {
		updateError = '';
		if (!desktopAppAvailable) {
			updateStatus = 'Available in the desktop app';
			return;
		}

		updateChecking = true;
		try {
			const { invoke } = await import('@tauri-apps/api/core');
			const version = await invoke<string | null>('check_for_updates_now');
			updateAvailableVersion = version;
			updateStatus = version ? `v${version} available` : 'Up to date';
		} catch (err) {
			updateError = err instanceof Error ? err.message : String(err);
			updateStatus = 'Update check failed';
		} finally {
			updateChecking = false;
		}
	}

	let downloadFolder = $state('');
	let downloadFolderSaving = $state(false);

	async function refreshDownloadFolder() {
		const settings = await loadDownloadSettings();
		if (settings) downloadFolder = settings.folder;
	}

	async function chooseDownloadFolder() {
		if (!isTauri()) return;
		try {
			const picked = await openDirectoryDialog({ directory: true, multiple: false });
			if (typeof picked === 'string' && picked) {
				downloadFolderSaving = true;
				const updated = await saveDownloadSettings({ folder: picked });
				if (updated) downloadFolder = updated.folder;
				downloadFolderSaving = false;
			}
		} catch (error) {
			console.warn('download folder pick failed', error);
			downloadFolderSaving = false;
		}
	}

	async function commitDownloadFolder(value: string) {
		const updated = await saveDownloadSettings({ folder: value });
		if (updated) downloadFolder = updated.folder;
	}

	function setDownloadFormat(format: DownloadFormat) {
		void saveDownloadSettings({ format });
	}

	function setFlacQuality(flac_quality: FlacQuality) {
		void saveDownloadSettings({ flac_quality });
	}

	function setMp3Source(mp3_source: Mp3Source) {
		void saveDownloadSettings({ mp3_source });
	}

	onMount(() => {
		const tauriUnlisteners: Array<() => void> = [];
		void refreshDownloadFolder();
		const tick = setInterval(() => {
			nowEpochSeconds = Math.floor(Date.now() / 1000);
		}, 1000);
		const discoveryTrainingPoll = setInterval(() => {
			if (discoveryIsRunning) void loadDiscoveryStatus();
		}, 3000);
		let discoveryCompletionRefreshTimer: ReturnType<typeof setTimeout> | null = null;
		let discoveryCompletionRefreshAttempts = 0;
		const clearDiscoveryCompletionRefresh = () => {
			if (discoveryCompletionRefreshTimer) clearTimeout(discoveryCompletionRefreshTimer);
			discoveryCompletionRefreshTimer = null;
		};
		const scheduleDiscoveryCompletionRefresh = () => {
			clearDiscoveryCompletionRefresh();
			discoveryCompletionRefreshAttempts = 0;
			const refreshUntilFinished = async () => {
				discoveryCompletionRefreshTimer = null;
				discoveryCompletionRefreshAttempts += 1;
				await loadDiscoveryStatus();
				if (componentUnmounted) return;
				if (
					!shouldContinueDiscoveryCompletionRefresh(
						discoveryStatus,
						discoveryCompletionRefreshAttempts,
						DISCOVERY_COMPLETION_REFRESH_MAX_ATTEMPTS
					)
				) return;
				discoveryCompletionRefreshTimer = setTimeout(
					refreshUntilFinished,
					DISCOVERY_COMPLETION_REFRESH_DELAY_MS
				);
			};
			discoveryCompletionRefreshTimer = setTimeout(
				refreshUntilFinished,
				DISCOVERY_COMPLETION_REFRESH_DELAY_MS
			);
		};
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
				void loadLastfmStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'musicbrainz') {
				mbStatus = 'running';
				mbLiveProgress = typeof latest.progress === 'number' ? latest.progress : mbLiveProgress;
				void loadMbStatus();
			}

			if (latest.type === 'sync_progress' && latest.service === 'lastfm') {
				void loadLastfmStatus();
			}

			if (latest.type === 'musicbrainz_enriched' && lastfmIsRunning) {
				void loadLastfmStatus();
			}

			if (latest.type === 'training_progress') {
				if (discoveryStatus?.latest_run) {
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
				if (shouldRefreshAfterTerminalDiscoveryProgress(latest)) scheduleDiscoveryCompletionRefresh();
			}

			if (
				latest.type === 'playback_changed' ||
				latest.type === 'track_changed' ||
				latest.type === 'playback_failed'
			) {
				void loadPlaybackRuntime();
			}

			if (latest.type === 'radio_similarity_computed') {
				void loadRadioSimilarityStatus();
				if (!radioSimilarityBusy) {
					const pairs = typeof latest.pairs === 'number' ? latest.pairs : null;
					radioSimilarityLabel = pairs
						? `Index rebuilt automatically: ${pairs.toLocaleString()} pairs.`
						: 'Radio similarity index rebuilt automatically.';
				}
			}
		});

		void refreshTidalStatus();
		void loadSyncInfo();
		void loadVisibleSettingsCategory();
		const cancelBackgroundSettingsLoad = scheduleSettingsBackgroundLoad();
		void loadDesktopAppInfo();
		void setupDesktopUpdateListeners(tauriUnlisteners);
		serverToken = getStoredToken() ?? '';
		return () => {
			if (mbPollTimer) clearInterval(mbPollTimer);
			clearDiscoveryCompletionRefresh();
			cancelBackgroundSettingsLoad();
			clearInterval(discoveryTrainingPoll);
			clearInterval(tick);
			wsUnsubscribe?.();
			for (const unlisten of tauriUnlisteners) unlisten();
			componentUnmounted = true;
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

	async function startLastfmEnrichment(mode: '' | 'retry_untagged' | 'refresh' = '') {
		lastfmError = '';
		try {
			const path = `/api/library/enrich/lastfm${mode ? `?mode=${mode}` : ''}`;
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

	function startLastfmPrimaryEnrichment() {
		if (lastfmRemaining === 0 && lastfmCheckedUntagged > 0) {
			startLastfmRetryUntagged();
			return;
		}
		void startLastfmEnrichment('');
	}

	function startLastfmRetryUntagged() {
		void startLastfmEnrichment('retry_untagged');
	}

	function startLastfmRefreshAll() {
		void startLastfmEnrichment('refresh');
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
			const response = await cachedApi.getPlaybackRuntime();
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
			mbStats = await cachedApi.getMusicBrainzStatus();
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
			portableSnapshot = await cachedApi.getPortableMusicBrainzSnapshot();
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
			const response = await cachedApi.getDiscoveryStatus();
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
			} else if (response.status === 'already_running') {
				// A run is already in progress (the start was rejected). Without
				// this the click did nothing visible and looked broken.
				showToast('A training run is already in progress. Watch it below or Stop it first.', 'info', 6000);
				errorMsg = '';
			} else {
				showToast(mode === 'full' ? 'Full retrain started.' : 'Incremental refresh started.', 'success');
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

	async function loadRadioSimilarityStatus() {
		try {
			const status = await cachedApi.getRadioSimilarityStatus();
			radioSimilarityRowCount = status.row_count;
			radioSimilarityBuiltAt = status.built_at;
			markServerOnline();
		} catch (error) {
			if (isFetchConnectionError(error)) markServerOffline();
		}
	}

	// The compute route is fire-and-forget. Completion normally arrives via the
	// `radio_similarity_computed` WS event; this poll is the fallback for a
	// dropped socket. It exits when the row count moves, on unmount, or after
	// the deadline.
	async function buildRadioSimilarity() {
		if (radioSimilarityBusy) return;
		radioSimilarityBusy = true;
		// Detect completion by a change in built_at, not row count — a rebuild
		// can legitimately produce the same number of pairs, or zero.
		const before = radioSimilarityBuiltAt;
		try {
			const response = await api.computeRadioSimilarity();
			markServerOnline();
			if (response.status === 'busy') {
				// The server declined: a sync/playback/enrichment writer is
				// active. Nothing is building, so don't poll.
				radioSimilarityLabel = response.message;
				return;
			}
			radioSimilarityLabel =
				response.status === 'already_running'
					? 'A rebuild is already running. Watching for it to finish…'
					: 'Building radio similarity index…';
			const deadline = Date.now() + 10 * 60 * 1000;
			while (Date.now() < deadline && !componentUnmounted) {
				await new Promise((resolve) => setTimeout(resolve, 3000));
				if (componentUnmounted) return;
				await loadRadioSimilarityStatus();
				if (radioSimilarityBuiltAt !== before) {
					radioSimilarityLabel = `Index ready: ${radioSimilarityRowCount?.toLocaleString()} pairs.`;
					return;
				}
			}
			if (!componentUnmounted) {
				radioSimilarityLabel = 'Still building. Check back in a few minutes.';
			}
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				radioSimilarityLabel = SERVER_UNREACHABLE_MESSAGE;
			} else {
				radioSimilarityLabel = `Build failed: ${error}`;
			}
		} finally {
			radioSimilarityBusy = false;
		}
	}

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
			const r = await cachedApi.getDiscoveryIntensity();
			discoveryIntensity = r.intensity;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function loadDiscoveryEngine() {
		try {
			const r = await cachedApi.getDiscoveryEngine();
			discoveryEngine = r.engine;
			discoveryEngineTrainable = r.trainable;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function loadDiscoverySafety() {
		try {
			discoverySafety = await cachedApi.getDiscoverySafety();
			discoverySafetyProfile = discoverySafety.safety_profile;
		} catch (err) {
			if (isFetchConnectionError(err)) markServerOffline();
		}
	}

	async function loadDiscoverySafetyProfile() {
		try {
			const r = await cachedApi.getDiscoverySafetyProfile();
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
			portableStatusLabel = `Exported ${result.snapshot.checked_rows.toLocaleString()} MusicBrainz checked tracks, ${result.snapshot.lastfm_checked_rows.toLocaleString()} Last.fm checked tracks, ${result.snapshot.genre_rows.toLocaleString()} genre rows, and ${result.snapshot.context_tag_rows.toLocaleString()} context tags to ${result.snapshot.path}.`;
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
			portableStatusLabel = `Imported ${result.checked_inserted?.toLocaleString() ?? '0'} MusicBrainz checked markers, ${result.lastfm_checked_inserted?.toLocaleString() ?? '0'} Last.fm checked markers, ${result.genre_inserted?.toLocaleString() ?? '0'} genre rows, and ${result.context_tag_inserted?.toLocaleString() ?? '0'} context tags.`;
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
	type SettingsCategory = 'appearance' | 'sources' | 'audio' | 'account';
	let activeCategory = $state<SettingsCategory>('appearance');
	let handledTidalLoginRequest = $state('');
	$effect(() => {
		const requestedCategory = page.url.searchParams.get('category');
		if (
			requestedCategory === 'appearance' ||
			requestedCategory === 'sources' ||
			requestedCategory === 'audio' ||
			requestedCategory === 'account'
		) {
			activeCategory = requestedCategory;
		}
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

	// ─── Wallpaper picker groups ────────────────────────────────────────
	// The flat 50-tile wall is grouped into labelled sections. Each group's
	// open/closed state is tracked here (defaults from the group metadata);
	// any shader not claimed by a group is swept into a trailing "More" group
	// so a new WALLPAPERS entry can never vanish from the picker.
	const wallpaperNone = WALLPAPERS.find((o) => o.id === 'none') ?? null;
	const groupedIds = new Set(WALLPAPER_GROUPS.flatMap((g) => g.ids));
	const wallpaperGroups = [
		...WALLPAPER_GROUPS.map((g) => ({
			key: g.key,
			label: g.label,
			blurb: g.blurb,
			defaultOpen: g.defaultOpen,
			options: g.ids
				.map((id) => WALLPAPERS.find((o) => o.id === id))
				.filter((o): o is WallpaperOption => !!o)
		})),
		...(() => {
			const rest = WALLPAPERS.filter((o) => o.id !== 'none' && !groupedIds.has(o.id));
			return rest.length
				? [{ key: 'more', label: 'More', blurb: 'Everything else', defaultOpen: false, options: rest }]
				: [];
		})()
	];
	let openGroups = $state<Record<string, boolean>>(
		Object.fromEntries(wallpaperGroups.map((g) => [g.key, g.defaultOpen]))
	);
	function toggleGroup(key: string) {
		openGroups[key] = !openGroups[key];
	}

	// Deterministic tile poster: a category-flavoured gradient with a per-shader
	// hue so every tile reads differently at a glance, without paying for 50 live
	// WebGL contexts. The real shader still renders in the big hover preview.
	function tileHue(id: string): number {
		let h = 0;
		for (let i = 0; i < id.length; i++) h = (h * 31 + id.charCodeAt(i)) >>> 0;
		return h % 360;
	}
	function wallpaperPoster(option: WallpaperOption, groupKey: string): string {
		const hue = tileHue(option.id);
		const h2 = (hue + 42) % 360;
		if (groupKey === 'reactive') {
			return `radial-gradient(circle at 50% 122%, hsl(${hue} 88% 62%) 0%, hsl(${h2} 82% 46%) 32%, #0a0a13 72%)`;
		}
		if (groupKey === 'studio') {
			return `linear-gradient(150deg, hsl(${hue} 14% 84%) 0%, #1b1b1f 55%, #0a0a0c 100%)`;
		}
		if (groupKey === 'pattern') {
			return `repeating-linear-gradient(${hue % 180}deg, hsl(${hue} 72% 56%) 0 3px, #0b0b13 3px 8px)`;
		}
		// ambient / more
		return `radial-gradient(circle at 30% 28%, hsl(${hue} 72% 56%), transparent 60%), radial-gradient(circle at 76% 74%, hsl(${h2} 66% 46%), transparent 60%), #07070c`;
	}

	// ─── Album art palette readout ──────────────────────────────────────
	// Surfaces the colours the "Album art" wallpaper source pulls from the
	// playing cover. State is driven straight off the store's discriminated
	// artPaletteStatus ('off'|'no-art'|'loading'|'ready'|'fallback') so the
	// card never has to guess loading-vs-failed with a timer.
	const ART_ROLES = ['Base', 'Mid', 'Glow', 'Accent'];
	const ART_UNIFORMS = ['u_color1', 'u_color2', 'u_color3', 'u_color4'];
	let artTrack = $derived($currentTrack);
	let artCover = $derived(
		artTrack?.artwork_url ? (upscaleTidalArtwork(artTrack.artwork_url, 320) ?? artTrack.artwork_url) : null
	);
	function artHex(c: [number, number, number]): string {
		const h = (n: number) =>
			Math.round(Math.min(1, Math.max(0, n)) * 255)
				.toString(16)
				.padStart(2, '0');
		return `#${h(c[0])}${h(c[1])}${h(c[2])}`;
	}
	function hideBrokenCover(e: Event) {
		(e.currentTarget as HTMLImageElement).style.visibility = 'hidden';
	}

	// ─── Audio output settings (TIDAL playback runtime) ─────────────────
	let audioDevices = $state<AudioDevice[]>([]);
	let isWindows = $derived(typeof navigator !== 'undefined' && /Win/i.test(navigator.platform));
	let settingsBackgroundLoadCancelled = false;

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

	const EXCLUSIVE_LATENCY_OPTIONS: { value: ExclusiveLatencyMode; label: string }[] = [
		{ value: 'STABLE', label: 'Stable' },
		{ value: 'LOW_LATENCY', label: 'Low latency' },
		{ value: 'ULTRA_LOW_LATENCY', label: 'Ultra low latency' }
	];

	async function loadAudioOutput() {
		await audioSettings.load();
		try {
			const resp = await cachedApi.listAudioDevices();
			audioDevices = resp.devices;
		} catch (err) {
			console.error('Failed to load audio devices', err);
		}
	}

	function scheduleSettingsIdleTask(task: () => void, delayMs: number): () => void {
		if (typeof window === 'undefined') return () => {};
		let idleId: number | null = null;
		const timer = window.setTimeout(() => {
			const idle = window.requestIdleCallback;
			if (typeof idle === 'function') {
				idleId = idle(task, { timeout: 1000 });
				return;
			}
			task();
		}, delayMs);
		return () => {
			window.clearTimeout(timer);
			if (idleId !== null) window.cancelIdleCallback?.(idleId);
		};
	}

	function loadVisibleSettingsCategory() {
		if (activeCategory === 'sources') {
			void loadMbStatus();
			void loadPortableSnapshot();
			void loadRadioSimilarityStatus();
			void loadLastfmStatus();
			return;
		}
		if (activeCategory === 'audio') {
			void loadPlaybackRuntime();
			void loadAudioStats();
			void syncAnalysisStatus();
			void loadPassiveDspState();
			void loadAudioOutput();
			return;
		}
		if (activeCategory === 'account') {
			void loadLastfmStatus();
		}
	}

	function selectSettingsCategory(category: SettingsCategory) {
		if (activeCategory === category) return;
		activeCategory = category;
		void loadVisibleSettingsCategory();
	}

	let settingsQuery = $state('');
	let searchFocused = $state(false);
	let searchMatches = $derived(searchSettings(settingsQuery));

	function jumpToSetting(entry: SettingsSearchEntry) {
		settingsQuery = '';
		searchFocused = false;
		selectSettingsCategory(entry.category);
		// Wait two frames so the freshly-switched category renders before we
		// scroll to and flash the target section.
		requestAnimationFrame(() => {
			requestAnimationFrame(() => {
				const el = document.querySelector(`[data-setting-id="${entry.id}"]`);
				if (el instanceof HTMLElement) {
					el.scrollIntoView({ behavior: 'smooth', block: 'start' });
					el.classList.add('setting-flash');
					setTimeout(() => el.classList.remove('setting-flash'), 1600);
				}
			});
		});
	}

	function scheduleSettingsBackgroundLoad(): () => void {
		settingsBackgroundLoadCancelled = false;
		const cancelers = [
			scheduleSettingsIdleTask(() => {
				if (settingsBackgroundLoadCancelled) return;
				void loadPlaybackRuntime();
				void loadMbStatus();
				void loadDiscoveryStatus();
				void loadDiscoveryEngine();
				void loadRadioSimilarityStatus();
			}, 900),
			scheduleSettingsIdleTask(() => {
				if (settingsBackgroundLoadCancelled) return;
				void loadPortableSnapshot();
				void loadDiscoveryIntensity();
				void loadDiscoverySafetyProfile();
				void loadDiscoverySafety();
				void loadAudioStats();
				void syncAnalysisStatus();
				void loadPassiveDspState();
				void loadAudioOutput();
			}, 1800),
			scheduleSettingsIdleTask(() => {
				if (settingsBackgroundLoadCancelled) return;
				void loadLastfmStatus();
			}, 2800),
		];
		return () => {
			settingsBackgroundLoadCancelled = true;
			for (const cancel of cancelers) cancel();
		};
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

	function onAudioReleaseOnPauseToggle(e: Event) {
		void audioSettings.patch({ exclusive_release_on_pause: (e.target as HTMLInputElement).checked });
	}

	function onExclusiveGraceChange(e: Event) {
		const v = parseInt((e.target as HTMLInputElement).value, 10);
		if (Number.isFinite(v)) {
			void audioSettings.patch({ exclusive_release_grace_secs: v });
		}
	}

	function onExclusiveLatencyModeChange(e: Event) {
		const value = (e.target as HTMLSelectElement).value as ExclusiveLatencyMode;
		void audioSettings.patch({ exclusive_latency_mode: value });
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
	let bitPerfectSettingsActive = $derived(
		$audioSettings.settings?.quality === 'HI_RES_LOSSLESS' &&
		$audioSettings.settings?.exclusive_mode === true &&
		$audioSettings.settings?.sample_rate_follow === true
	);
	let djProcessingActive = $derived(playbackRuntime?.dj_engine_enabled === true);
	let bitPerfectActive = $derived(bitPerfectSettingsActive && !djProcessingActive);

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
		{ id: 'audio', label: 'Audio', icon: '♪', hint: 'Output + analysis' },
		{ id: 'account', label: 'Account', icon: '⚙', hint: 'PIN + updates' },
	];

	let activeCategoryMeta = $derived(
		settingsCategories.find((category) => category.id === activeCategory) ?? settingsCategories[0]
	);

	let activePalette = $derived(PALETTES.find((p) => p.id === $palette) ?? PALETTES[0]);
	let activeSwatches = $derived([
		activePalette.shader.c1,
		activePalette.shader.c2,
		activePalette.shader.c3,
		activePalette.shader.c4
	]);
	let paletteMenuOpen = $state(false);

	function paletteSwatchesFor(p: Palette) {
		return [p.shader.c1, p.shader.c2, p.shader.c3, p.shader.c4];
	}

	function choosePalette(id: PaletteId) {
		setPalette(id);
		paletteMenuOpen = false;
	}

	function closePaletteMenuOnFocusOut(e: FocusEvent) {
		const current = e.currentTarget;
		const next = e.relatedTarget;
		if (!(current instanceof HTMLElement)) return;
		if (next instanceof Node && current.contains(next)) return;
		paletteMenuOpen = false;
	}

	function onPaletteTriggerKeydown(e: KeyboardEvent) {
		if (e.key === 'ArrowDown' || e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			paletteMenuOpen = true;
		}
		if (e.key === 'Escape') {
			paletteMenuOpen = false;
		}
	}
</script>

<svelte:head>
	<title>Settings | NOOR</title>
</svelte:head>

<div class="page-shell settings-page animate-in">
	<header class="settings-command">
		<div class="settings-title">
			<p class="eyebrow">Settings</p>
			<h1>Settings</h1>
			<p>Sources, appearance, audio, access, and updates.</p>
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

	<div class="settings-search">
		<svg class="settings-search-icon" viewBox="0 0 24 24" aria-hidden="true">
			<circle cx="11" cy="11" r="7" />
			<path d="M21 21l-4.2-4.2" />
		</svg>
		<input
			type="search"
			class="settings-search-input"
			placeholder="Search settings..."
			bind:value={settingsQuery}
			onfocus={() => (searchFocused = true)}
			onblur={() => setTimeout(() => (searchFocused = false), 150)}
			onkeydown={(e) => {
				if (e.key === 'Enter' && searchMatches.length) jumpToSetting(searchMatches[0]);
				else if (e.key === 'Escape') settingsQuery = '';
			}}
			aria-label="Search settings"
		/>
		{#if searchFocused && settingsQuery.trim() && searchMatches.length}
			<ul class="settings-search-results">
				{#each searchMatches as match (match.id)}
					<li>
						<button type="button" class="settings-search-result" onclick={() => jumpToSetting(match)}>
							<span class="settings-search-result-label">{match.label}</span>
							<span class="settings-search-result-cat">{match.category}</span>
						</button>
					</li>
				{/each}
			</ul>
		{:else if searchFocused && settingsQuery.trim()}
			<div class="settings-search-empty">No settings match that search.</div>
		{/if}
	</div>

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
		{#each settingsCategories as cat (cat.id)}
			<button
				type="button"
				class="settings-rail-btn"
				class:active={activeCategory === cat.id}
				onclick={() => selectSettingsCategory(cat.id)}
				aria-pressed={activeCategory === cat.id}
			>
				<span class="settings-rail-icon" aria-hidden="true">
					{#if cat.id === 'appearance'}
						<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="8" /><path d="M12 4v16M4 12h16" /></svg>
					{:else if cat.id === 'sources'}
						<svg viewBox="0 0 24 24"><path d="M7 7h10v10H7z" /><path d="M12 2v5M12 17v5M2 12h5M17 12h5" /></svg>
					{:else if cat.id === 'audio'}
						<svg viewBox="0 0 24 24"><path d="M9 18V5l10-2v13" /><circle cx="6" cy="18" r="3" /><circle cx="16" cy="16" r="3" /></svg>
					{:else if cat.id === 'account'}
						<svg viewBox="0 0 24 24"><circle cx="12" cy="8" r="4" /><path d="M5 21c1.5-4 4-6 7-6s5.5 2 7 6" /></svg>
					{:else}
						<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="9" /><path d="M12 11v6M12 7h.01" /></svg>
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
		class:single-column={activeCategory === 'appearance'}
		class:split-even={activeCategory === 'sources' || activeCategory === 'account'}
	>
		<div class="settings-main">
			{#if activeCategory === 'appearance'}
			<section data-setting-id="colour-scheme" class="glass-panel section-panel palette-section" class:palette-section-open={paletteMenuOpen}>
				<SectionHeader eyebrow="Palette" title="Colour scheme" subtitle="UI accent, wallpaper, and no-wallpaper colours." />
				<div class="palette-row">
					<div
						class="palette-picker"
						onfocusout={closePaletteMenuOnFocusOut}
					>
						<button
							type="button"
							class="palette-trigger"
							aria-haspopup="listbox"
							aria-expanded={paletteMenuOpen}
							onclick={() => (paletteMenuOpen = !paletteMenuOpen)}
							onkeydown={onPaletteTriggerKeydown}
						>
							<span class="palette-band" aria-hidden="true">
								<svg viewBox="0 0 96 16" preserveAspectRatio="none">
									<defs>
										<linearGradient id="active-palette-band" x1="0" x2="1" y1="0" y2="0">
											{#each activeSwatches as c, i (i)}
												<stop offset={`${(i / (activeSwatches.length - 1)) * 100}%`} stop-color={rgbCss(c)} />
											{/each}
										</linearGradient>
									</defs>
									<rect width="96" height="16" rx="3" fill="url(#active-palette-band)" />
								</svg>
							</span>
							<span class="palette-trigger-copy">
								<strong>{activePalette.label}</strong>
								<small>{activePalette.sublabel}</small>
							</span>
							<span class="palette-trigger-caret" aria-hidden="true">▾</span>
						</button>
						{#if paletteMenuOpen}
							<div class="palette-menu" role="listbox" aria-label="Colour scheme">
								{#each PALETTES as p (p.id)}
									<button
										type="button"
										class="palette-option"
										class:active={$palette === p.id}
										role="option"
										aria-selected={$palette === p.id}
										onclick={() => choosePalette(p.id)}
									>
										<span class="palette-band" aria-hidden="true">
											<svg viewBox="0 0 96 16" preserveAspectRatio="none">
												<defs>
													<linearGradient id={`palette-band-${p.id}`} x1="0" x2="1" y1="0" y2="0">
														{#each paletteSwatchesFor(p) as c, i (i)}
															<stop offset={`${(i / 3) * 100}%`} stop-color={rgbCss(c)} />
														{/each}
													</linearGradient>
												</defs>
												<rect width="96" height="16" rx="3" fill={`url(#palette-band-${p.id})`} />
											</svg>
										</span>
										<span class="palette-option-copy">
											<strong>{p.label}</strong>
											<small>{p.sublabel}</small>
										</span>
									</button>
								{/each}
							</div>
						{/if}
					</div>
					<div class="palette-swatches" aria-hidden="true">
						{#each activeSwatches as c, i (i)}
							<span class="palette-swatch" style={`background: ${rgbCss(c)}`}></span>
						{/each}
					</div>
				</div>
			</section>

			<section data-setting-id="interface-size" class="glass-panel section-panel">
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

			<section data-setting-id="background" class="glass-panel section-panel">
				<SectionHeader eyebrow="Wallpaper" title="Background" subtitle="Preview, then apply." />

				<div class="wallpaper-big-preview">
					{#if previewShader}
						<ShaderWallpaper
							shader={previewShader}
							maxDpr={$wallpaperQuality === 'high' ? 2 : 1}
							targetFps={$wallpaperFps}
							interactive={true}
							reactGain={WALLPAPERS.find((o) => o.id === previewTileId)?.reactGain ?? 1}
						/>
					{:else if previewTileId === 'none' || $wallpaper === 'none'}
						<div class="wallpaper-none-preview">
							<span>No wallpaper</span>
						</div>
					{:else}
						<div class="wallpaper-big-preview-hint">
							<span>Hover a tile to preview</span>
						</div>
					{/if}
				</div>

				<div class="wallpaper-picker">
					<div class="wallpaper-grid">
						{#if wallpaperNone}
							<button
								type="button"
								class="wallpaper-tile"
								class:active={$wallpaper === 'none'}
								class:previewing={previewTileId === 'none'}
								onclick={() => setWallpaper('none')}
								aria-pressed={$wallpaper === 'none'}
								onpointerenter={() => onTileEnter(wallpaperNone!)}
								onpointerleave={onTileLeave}
							>
								<span class="wallpaper-tile-swatch wallpaper-tile-swatch-none"></span>
								<span class="wallpaper-tile-label">
									<strong>Off</strong>
									{#if $wallpaper === 'none'}<span class="wallpaper-active-badge">On</span>{/if}
								</span>
							</button>
						{/if}
					</div>

					{#each wallpaperGroups as group (group.key)}
						<div class="wallpaper-group-block">
							<button
								type="button"
								class="wallpaper-group-toggle"
								onclick={() => toggleGroup(group.key)}
								aria-expanded={openGroups[group.key]}
							>
								<span class="wallpaper-group-caret" class:open={openGroups[group.key]}>&#9656;</span>
								<span class="wallpaper-group-name">{group.label}</span>
								<span class="wallpaper-group-count">{group.options.length}</span>
								<span class="wallpaper-group-blurb">{group.blurb}</span>
							</button>
							{#if openGroups[group.key]}
								<div class="wallpaper-grid">
									{#each group.options as option (option.id)}
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
											<span
												class="wallpaper-tile-swatch"
												style={`background: ${wallpaperPoster(option, group.key)}`}
											></span>
											<span class="wallpaper-tile-label">
												<strong>{option.label}</strong>
												{#if $wallpaper === option.id}<span class="wallpaper-active-badge">Active</span>{/if}
											</span>
										</button>
									{/each}
								</div>
							{/if}
						</div>
					{/each}
				</div>

				<div class="wallpaper-tune">
					<div class="wallpaper-group">
						<div class="wallpaper-group-head">
							<h4>Reacts to music</h4>
							<p>How the background answers what is playing.</p>
						</div>

						<div class="wallpaper-control">
							<span>
								<strong>Beat reactivity</strong>
								<small>Let the playing track drive the reactive wallpapers.</small>
							</span>
							<div class="wallpaper-control-field">
								<Toggle
									checked={$wallpaperReactive}
									onchange={(e) => setWallpaperReactive(e.currentTarget.checked)}
								/>
							</div>
						</div>

						{#if $wallpaperReactive}
							<div class="wallpaper-subgroup">
								<label class="wallpaper-control">
									<span>
										<strong>Strength</strong>
										<small>How hard it moves with the beat. 100% is the tuned default.</small>
									</span>
									<div class="wallpaper-control-field">
										<input
											type="range"
											min={WALLPAPER_REACTIVITY_MIN}
											max={WALLPAPER_REACTIVITY_MAX}
											step="5"
											value={$wallpaperReactivity}
											oninput={(e) => setWallpaperReactivity(parseInt((e.currentTarget as HTMLInputElement).value, 10))}
											aria-label="Wallpaper reactivity strength"
										/>
										<output>{$wallpaperReactivity}%</output>
									</div>
								</label>

								<label class="wallpaper-control">
									<span>
										<strong>Smoothing</strong>
										<small>Snappy hits on the beat, or floaty swells between them.</small>
									</span>
									<div class="wallpaper-control-field">
										<input
											type="range"
											min={WALLPAPER_SMOOTHING_MIN}
											max={WALLPAPER_SMOOTHING_MAX}
											step="5"
											value={$wallpaperBeatSmoothing}
											oninput={(e) => setWallpaperBeatSmoothing(parseInt((e.currentTarget as HTMLInputElement).value, 10))}
											aria-label="Beat smoothing"
										/>
										<output>
											{$wallpaperBeatSmoothing < 34
												? 'Snappy'
												: $wallpaperBeatSmoothing > 66
													? 'Floaty'
													: 'Balanced'}
										</output>
									</div>
								</label>
							</div>
						{/if}

						<label class="wallpaper-control">
							<span>
								<strong>Reduce motion</strong>
								<small>Calms the reaction. Auto follows your system setting.</small>
							</span>
							<div class="wallpaper-control-field">
								<select
									class="audio-select"
									value={$wallpaperReduceMotion}
									onchange={(e) => setWallpaperReduceMotion((e.currentTarget as HTMLSelectElement).value as WallpaperReduceMotion)}
									aria-label="Reduce motion"
								>
									<option value="auto">Auto (system)</option>
									<option value="on">On</option>
									<option value="off">Off</option>
								</select>
							</div>
						</label>

						<label class="wallpaper-control">
							<span>
								<strong>When idle</strong>
								<small>What the background does when nothing is playing. Drift and Frozen save power; Demo ignores the FPS cap and runs full motion.</small>
							</span>
							<div class="wallpaper-control-field">
								<select
									class="audio-select"
									value={$wallpaperIdle}
									onchange={(e) => setWallpaperIdle((e.currentTarget as HTMLSelectElement).value as WallpaperIdle)}
									aria-label="Idle behaviour"
								>
									<option value="drift">Gentle drift (half FPS)</option>
									<option value="frozen">Frozen (no motion)</option>
									<option value="demo">Demo pulse (full FPS)</option>
								</select>
							</div>
						</label>
					</div>

					<div class="wallpaper-group">
						<div class="wallpaper-group-head">
							<h4>Rendering</h4>
							<p>Colours, sharpness, and how much GPU it uses.</p>
						</div>

						<label class="wallpaper-control">
							<span>
								<strong>Colours</strong>
								<small>Use the palette, or pull colours from the cover art.</small>
							</span>
							<div class="wallpaper-control-field">
								<select
									class="audio-select"
									value={$wallpaperColorSource}
									onchange={(e) => setWallpaperColorSource((e.currentTarget as HTMLSelectElement).value as WallpaperColorSource)}
									aria-label="Wallpaper colours"
								>
									<option value="palette">Palette</option>
									<option value="art">Album art</option>
								</select>
							</div>
						</label>
				<section
					class="art-palette-card"
					class:is-off={$artPaletteStatus === 'off'}
					aria-label="Album art palette"
					aria-hidden={$artPaletteStatus === 'off' ? 'true' : undefined}
				>
					<div class="art-palette-head">
						<span class="art-palette-title">
							<strong>Album art palette</strong>
							<small>The four colours pulled from the cover, and where each lands in the shader.</small>
						</span>
						<span class="art-palette-badge" class:live={$artPaletteStatus === 'ready'}>
							{$artPaletteStatus === 'ready'
								? 'Live'
								: $artPaletteStatus === 'off'
									? 'Off'
									: 'Idle'}
						</span>
					</div>

					{#if $artPaletteStatus !== 'off'}
						<div class="art-palette-body">
							<div class="art-palette-cover">
								{#if artCover && ($artPaletteStatus === 'ready' || $artPaletteStatus === 'fallback')}
									<img
										class="art-palette-cover-img"
										src={artCover}
										alt={artTrack
											? `Cover for ${artTrack.title}${artTrack.artist_name ? ` by ${artTrack.artist_name}` : ''}`
											: 'Cover art'}
										loading="lazy"
										onerror={hideBrokenCover}
									/>
								{:else if $artPaletteStatus === 'loading'}
									<div class="art-palette-cover-skel" aria-hidden="true"></div>
								{:else}
									<svg class="art-palette-cover-icon" viewBox="0 0 24 24" aria-hidden="true">
										<path d="M4 5h16v14H4z" fill="none" stroke="currentColor" stroke-width="1.4" />
										<circle cx="9" cy="10" r="1.6" fill="currentColor" />
										<path d="M4 17l5-4 4 3 3-2 4 3" fill="none" stroke="currentColor" stroke-width="1.4" />
									</svg>
								{/if}
							</div>
							<div class="art-palette-readout">
								{#if $artPaletteStatus === 'ready' && $artPalette}
									<ul class="art-palette-rows">
										{#each $artPalette as c, i (ART_UNIFORMS[i])}
											<li class="art-palette-row">
												<span class="palette-swatch art-palette-chip" style={`background: ${rgbCss(c)}`}></span>
												<span class="art-palette-role">{ART_ROLES[i]}</span>
												<span class="art-palette-hex">{artHex(c)}</span>
												<span class="art-palette-uniform">{ART_UNIFORMS[i]}</span>
											</li>
										{/each}
									</ul>
								{:else if $artPaletteStatus === 'loading'}
									<ul class="art-palette-rows" aria-hidden="true">
										{#each ART_ROLES as role (role)}
											<li class="art-palette-row">
												<span class="palette-swatch art-palette-chip art-palette-chip-skel"></span>
												<span class="art-palette-role">{role}</span>
												<span class="art-palette-hex art-palette-hex-skel"></span>
												<span class="art-palette-uniform art-palette-uniform-skel"></span>
											</li>
										{/each}
									</ul>
								{:else}
									<div class="art-palette-note">
										<svg class="art-palette-note-icon" viewBox="0 0 24 24" aria-hidden="true">
											<circle cx="12" cy="12" r="9" fill="none" stroke="currentColor" stroke-width="1.4" />
											<path d="M12 8v5" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
											<circle cx="12" cy="16" r="0.6" fill="currentColor" stroke="currentColor" stroke-width="0.9" />
										</svg>
										<p>
											{$artPaletteStatus === 'fallback'
												? "Couldn't read this cover. Using the palette instead."
												: 'Play a track to pull colours from its cover.'}
										</p>
									</div>
								{/if}
							</div>
						</div>
						<p class="art-palette-state" aria-live="polite">
							{#if $artPaletteStatus === 'ready'}
								Driving the shader from {artTrack ? artTrack.title : 'this cover'}.
							{:else if $artPaletteStatus === 'loading'}
								Reading the cover...
							{:else if $artPaletteStatus === 'fallback'}
								Palette colours in use until the next readable cover.
							{:else}
								Waiting for something to play.
							{/if}
						</p>
					{:else}
						<p class="art-palette-state art-palette-state-off">
							Set Wallpaper colours to Album art to pull the shader palette from the cover.
						</p>
					{/if}
				</section>

						<label class="wallpaper-control">
							<span>
								<strong>Render quality</strong>
								<small>High is sharper but uses more GPU.</small>
							</span>
							<div class="wallpaper-control-field">
								<select
									class="audio-select"
									value={$wallpaperQuality}
									onchange={(e) => setWallpaperQuality((e.currentTarget as HTMLSelectElement).value as WallpaperQuality)}
									aria-label="Render quality"
								>
									<option value="standard">Standard</option>
									<option value="high">High (2x)</option>
								</select>
							</div>
						</label>

						<label class="wallpaper-control">
							<span>
								<strong>Wallpaper blur</strong>
								<small>Soften or sharpen the background layer.</small>
							</span>
							<div class="wallpaper-control-field">
								<input
									type="range"
									min={WALLPAPER_BLUR_MIN}
									max={WALLPAPER_BLUR_MAX}
									step="1"
									value={$wallpaperBlur}
									oninput={(e) => setWallpaperBlur(parseInt((e.currentTarget as HTMLInputElement).value, 10))}
									aria-label="Wallpaper blur"
								/>
								<output>{$wallpaperBlur}px</output>
							</div>
						</label>

						<label class="wallpaper-control">
							<span>
								<strong>Wallpaper FPS</strong>
								<small>Higher looks smoother. Lower saves GPU.</small>
							</span>
							<div class="wallpaper-control-field">
								<input
									type="range"
									min={WALLPAPER_FPS_MIN}
									max={WALLPAPER_FPS_MAX}
									step="1"
									value={$wallpaperFps}
									oninput={(e) => setWallpaperFps(parseInt((e.currentTarget as HTMLInputElement).value, 10))}
									aria-label="Wallpaper FPS"
								/>
								<output>{$wallpaperFps} FPS</output>
							</div>
						</label>
					</div>
				</div>
			</section>
			{/if}

			{#if activeCategory === 'account'}
			<IntegrationsPanel />

			{/if}

			{#if activeCategory === 'sources'}
			<section data-setting-id="connect-tidal" class="glass-panel section-panel">
				<SectionHeader eyebrow="Streaming" title="Connect TIDAL" subtitle="Auth, sync, and playback metadata." />

				{#if serverStatus === 'offline' && $tidalStatus !== 'connecting'}
					<div class="auth-card glass">
						<p class="page-copy">
							NOOR cannot reach the backend, so it cannot confirm whether your saved
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
								<Toggle
									checked={$syncInfo?.auto_sync_daily ?? false}
									onchange={() => void toggleAutoSync()}
								/>
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
			<section data-setting-id="musicbrainz-enrichment" class="glass-panel section-panel">
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

			<section data-setting-id="last-fm-tags" class="glass-panel section-panel">
				<SectionHeader eyebrow="Metadata" title="Last.fm tags" subtitle="Crowd tags from a local API key." />

				{#if lastfmError}
					<p class="page-copy is-error" role="alert">{lastfmError}</p>
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
								{:else if !lastfmIsRunning && lastfmRemaining === 0 && lastfmCheckedUntagged > 0}
									All {lastfmTotal.toLocaleString()} eligible tracks checked. {lastfmCheckedUntagged.toLocaleString()} returned no saved Last.fm tags.
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
							onclick={startLastfmPrimaryEnrichment}
							disabled={lastfmIsRunning || lastfmTotal === 0 || (lastfmRemaining === 0 && lastfmCheckedUntagged === 0)}
						>
							{lastfmIsRunning
								? 'Running…'
								: lastfmRemaining === 0 && lastfmCheckedUntagged > 0
									? 'Retry untagged'
									: lastfmRemaining === 0
									? 'All checked'
									: lastfmChecked > 0
										? 'Resume enrichment'
										: 'Enrich genres'}
						</button>
						<button
							class="btn btn-glass"
							onclick={startLastfmRefreshAll}
							disabled={lastfmIsRunning || lastfmTotal === 0}
						>
							Recheck all tags
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
			<section data-setting-id="playback-output" class="glass-panel section-panel">
				<SectionHeader eyebrow="Output" title="Playback output" subtitle="Quality, device, and bit-perfect routing." />
				{#if $audioSettings.settings}
					{@const s = $audioSettings.settings}
					<div class="audio-mode-summary">
						<div>
							<span class="audio-mode-label">Current mode</span>
							<strong>
								{bitPerfectActive ? 'Bit-perfect exclusive' : s.exclusive_mode && djProcessingActive ? 'Exclusive with DJ processing' : s.exclusive_mode ? 'Exclusive output' : 'Shared output'}
							</strong>
							<p>
								{#if bitPerfectActive}
									Hi-Res Lossless, exclusive WASAPI, native sample rates. Crossfade is bypassed; same-rate gapless prebuffer stays on.
								{:else if s.exclusive_mode && djProcessingActive}
									Exclusive device control is on. DJ processing is active, so playback is not bit-perfect.
								{:else if s.exclusive_mode}
									Exclusive device control is on. Crossfade is bypassed for bit-perfect output.
								{:else}
									Windows shared output. Crossfade and normal system audio mixing are available.
								{/if}
							</p>
						</div>
						{#if isWindows}
							<span class="audio-mode-toggle">
								<Toggle
									checked={bitPerfectSettingsActive}
									onchange={onBitPerfectToggle}
									label="Toggle bit-perfect mode"
								/>
							</span>
						{/if}
					</div>

					<div class="audio-field-grid">
						<label class="audio-field">
							<span>Stream quality</span>
							<select
								class="audio-select"
								value={s.quality}
								onchange={onAudioQualityChange}
							>
								{#each AUDIO_QUALITY_OPTIONS as opt (opt.value)}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
						</label>
						<label class="audio-field">
							<span>Output device</span>
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
						</label>
					</div>

					{#if isWindows}
						<details class="audio-advanced">
							<summary>
								<span>Advanced output</span>
								<small>Exclusive mode, rate matching, idle release</small>
							</summary>
							<div class="info-list">
								<div class="info-row">
									<span>Exclusive output (WASAPI)</span>
									<strong>
										<Toggle
											checked={s.exclusive_mode}
											onchange={onAudioExclusiveToggle}
										/>
									</strong>
								</div>
								<p class="page-copy setting-caption">
									Takes over the device while playing. Crossfade is bypassed; prepared same-rate tracks still hand off gaplessly.
								</p>
								{#if s.exclusive_mode && !$exclusiveStatus.engaged && $exclusiveStatus.failureReason}
									<div class="exclusive-failed-banner" role="alert">
										<strong>Exclusive mode unavailable</strong>
										<span class="setting-status-line">
											{$exclusiveStatus.failureReason} Audio is currently routed
											through Windows shared mixing.
										</span>
										<div class="exclusive-actions">
											<button
												type="button"
												class="btn btn-primary btn-compact"
												disabled={retryingExclusive}
												onclick={retryExclusive}
											>
												{retryingExclusive ? 'Retrying...' : 'Retry'}
											</button>
											<button
												type="button"
												class="btn btn-compact"
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
								{#if s.exclusive_mode}
									<label class="audio-field audio-field-single">
										<span>Exclusive buffer mode</span>
										<select
											class="audio-select"
											value={s.exclusive_latency_mode}
											onchange={onExclusiveLatencyModeChange}
										>
											{#each EXCLUSIVE_LATENCY_OPTIONS as opt (opt.value)}
												<option value={opt.value}>{opt.label}</option>
											{/each}
										</select>
									</label>
									<p class="page-copy setting-caption">
										Stable is best for music playback. Low latency and ultra low latency reduce output delay when the driver can keep up.
									</p>
								{/if}
								<div class="info-row">
									<span>Idle release</span>
									<strong class="range-with-value">
										<input
											type="range"
											class="exclusive-grace-slider"
											min="5"
											max="120"
											step="5"
											value={s.exclusive_release_grace_secs}
											oninput={onExclusiveGraceChange}
										/>
										<span class="setting-numeric">
											{s.exclusive_release_grace_secs}s
										</span>
									</strong>
								</div>
								<p class="page-copy setting-caption">
									Lower values release the device faster after pause. Higher values avoid repeated device grabs.
								</p>
								<div class="info-row">
									<span>Release on pause</span>
									<strong>
										<Toggle
											checked={s.exclusive_release_on_pause}
											onchange={onAudioReleaseOnPauseToggle}
										/>
									</strong>
								</div>
								<p class="page-copy setting-caption">
									Frees the device the moment you pause so other apps can use it, instead of waiting out the idle release. Re-grabs on play; may add a brief gap on quick pause/resume.
								</p>
								<div class="info-row">
									<span>Sample rate follows source</span>
									<strong>
										<Toggle
											checked={s.sample_rate_follow}
											onchange={onAudioSrFollowToggle}
										/>
									</strong>
								</div>
								<p class="page-copy setting-caption">
									Matches 44.1, 48, 96, or 192 kHz tracks when the device accepts the rate.
								</p>
							</div>
						</details>
					{:else}
						<p class="page-copy setting-caption">Exclusive output is available on Windows.</p>
					{/if}

					<details class="audio-advanced">
						<summary>
							<span>Video playback</span>
							<small>Quality for music videos</small>
						</summary>
						<label class="audio-field audio-field-single">
							<span>Video quality</span>
							<select
								class="audio-select"
								value={s.video_quality_mode}
								onchange={onVideoQualityModeChange}
							>
								{#each VIDEO_QUALITY_OPTIONS as opt (opt.value)}
									<option value={opt.value}>{opt.label}</option>
								{/each}
							</select>
						</label>
						<p class="page-copy setting-caption">
							Max uses the highest stream the video exposes. Auto adapts to bandwidth.
						</p>
					</details>

					{#if $audioSettings.pendingApply}
						<p class="page-copy setting-caption audio-muted">Output reconfiguring...</p>
					{/if}
					{#if $audioSettings.error}
						<p class="page-copy audio-error">{$audioSettings.error}</p>
					{/if}
				{:else if $audioSettings.loading}
					<p class="page-copy">Loading audio settings...</p>
				{:else if $audioSettings.error}
					<p class="page-copy audio-error">{$audioSettings.error}</p>
				{/if}
			</section>
			{/if}

			{#if activeCategory === 'audio'}
			<section data-setting-id="downloads" class="glass-panel section-panel">
				<SectionHeader eyebrow="Output" title="Downloads" subtitle="Save tracks to disk as FLAC or MP3." />

				<div class="download-settings">
					<label class="audio-field download-folder-field">
						<span>Save to folder</span>
						<div class="download-folder-row">
							<input
								class="audio-select download-folder-input"
								type="text"
								value={downloadFolder}
								readonly={isTauri()}
								placeholder="Choose a download folder"
								onchange={(e) => void commitDownloadFolder((e.currentTarget as HTMLInputElement).value)}
							/>
							{#if isTauri()}
								<button
									type="button"
									class="btn btn-glass download-folder-btn"
									disabled={downloadFolderSaving}
									onclick={() => void chooseDownloadFolder()}
								>
									{downloadFolderSaving ? 'Saving…' : 'Change'}
								</button>
							{/if}
						</div>
					</label>

					<div class="audio-field">
						<span>Default format</span>
						<div class="download-format-toggle" role="group" aria-label="Default download format">
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultDownloadFormat === 'flac'}
								aria-pressed={$defaultDownloadFormat === 'flac'}
								onclick={() => setDownloadFormat('flac')}
							>
								FLAC<small>Lossless</small>
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultDownloadFormat === 'aac'}
								aria-pressed={$defaultDownloadFormat === 'aac'}
								onclick={() => setDownloadFormat('aac')}
							>
								AAC<small>M4A, best lossy</small>
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultDownloadFormat === 'mp3'}
								aria-pressed={$defaultDownloadFormat === 'mp3'}
								onclick={() => setDownloadFormat('mp3')}
							>
								MP3<small>320 kbps</small>
							</button>
						</div>
					</div>

					<div class="audio-field">
						<span>FLAC quality</span>
						<div class="download-format-toggle" role="group" aria-label="FLAC download quality">
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultFlacQuality === 'hires'}
								aria-pressed={$defaultFlacQuality === 'hires'}
								onclick={() => setFlacQuality('hires')}
							>
								Hi-Res<small>Best available</small>
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultFlacQuality === 'cd'}
								aria-pressed={$defaultFlacQuality === 'cd'}
								onclick={() => setFlacQuality('cd')}
							>
								CD<small>16-bit / 44.1 kHz</small>
							</button>
						</div>
					</div>

					<div class="audio-field">
						<span>MP3 source</span>
						<div class="download-format-toggle" role="group" aria-label="MP3 transcode source">
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultMp3Source === 'aac'}
								aria-pressed={$defaultMp3Source === 'aac'}
								onclick={() => setMp3Source('aac')}
							>
								AAC<small>Fast</small>
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultMp3Source === 'lossless'}
								aria-pressed={$defaultMp3Source === 'lossless'}
								onclick={() => setMp3Source('lossless')}
							>
								Lossless<small>Best MP3, slower</small>
							</button>
						</div>
					</div>

					<p class="download-settings-hint">
						FLAC saves the lossless master; MP3 is a smaller portable copy. Right-click any track,
						album, or playlist to download, or use the download button on the now-playing artwork.
					</p>
				</div>
			</section>
			{/if}

			{#if activeCategory === 'audio'}
			<section data-setting-id="discovery-engine" class="glass-panel section-panel">
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
							<strong>{discoveryStatusLastTrainedAt ? new Date(discoveryStatusLastTrainedAt + 'Z').toLocaleString() : '—'}</strong>
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

			<section data-setting-id="radio-similarity-index" class="glass-panel section-panel">
				<SectionHeader
					eyebrow="Learning"
					title="Radio similarity index"
					subtitle="Metadata-heuristic recall lane for radio (co-album, co-artist, co-listen, genre)."
				/>
				<p>
					The radio Engine lane reads a precomputed <code>track_similarity</code> index. It's separate from the discovery model's learned neighbours — radio uses both. If it's never built, the Engine lane silently contributes nothing. Building it can take a few minutes on large libraries.
				</p>
				<div class="info-list">
					<div class="info-row">
						<span>Indexed pairs</span>
						<strong>{radioSimilarityRowCount === null ? '—' : radioSimilarityRowCount.toLocaleString()}</strong>
					</div>
					<div class="info-row">
						<span>Last built</span>
						<strong>{radioSimilarityBuiltAt ?? 'Never'}</strong>
					</div>
				</div>
				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void buildRadioSimilarity()} disabled={radioSimilarityBusy}>
						{radioSimilarityBusy ? 'Building…' : 'Build radio similarity index'}
					</button>
				</div>
				{#if radioSimilarityLabel}
					<p class="galaxy-refresh-label">{radioSimilarityLabel}</p>
				{/if}
			</section>
			{/if}
		</div>

		<div class="settings-side">

			{#if activeCategory === 'account'}
			<section data-setting-id="access-pin" class="glass-panel section-panel">
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

			<section data-setting-id="app-updates" class="glass-panel section-panel">
				<SectionHeader eyebrow="Desktop" title="App updates" subtitle="Version, install mode, and update checks." />
				<div class="inner-metrics">
					<MetricPair label="Version" value={appVersion || 'Unknown'} copy="Current app build." />
					<MetricPair label="Install mode" value={installModeLabel} copy={desktopAppAvailable ? 'Detected from the running shell.' : 'Use the desktop app for update checks.'} />
					<MetricPair label="Updates" value={updateStatus} copy={updateAvailableVersion ? 'Ready from the tray menu or this panel.' : 'Manual checks use the active release channel.'} />
				</div>
				<div class="action-row">
					<button
						type="button"
						class="btn btn-primary"
						onclick={() => void checkForUpdatesNow()}
						disabled={!desktopAppAvailable || updateChecking}
					>
						{updateChecking ? 'Checking...' : 'Check for updates'}
					</button>
					{#if !desktopAppAvailable}
						<span class="page-copy setting-caption">Available in the desktop app.</span>
					{/if}
				</div>
				{#if updateError}
					<p class="field-error" role="alert">{updateError}</p>
				{/if}
			</section>

			{#if desktopAppAvailable}
			<section data-setting-id="closing-the-window" class="glass-panel section-panel">
				<SectionHeader eyebrow="Desktop" title="Closing the window" subtitle="Quit NOORwave, or keep it running in the tray." />
				<div class="info-list">
					<div class="info-row">
						<div>
							<span>Minimize to tray on close</span>
							<p class="info-row-hint">
								{minimizeToTray
									? 'Closing the window keeps NOORwave running in the tray. Quit from the tray menu.'
									: 'Closing the window quits NOORwave. Turn this on to keep it running in the tray instead.'}
							</p>
						</div>
						<strong>
							<Toggle
								checked={minimizeToTray}
								onchange={(e) => void setMinimizeToTray(e.currentTarget.checked)}
							/>
						</strong>
					</div>
				</div>
			</section>
			{/if}
			{/if}

			{#if activeCategory === 'sources'}
			<section data-setting-id="portable-snapshot" class="glass-panel section-panel">
				<SectionHeader eyebrow="Transfer" title="Portable snapshot" subtitle="Export/import MusicBrainz and Last.fm enrichment." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Snapshot checked" value={portableSnapshot?.checked_rows?.toLocaleString() ?? '0'} copy="Tracks marked as already processed." />
					<MetricPair label="Snapshot genres" value={portableSnapshot?.genre_rows?.toLocaleString() ?? '0'} copy="Genre rows ready to import elsewhere." />
					<MetricPair label="Last.fm checked" value={portableSnapshot?.lastfm_checked_rows?.toLocaleString() ?? '0'} copy="Last.fm tracks marked as already processed." />
					<MetricPair label="Context tags" value={portableSnapshot?.context_tag_rows?.toLocaleString() ?? '0'} copy="Last.fm mood and activity tags ready to import." />
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
			<section data-setting-id="clear-non-library-entries" class="glass-panel section-panel">
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
				<p class="page-copy is-warning">
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
					<p class="page-copy is-error" role="alert">{purgeError}</p>
				{/if}
				<div class="action-row">
					<button class="btn btn-glass danger" onclick={purgeOrphanTidalStream} disabled={purgeRunning}>
						{purgeRunning ? 'Purging…' : 'Clear non-library entries'}
					</button>
				</div>
			</section>
			{/if}

			{#if activeCategory === 'audio'}
			<section data-setting-id="now-playing-path" class="glass-panel section-panel">
				<SectionHeader eyebrow="Runtime" title="Now playing path" subtitle="Current device and format." />
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
						<span>Track ID</span>
						<strong>{playbackRuntime?.active_track_id ?? 'None'}</strong>
					</div>
				</div>

				{#if playbackRuntime?.last_error}
					<p class="runtime-error">{playbackRuntime.last_error}</p>
				{/if}
			</section>
			{/if}

			{#if activeCategory === 'audio'}
			<section data-setting-id="library-audio-data" class="glass-panel section-panel">
				<SectionHeader eyebrow="Analysis" title="Library audio data" subtitle="Passive BPM, key, and energy capture." />

				<div class="stat-grid inner-metrics">
					<MetricPair label="Analyzed" value={$audioAnalysis.analyzed.toLocaleString()} copy="Tracks with DSP features." />
					<MetricPair label="Avg BPM" value={$audioAnalysis.stats?.avg_bpm?.toFixed(1) ?? '—'} copy="Average tempo across analyzed tracks." />
					<MetricPair label="Top Key" value={$audioAnalysis.stats?.top_key ?? '—'} copy="Most common key signature." />
					<MetricPair label="Avg Energy" value={$audioAnalysis.stats?.avg_energy?.toFixed(2) ?? '—'} copy="Average energy level (0–1)." />
				</div>

				<p class="analysis-note">
					New data is captured from playback. There is no bulk scan because large TIDAL preview bursts trigger rate limits.
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
						<Toggle
							checked={$audioAnalysis.passiveEnabled}
							onchange={(e) => void setPassiveDspEnabled((e.currentTarget as HTMLInputElement).checked)}
						/>
					</strong>
				</div>

				<div class="action-row">
					<button class="btn btn-glass danger" onclick={clearAllAnalysis}>Clear All</button>
				</div>

				<details class="advanced-details">
					<summary>Advanced analysis limits</summary>
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

		</div>
	</section>
</div>

<style>
	/* Caption + status helpers — extracted from template inline styles */
	.setting-caption {
		font-size: var(--font-size-sm);
	}
	.setting-status-line {
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}
	.setting-numeric {
		margin-left: 0.5rem;
		font-variant-numeric: tabular-nums;
	}

	.audio-mode-summary {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--space-4);
		padding: var(--space-4);
		border-left: 3px solid var(--accent);
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.03);
	}

	.audio-mode-summary > div {
		display: grid;
		gap: var(--space-1);
		min-width: 0;
	}

	.audio-mode-label,
	.audio-field > span,
	.audio-advanced summary small {
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: var(--text-tertiary, var(--text-secondary));
	}

	.audio-mode-summary strong {
		font-size: var(--font-size-lg);
		line-height: var(--line-height-tight);
	}

	.audio-mode-summary p {
		margin: 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
		color: var(--text-secondary);
	}

	.audio-mode-toggle {
		flex: 0 0 auto;
	}

	.audio-field-grid {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: var(--space-3);
	}

	.audio-field {
		display: grid;
		gap: var(--space-2);
		min-width: 0;
	}

	.audio-field-single {
		margin-top: var(--space-3);
	}

	.audio-select {
		width: 100%;
		min-width: 0;
		padding: 9px 10px;
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.05);
		color: var(--text-primary);
		font: inherit;
	}

	.audio-advanced {
		border: 1px solid var(--border-subtle);
		border-radius: var(--radius-sm);
		padding: var(--space-3) var(--space-4);
	}

	.download-settings {
		display: grid;
		gap: var(--space-4);
		margin-top: var(--space-3);
	}

	.download-folder-field {
		min-width: 0;
	}

	.download-folder-row {
		display: flex;
		gap: var(--space-2);
		align-items: center;
		min-width: 0;
	}

	.download-folder-input {
		flex: 1 1 auto;
	}

	.download-folder-btn {
		flex: 0 0 auto;
		white-space: nowrap;
	}

	.download-format-toggle {
		display: flex;
		gap: var(--space-2);
	}

	.download-format-option {
		flex: 1 1 0;
		display: grid;
		gap: 2px;
		justify-items: center;
		padding: 8px 12px;
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-sm);
		background: rgba(255, 255, 255, 0.05);
		color: var(--text-secondary);
		font-family: inherit;
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition:
			background 160ms ease,
			border-color 160ms ease,
			color 160ms ease;
	}

	.download-format-option small {
		font-size: var(--font-size-xs);
		opacity: 0.7;
	}

	.download-format-option:hover {
		border-color: var(--accent-line);
	}

	.download-format-option.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.download-settings-hint {
		margin: 0;
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
	}

	.audio-advanced summary {
		display: flex;
		align-items: baseline;
		justify-content: space-between;
		gap: var(--space-3);
		cursor: pointer;
		list-style: none;
	}

	.audio-advanced summary::-webkit-details-marker {
		display: none;
	}

	.audio-advanced summary > span {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
	}

	.audio-advanced[open] {
		background: rgba(255, 255, 255, 0.02);
	}

	.audio-advanced[open] summary {
		margin-bottom: var(--space-3);
	}

	.exclusive-failed-banner {
		display: grid;
		gap: var(--space-2);
		margin: var(--space-2) 0;
		padding: var(--space-3) var(--space-4);
		border: 1px solid var(--state-error);
		border-left-width: 4px;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-error) 12%, transparent);
		color: var(--text-primary);
	}

	.exclusive-failed-banner strong {
		color: var(--text-primary);
	}

	.exclusive-actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space-2);
	}

	.btn-compact {
		padding: var(--space-1) var(--space-3);
	}

	.range-with-value {
		display: inline-flex;
		align-items: center;
		gap: var(--space-2);
	}

	.exclusive-grace-slider {
		width: clamp(8rem, 18vw, 10rem);
		accent-color: var(--accent);
	}

	.audio-muted {
		color: var(--text-secondary);
	}

	.audio-error {
		color: var(--state-error);
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
		background: var(--surface-2);
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

	.settings-search {
		position: relative;
		margin-bottom: var(--space-3);
	}

	.settings-search-icon {
		position: absolute;
		left: 14px;
		top: 50%;
		transform: translateY(-50%);
		width: 16px;
		height: 16px;
		fill: none;
		stroke: currentColor;
		stroke-width: 2;
		stroke-linecap: round;
		opacity: 0.5;
		pointer-events: none;
	}

	.settings-search-input {
		width: 100%;
		padding: 10px 14px 10px 40px;
		background: var(--bg-surface);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		color: inherit;
		font-size: var(--font-size-sm);
	}

	.settings-search-input::placeholder {
		color: var(--text-tertiary);
	}

	.settings-search-input:focus {
		outline: none;
		border-color: var(--accent);
	}

	.settings-search-results {
		position: absolute;
		z-index: var(--z-overlay);
		top: calc(100% + 6px);
		left: 0;
		right: 0;
		margin: 0;
		padding: 6px;
		list-style: none;
		background: var(--bg-elevated);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		box-shadow: 0 12px 32px rgba(0, 0, 0, 0.35);
		max-height: 320px;
		overflow-y: auto;
	}

	.settings-search-results li {
		list-style: none;
	}

	.settings-search-result {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: var(--gap-sm);
		width: 100%;
		padding: 8px 12px;
		background: transparent;
		border: none;
		border-radius: var(--radius-sm);
		color: inherit;
		cursor: pointer;
		text-align: left;
	}

	.settings-search-result:hover {
		background: var(--bg-surface);
	}

	.settings-search-result-label {
		font-size: var(--font-size-sm);
	}

	.settings-search-result-cat {
		flex: 0 0 auto;
		font-size: var(--font-size-2xs);
		text-transform: uppercase;
		letter-spacing: 0.06em;
		color: var(--text-tertiary);
	}

	.settings-search-empty {
		position: absolute;
		z-index: var(--z-overlay);
		top: calc(100% + 6px);
		left: 0;
		right: 0;
		padding: 14px;
		background: var(--bg-elevated);
		border: 1px solid var(--panel-border);
		border-radius: var(--radius-md);
		font-size: var(--font-size-sm);
		color: var(--text-tertiary);
	}

	:global(.setting-flash) {
		animation: settingFlash 1.6s ease;
	}

	@keyframes -global-settingFlash {
		0% {
			box-shadow: 0 0 0 0 transparent;
		}
		20% {
			box-shadow: 0 0 0 2px var(--accent);
		}
		100% {
			box-shadow: 0 0 0 0 transparent;
		}
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

	/* Sources and Account split their cards across two even columns (main + side).
	   Each whole card lives in one column, so they fill the width without the
	   multicol split that tore tall cards (like the integrations panel) in half. */
	.settings-grid.split-even {
		grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
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

	.palette-picker {
		position: relative;
		flex: 1;
		min-width: 220px;
	}

	.palette-trigger,
	.palette-option {
		width: 100%;
		display: grid;
		grid-template-columns: minmax(76px, 96px) minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
		text-align: left;
		border: 1px solid var(--border-subtle);
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		color: var(--text-primary);
		cursor: pointer;
	}

	.palette-trigger {
		min-height: 46px;
		padding: 8px 10px;
		border-radius: var(--radius-sm);
	}

	.palette-trigger:hover,
	.palette-trigger:focus-visible,
	.palette-option:hover,
	.palette-option:focus-visible,
	.palette-option.active {
		border-color: color-mix(in srgb, var(--accent-strong) 48%, transparent);
		background: color-mix(in srgb, var(--accent-soft) 56%, var(--instrument-surface));
		outline: none;
	}

	.palette-band {
		display: block;
		min-width: 0;
		height: 16px;
		border-radius: 4px;
		overflow: hidden;
		border: 1px solid rgba(255, 255, 255, 0.16);
		background: var(--bg-raised);
		box-shadow: 0 0 0 1px rgba(0, 0, 0, 0.24) inset;
	}

	.palette-band svg {
		display: block;
		width: 100%;
		height: 100%;
	}

	.palette-trigger-copy,
	.palette-option-copy {
		min-width: 0;
		display: grid;
		gap: 2px;
	}

	.palette-trigger-copy strong,
	.palette-option-copy strong {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.palette-trigger-copy small,
	.palette-option-copy small {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.palette-trigger-caret {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		line-height: 1;
	}

	.palette-menu {
		position: absolute;
		inset-inline: 0;
		top: calc(100% + 6px);
		z-index: var(--z-overlay);
		max-height: min(420px, 58vh);
		overflow-y: auto;
		display: grid;
		gap: 4px;
		padding: 6px;
		border-radius: var(--radius-sm);
		border: 1px solid var(--border-strong);
		background: rgba(10, 10, 14, 0.97);
		backdrop-filter: var(--blur-modal);
		-webkit-backdrop-filter: var(--blur-modal);
		box-shadow: var(--panel-shadow);
	}

	.palette-option {
		grid-template-columns: minmax(68px, 88px) minmax(0, 1fr);
		padding: 8px;
		border-radius: var(--radius-xs);
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

	.wallpaper-none-preview {
		position: absolute;
		inset: 0;
		display: flex;
		align-items: center;
		justify-content: center;
		background:
			radial-gradient(circle at 14% 12%, var(--atlas-haze-a), transparent 34%),
			radial-gradient(circle at 88% 16%, var(--atlas-haze-b), transparent 32%),
			radial-gradient(circle at 72% 88%, var(--atlas-haze-c), transparent 34%),
			var(--atlas-bg);
	}

	.wallpaper-none-preview span {
		padding: 5px 10px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: color-mix(in srgb, var(--bg-base) 68%, transparent);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		text-transform: uppercase;
		letter-spacing: 0.08em;
	}

	.wallpaper-big-preview-hint span {
		font-size: var(--font-size-xs);
		letter-spacing: 0.08em;
		text-transform: uppercase;
		color: rgba(255, 255, 255, 0.20);
	}

	/* ── Picker: grouped, collapsible ── */
	.wallpaper-picker {
		display: grid;
		gap: 6px;
	}

	.wallpaper-group-block {
		display: grid;
		gap: 8px;
	}

	.wallpaper-group-toggle {
		all: unset;
		display: flex;
		align-items: baseline;
		gap: 8px;
		cursor: pointer;
		padding: 8px 4px 4px;
		border-top: 1px solid var(--border-subtle);
	}

	.wallpaper-group-caret {
		font-size: var(--font-size-2xs);
		color: var(--text-tertiary, var(--text-secondary));
		transition: transform 160ms ease;
	}

	.wallpaper-group-caret.open {
		transform: rotate(90deg);
	}

	.wallpaper-group-name {
		font-size: var(--font-size-sm);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
	}

	.wallpaper-group-count {
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
		padding: 1px 6px;
		border-radius: 999px;
		background: rgba(255, 255, 255, 0.06);
	}

	.wallpaper-group-blurb {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, var(--text-secondary));
		margin-left: auto;
		text-align: right;
	}

	.wallpaper-group-toggle:hover .wallpaper-group-name {
		color: var(--accent, #7c80ff);
	}

	/* ── Tune: two grouped control cards ── */
	.wallpaper-tune {
		display: grid;
		grid-template-columns: repeat(2, minmax(0, 1fr));
		gap: 10px;
		align-items: start;
	}

	.wallpaper-group {
		border: 1px solid var(--border-subtle);
		border-radius: 10px;
		background: rgba(255, 255, 255, 0.02);
		padding: 4px 12px 8px;
	}

	.wallpaper-group-head {
		display: grid;
		gap: 2px;
		padding: 10px 0 6px;
	}

	.wallpaper-group-head h4 {
		margin: 0;
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		color: var(--text-secondary);
	}

	.wallpaper-group-head p {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, var(--text-secondary));
		line-height: var(--line-height-snug);
	}

	.wallpaper-control {
		display: grid;
		grid-template-columns: minmax(0, 1fr) minmax(120px, 168px);
		align-items: center;
		gap: 14px;
		padding: 9px 0;
		border-top: 1px solid var(--border-subtle);
	}

	.wallpaper-group-head + .wallpaper-control,
	.wallpaper-subgroup .wallpaper-control:first-child {
		border-top: none;
	}

	.wallpaper-subgroup {
		display: grid;
		gap: 0;
		margin-left: 10px;
		padding-left: 10px;
		border-left: 2px solid color-mix(in srgb, var(--accent, #6366f1) 40%, transparent);
	}

	/* Album art palette readout card: mirrors the .wallpaper-control frame and
	   reserves its footprint so loading -> ready -> fallback never shift layout. */
	.art-palette-card {
		margin-top: 10px;
		padding: 12px;
		border: 1px solid var(--border-subtle);
		border-radius: 8px;
		background: rgba(255, 255, 255, 0.025);
		display: grid;
		gap: 12px;
	}

	.art-palette-card.is-off {
		opacity: 0.55;
		gap: 8px;
	}

	.art-palette-head {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 12px;
	}

	.art-palette-title {
		display: grid;
		gap: 3px;
		min-width: 0;
	}

	.art-palette-title strong {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.art-palette-title small {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, var(--text-secondary));
		line-height: var(--line-height-snug);
	}

	.art-palette-badge {
		flex: none;
		padding: 2px 9px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		background: rgba(255, 255, 255, 0.04);
		white-space: nowrap;
	}

	.art-palette-badge.live {
		color: var(--accent);
		border-color: color-mix(in srgb, var(--accent) 45%, transparent);
		background: color-mix(in srgb, var(--accent) 14%, transparent);
	}

	.art-palette-body {
		display: grid;
		grid-template-columns: 96px minmax(0, 1fr);
		gap: 16px;
		min-height: 96px;
	}

	.art-palette-cover {
		width: 96px;
		height: 96px;
		border-radius: 8px;
		overflow: hidden;
		border: 1px solid var(--border-subtle);
		background: rgba(255, 255, 255, 0.03);
		display: flex;
		align-items: center;
		justify-content: center;
		color: var(--text-tertiary, var(--text-secondary));
	}

	.art-palette-cover-img {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.art-palette-cover-icon {
		width: 34px;
		height: 34px;
		opacity: 0.7;
	}

	.art-palette-cover-skel {
		width: 100%;
		height: 100%;
		background: linear-gradient(
			90deg,
			rgba(255, 255, 255, 0.04),
			rgba(255, 255, 255, 0.1),
			rgba(255, 255, 255, 0.04)
		);
		background-size: 200% 100%;
		animation: art-shimmer 1.1s ease-in-out infinite;
	}

	.art-palette-readout {
		display: flex;
		align-items: center;
		min-width: 0;
	}

	.art-palette-rows {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 6px;
		width: 100%;
	}

	.art-palette-row {
		display: grid;
		grid-template-columns: 22px minmax(48px, auto) 1fr auto;
		align-items: center;
		gap: 10px;
	}

	.art-palette-chip {
		border-radius: 5px;
	}

	.art-palette-role {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.art-palette-hex,
	.art-palette-uniform {
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
	}

	.art-palette-uniform {
		color: var(--text-tertiary, var(--text-secondary));
		text-align: right;
	}

	.art-palette-chip-skel,
	.art-palette-hex-skel,
	.art-palette-uniform-skel {
		background: linear-gradient(
			90deg,
			rgba(255, 255, 255, 0.04),
			rgba(255, 255, 255, 0.1),
			rgba(255, 255, 255, 0.04)
		);
		background-size: 200% 100%;
		animation: art-shimmer 1.1s ease-in-out infinite;
	}

	.art-palette-chip-skel {
		border: 1px solid rgba(255, 255, 255, 0.12);
	}

	.art-palette-hex-skel {
		width: 56px;
		height: 10px;
		border-radius: 999px;
	}

	.art-palette-uniform-skel {
		width: 60px;
		height: 10px;
		border-radius: 999px;
		justify-self: end;
	}

	.art-palette-note {
		display: flex;
		align-items: center;
		gap: 10px;
		color: var(--text-secondary);
	}

	.art-palette-note-icon {
		width: 20px;
		height: 20px;
		flex: none;
		opacity: 0.7;
	}

	.art-palette-note p {
		margin: 0;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
	}

	.art-palette-state {
		margin: 0;
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, var(--text-secondary));
	}

	@keyframes art-shimmer {
		0% {
			background-position: 200% 0;
		}
		100% {
			background-position: -200% 0;
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.art-palette-cover-skel,
		.art-palette-chip-skel,
		.art-palette-hex-skel,
		.art-palette-uniform-skel {
			animation: none;
			background: rgba(255, 255, 255, 0.06);
		}
	}

	@media (max-width: 560px) {
		.art-palette-body {
			grid-template-columns: 1fr;
		}
	}

	.wallpaper-control > span {
		display: grid;
		gap: 3px;
		min-width: 0;
	}

	.wallpaper-control strong {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.wallpaper-control small {
		font-size: var(--font-size-xs);
		color: var(--text-tertiary, var(--text-secondary));
		line-height: var(--line-height-snug);
	}

	.wallpaper-control-field {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 8px;
	}

	.wallpaper-control-field select {
		flex: 1;
		min-width: 0;
	}

	.wallpaper-control-field input {
		flex: 1;
		min-width: 0;
		accent-color: var(--accent);
	}

	.wallpaper-control-field output {
		min-width: 5ch;
		text-align: right;
		font-size: var(--font-size-xs);
		font-variant-numeric: tabular-nums;
		color: var(--text-secondary);
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
		width: 40px;
		height: 30px;
		border-radius: 6px;
		background: linear-gradient(135deg,
			color-mix(in srgb, var(--accent, #7c80ff) 60%, #0a0a14),
			color-mix(in srgb, var(--accent, #7c80ff) 20%, #050508));
		border: 1px solid var(--panel-border);
		box-shadow: inset 0 0 0 1px rgba(0, 0, 0, 0.25);
	}

	.wallpaper-tile-swatch-none {
		background:
			radial-gradient(circle at 28% 28%, var(--atlas-haze-a), transparent 64%),
			radial-gradient(circle at 76% 72%, var(--atlas-haze-b), transparent 62%),
			var(--atlas-bg);
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
		line-height: var(--line-height-snug);
		overflow-wrap: anywhere;
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


	.section-panel {
		padding: 16px;
		display: flex;
		flex-direction: column;
		gap: 14px;
		border-radius: var(--radius-md);
	}

	.palette-section-open {
		position: relative;
		z-index: calc(var(--z-overlay) + 1);
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

	.runtime-error {
		color: var(--state-error);
	}

	.galaxy-refresh-label {
		margin: 4px 0 0;
		font-size: var(--font-size-sm);
		color: var(--signal-text);
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

		.audio-mode-summary,
		.audio-advanced summary {
			align-items: flex-start;
			flex-direction: column;
		}

		.audio-field-grid {
			grid-template-columns: 1fr;
		}

		.wallpaper-tune {
			grid-template-columns: 1fr;
		}

		.action-row {
			flex-direction: column;
		}

		.action-row :global(.btn) {
			width: 100%;
		}
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

	.is-error {
		color: var(--state-error);
	}

	.is-warning {
		color: var(--state-warning);
	}
</style>
