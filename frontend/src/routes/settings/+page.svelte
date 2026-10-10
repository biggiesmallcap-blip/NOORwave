<script lang="ts">
	import { motionPreference, prefersReducedMotion } from '$lib/stores/motion';
	import { librarySongsScope } from '$lib/stores/library_songs';
	import { onMount, tick, untrack } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import SettingGroup from '$lib/components/settings/SettingGroup.svelte';
	import SettingRow from '$lib/components/settings/SettingRow.svelte';
	import Segmented from '$lib/components/ui/Segmented.svelte';
	import Dropdown from '$lib/components/ui/Dropdown.svelte';
	import { likeSongOnVideoSave } from '$lib/videos/like_song_for_video';
	import AppearanceFields, { type AppearanceValues } from '$lib/components/settings/AppearanceFields.svelte';
	import StartupSetting from '$lib/components/settings/StartupSetting.svelte';
	import CloseBehaviorSetting from '$lib/components/settings/CloseBehaviorSetting.svelte';
	import ExternalLink from '$lib/components/ui/ExternalLink.svelte';
	import { SETTINGS_CATEGORIES, resolveSettingsLocation, settingsHref, categoryLabel, type SettingsCategoryId } from '$lib/components/settings/settingsManifest';
	import '$lib/components/settings/settings.css';
	import type { Unsubscriber } from 'svelte/store';
	import { showToast } from '$lib/stores/toast';
	import {
		api,
		getApiBase,
		authFetch,
		type AudioDevice,
		type AudioQuality,
		type ExclusiveLatencyMode,
		type VideoQualityMode,
		type DatabaseStats,
		type DiscoveryEngine,
		type DiscoveryStatus,
		type DiscoveryTrainingSafetyProfile,
		type MusicBrainzStatus,
		type PlaybackRuntimeInfo,
		type PortableMusicBrainzSnapshotStatus,
		type ArtworkCacheSettings
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
		setSyncEnrichment,
		recleanLibrary,
		cancelTidalSync,
		startTidalSync
	} from '$lib/stores/tidal';
	import {
		audioAnalysis,
		clearAllAnalysis, audioAnalysisError, passiveDspKnown, passiveDspPending,
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
	import TidalContentSetting from '$lib/components/settings/TidalContentSetting.svelte';
	import VideoDiscoverySetting from '$lib/components/settings/VideoDiscoverySetting.svelte';
	import ExploreStationsSetting from '$lib/components/settings/ExploreStationsSetting.svelte';
	import IntegrationsPanel from '$lib/components/settings/IntegrationsPanel.svelte';
	import PhoneRemotePanel from '$lib/components/settings/PhoneRemotePanel.svelte';
	import {
		applyTrainingProgress,
		describeDiscoveryRun,
		describeDiscoveryUpgrade,
		discoveryLastTrainedAt,
		discoveryModelHeldBack,
		discoveryModelLabel,
		discoveryStageLabel,
		type DiscoveryUpgrade,
		shouldContinueDiscoveryCompletionRefresh,
		shouldRefreshAfterTerminalDiscoveryProgress
	} from '$lib/components/settings/discovery_status';
	import { portal } from '$lib/actions/portal';
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
	import { crossfadeMs, currentTrack, setPlayerCrossfadeMs } from '$lib/stores/player';
	import { upscaleTidalArtwork } from '$lib/utils/artwork';
	import { palette, setPalette } from '$lib/stores/palette';
	import { uiZoom, setZoom, zoomIn, zoomOut, resetZoom, MIN as ZOOM_MIN, MAX as ZOOM_MAX, WHEEL_STEP as ZOOM_STEP } from '$lib/stores/uiZoom';
	import {
		videoFullscreenStyle,
		videoFullscreenGrowMs,
		videoFullscreenDimMs,
		VIDEO_FULLSCREEN_STYLES,
		VIDEO_FULLSCREEN_GROW_MIN,
		VIDEO_FULLSCREEN_GROW_MAX,
		VIDEO_FULLSCREEN_DIM_MIN,
		VIDEO_FULLSCREEN_DIM_MAX,
		type VideoFullscreenStyle,
	} from '$lib/stores/video_fullscreen_style';
	import { hasNativeVideoFullscreen } from '$lib/tauri/video_fullscreen';
	import { playerPlacement, type PlayerPlacement } from '$lib/stores/playerLayout';
	import { playerArtworkStyle, type PlayerArtworkStyle } from '$lib/stores/playerArtwork';
	import { bottomQualityDisplay, sideQualityDisplay, type QualityDisplay } from '$lib/stores/playerInformation';
	import { surfaceMode, type SurfaceMode } from '$lib/stores/surfaceMode';
	import { horizontalShelfWheel } from '$lib/stores/shelf_scrolling';
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
	import { isTauri } from '$lib/util/external';
	import {
		pendingTidalLogin, startTidalLogin, completeTidalLogin as finishTidalLogin,
		openTidalVerifyUrl, pasteTidalRedirectUrl, cancelTidalLogin
	} from '$lib/stores/tidalLogin';
	import { cachedApi } from '$lib/cache/api_queries';
	import { dataCache } from '$lib/cache/query';
	import {
		browserUpdateState,
		loadingDesktopUpdateState,
		unavailableDesktopUpdateState,
		type DesktopUpdateInfo
	} from '$lib/desktop/update_state';

	const SERVER_UNREACHABLE_MESSAGE =
		'NOOR cannot reach the local server, so it cannot verify your current TIDAL session.';
	const QUALITY_DISPLAY_OPTIONS: { id: QualityDisplay; label: string }[] = [
		{ id: 'off', label: 'Hidden' },
		{ id: 'icon', label: 'Icon' },
		{ id: 'details', label: 'Details' },
		{ id: 'both', label: 'Both' }
	];
	const APP_VERSION = String(import.meta.env.NOOR_APP_VERSION ?? '0.0.0');
	const DISCOVERY_COMPLETION_REFRESH_DELAY_MS = 1000;
	const DISCOVERY_COMPLETION_REFRESH_MAX_ATTEMPTS = 12;
	type BadgeTone = 'default' | 'active' | 'success' | 'warning' | 'error' | 'muted';

	let serverStatus = $state<'checking' | 'online' | 'offline'>('checking');
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
	let updateError = $state('');

	let mbStatus = $state<'idle' | 'running' | 'done'>('idle');
	let mbLiveProgress = $state<number | null>(null);
	let mbProgressLabel = $state('');
	let mbStats = $state<MusicBrainzStatus | null>(null);
	let portableSnapshot = $state<PortableMusicBrainzSnapshotStatus | null>(null);
	let discoveryStatus = $state<DiscoveryStatus | null>(null);
	let discoveryStatusLastTrainedAt = $derived(discoveryLastTrainedAt(discoveryStatus));
	let discoveryStatusModelHeldBack = $derived(discoveryModelHeldBack(discoveryStatus));
	let discoveryUpgrade = $state<DiscoveryUpgrade | null>(null);
	let discoveryUpgradeText = $derived(describeDiscoveryUpgrade(discoveryUpgrade));
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
	let lastfmStatusKnown = $state(false);
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
			const pending = await invoke<DesktopUpdateInfo | null>('get_update_state');
			updateAvailableVersion = pending?.version ?? null;
			updateStatus = pending ? `v${pending.version} available` : 'Up to date';
		} catch (err) {
			const unavailableState = unavailableDesktopUpdateState(appVersion, err);
			installModeLabel = unavailableState.installModeLabel;
			updateStatus = unavailableState.updateStatus;
			updateAvailableVersion = unavailableState.updateAvailableVersion;
			updateError = unavailableState.updateError;
		}
	}

	async function setupDesktopUpdateListeners(unlisteners: Array<() => void>) {
		if (!isTauri()) return;
		try {
			const { listen } = await import('@tauri-apps/api/event');
			const unlistenAvailable = await listen<DesktopUpdateInfo>('update-available', (event) => {
				updateAvailableVersion = event.payload.version;
				updateStatus = `v${event.payload.version} available`;
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
			const update = await invoke<DesktopUpdateInfo | null>('check_for_updates_now');
			updateAvailableVersion = update?.version ?? null;
			updateStatus = update ? `v${update.version} available` : 'Up to date';
		} catch (err) {
			updateError = err instanceof Error ? err.message : String(err);
			updateStatus = 'Update check failed';
		} finally {
			updateChecking = false;
		}
	}

	async function openPatchInfoFromSettings() {
		if (!desktopAppAvailable || !updateAvailableVersion) return;
		const { emit } = await import('@tauri-apps/api/event');
		await emit('open-update-details');
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
			errorMsg = 'Could not open the folder picker. Please try again.';
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

	// Settings > Playback > Transitions. Crossfade is the plain fade between
	// tracks; DJ plans its own overlaps, so it applies when DJ is off. The DJ
	// transition style is DJ policy and lives on the Mix page.
	const CROSSFADE_OPTIONS = [
		{ value: '0', label: 'Off' },
		{ value: '2000', label: '2s' },
		{ value: '5000', label: '5s' },
		{ value: '8000', label: '8s' },
		{ value: '12000', label: '12s' },
	] as const;
	// A stored crossfade that is not one of the steps shows as the nearest step.
	let crossfadeStep = $derived(
		CROSSFADE_OPTIONS.reduce<(typeof CROSSFADE_OPTIONS)[number]>((best, option) =>
			Math.abs(Number(option.value) - $crossfadeMs) < Math.abs(Number(best.value) - $crossfadeMs) ? option : best,
		CROSSFADE_OPTIONS[0]).value,
	);

	// Artwork cache (Settings > Library): covers and artist photos saved on
	// disk next to the database. Older servers lack the endpoint; the row hides.
	let artworkCache = $state<ArtworkCacheSettings | null>(null);
	async function loadArtworkCache() {
		try {
			artworkCache = await api.getArtworkCache();
		} catch {
			artworkCache = null;
		}
	}
	async function setArtworkCacheSize(value: string) {
		try {
			artworkCache = await api.setArtworkCacheSize(Number(value));
		} catch (err) {
			console.error('Failed to save the artwork cache size:', err);
		}
	}
	// Older servers lack the endpoint: the row stays visible but disabled.
	function artworkCacheHint(cache: ArtworkCacheSettings | null): string {
		const base = 'Covers and artist photos you have seen are kept on disk so they load instantly; the oldest are dropped first.';
		if (!cache) return base;
		if (cache.max_mb === 0) return `${base} Off: pictures load from TIDAL each time.`;
		return `${base} Using ${Math.round(cache.used_bytes / 1048576)} MB of ${cacheSizeLabel(cache.max_mb)}.`;
	}
	function cacheSizeLabel(mb: number): string {
		return mb === 0 ? 'Off' : mb >= 1000 ? `${mb / 1000} GB` : `${mb} MB`;
	}

	onMount(() => {
		if ($pendingTidalLogin) activeCategory = 'services';
		void loadArtworkCache();
		const tauriUnlisteners: Array<() => void> = [];
		void refreshDownloadFolder();
		const tick = setInterval(() => {
			nowEpochSeconds = Math.floor(Date.now() / 1000);
		}, 1000);
		const discoveryTrainingPoll = setInterval(() => {
			// Paused while compacting: the database is held for the duration, so
			// these would only pile up blocked requests behind the VACUUM.
			if (discoveryIsRunning && !databaseCompacting) void loadDiscoveryStatus();
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
				discoveryStatus = applyTrainingProgress(discoveryStatus, latest);
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
		untrack(() => void loadVisibleSettingsCategory());
		const cancelBackgroundSettingsLoad = scheduleSettingsBackgroundLoad();
		void loadDesktopAppInfo();
		void setupDesktopUpdateListeners(tauriUnlisteners);
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
		// Compacting holds the database for minutes, so every other poll blocks or
		// times out. Without this guard the app would declare the server dead
		// mid-VACUUM, which is exactly when a user is most likely to force-quit and
		// leave a half-rewritten file behind.
		if (databaseCompacting) return;
		serverStatus = 'offline';
	}

	async function connectTidal() {
		errorMsg = '';
		try {
			await startTidalLogin();
			markServerOnline();
		} catch (e) {
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
				errorMsg = `Failed to connect: ${e}`;
			}
		}
	}

	async function completeTidalLogin() {
		errorMsg = '';
		try {
			await finishTidalLogin();
			markServerOnline();
		} catch (e) {
			if (isFetchConnectionError(e)) {
				markServerOffline();
				errorMsg = SERVER_UNREACHABLE_MESSAGE;
			} else {
				markServerOnline();
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

	let syncPreferencesBusy = $state(false);
	async function toggleAutoSync() {
		if (syncPreferencesBusy || !$syncInfo) return;
		syncPreferencesBusy = true; errorMsg = '';
		const current = $syncInfo?.auto_sync_daily ?? false;
		if (!await setAutoSyncDaily(!current)) errorMsg = 'Daily sync could not be saved. Please retry.';
		syncPreferencesBusy = false;
	}

	async function toggleSyncEnrichment() {
		if (syncPreferencesBusy || !$syncInfo) return;
		syncPreferencesBusy = true; errorMsg = '';
		const current = $syncInfo?.enrich_from_favorite_albums ?? true;
		if (!await setSyncEnrichment(!current)) errorMsg = 'Sync options could not be saved. Please retry.';
		syncPreferencesBusy = false;
	}

	let recleanRunning = $state(false);
	let recleanSummary = $state('');

	async function handleReclean() {
		if (recleanRunning) return;
		const confirmed = confirm(
			'Reclean library from likes?\n\n' +
				'Non-liked tracks from favorited albums move to the hidden discovery pool ' +
				'(they keep feeding radio and recommendations), and exact duplicate copies ' +
				'are merged. Liked songs are never removed.'
		);
		if (!confirmed) return;
		recleanRunning = true;
		recleanSummary = '';
		const summary = await recleanLibrary();
		recleanRunning = false;
		if (!summary) {
			recleanSummary = 'Reclean failed or is already running - try again in a moment.';
			return;
		}
		const merged =
			summary.merged_groups > 0
				? `, merged ${summary.merged_groups} duplicate ${summary.merged_groups === 1 ? 'group' : 'groups'} (${summary.removed_tracks} copies removed)`
				: ', no duplicates merged';
		const review =
			summary.skipped_groups > 0
				? `; ${summary.skipped_groups} version ${summary.skipped_groups === 1 ? 'group' : 'groups'} left for review on the Duplicates page`
				: '';
		recleanSummary = `Moved ${summary.demoted.toLocaleString()} album tracks to the discovery pool${merged}${review}.`;
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
		try {
			const resp = await authFetch(`${getApiBase()}/api/tidal/logout`, { method: 'POST' });
			markServerOnline();
			if (!resp.ok) throw new Error(`Server returned ${resp.status}`);
			tidalStatus.set('disconnected');
			tidalUserId.set('');
			cancelTidalLogin();
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
			api.getLastfmStatus(),
			authFetch(`${getApiBase()}/api/library/enrich/lastfm/status`).then(async response => {
				if (!response.ok) throw new Error('Tag status unavailable');
				return response.json();
			})
		]);
		if (configResp.status === 'fulfilled') {
			markServerOnline();
			lastfmConfigured = configResp.value.enrichment === true;
		} else if (isFetchConnectionError(configResp.reason)) {
			markServerOffline();
		}

		if (enrichResp.status === 'fulfilled') {
			const data2 = enrichResp.value;
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
		lastfmStatusKnown = configResp.status === 'fulfilled' && enrichResp.status === 'fulfilled';
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

	// Always reads through the uncached client. This is a live status poll driven
	// by start/stop clicks and the completion watcher; served from the 30s query
	// cache it reported the PREVIOUS run's terminal status right after a start,
	// which is what hid the Stop button for the whole run.
	async function loadDiscoveryStatus() {
		void api
			.getDiscoveryUpgrade()
			.then((upgrade) => (discoveryUpgrade = upgrade))
			.catch(() => (discoveryUpgrade = null));
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

	let databaseStats = $state<DatabaseStats | null>(null);
	let databaseCompacting = $state(false);
	let databaseCompactResult = $state('');
	let databaseCompactError = $state('');

	function formatBytes(bytes: number): string {
		if (!Number.isFinite(bytes) || bytes <= 0) return '0 MB';
		const gb = bytes / 1024 ** 3;
		if (gb >= 1) return `${gb.toFixed(1)} GB`;
		return `${Math.max(1, Math.round(bytes / 1024 ** 2))} MB`;
	}

	async function loadDatabaseStats() {
		try {
			databaseStats = await api.getDatabaseStats();
			markServerOnline();
		} catch (error) {
			if (isFetchConnectionError(error)) markServerOffline();
		}
	}

	async function compactDatabase() {
		databaseCompacting = true;
		databaseCompactError = '';
		databaseCompactResult = '';
		try {
			const result = await api.compactDatabase();
			databaseCompactResult =
				result.reclaimed_bytes > 0
					? `Reclaimed ${formatBytes(result.reclaimed_bytes)} (now ${formatBytes(result.after_bytes)}).`
					: `Nothing to reclaim; the database is already compact (${formatBytes(result.after_bytes)}).`;
			await loadDatabaseStats();
		} catch (error) {
			if (isFetchConnectionError(error)) {
				markServerOffline();
				databaseCompactError = SERVER_UNREACHABLE_MESSAGE;
			} else {
				databaseCompactError = `Compacting failed: ${error}`;
			}
		} finally {
			databaseCompacting = false;
		}
	}

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
				radioSimilarityLabel = 'Status not confirmed. Refresh to check the last successful build.';
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


	const INTENSITY_PRESETS: Record<
		'max' | 'medium' | 'low',
		{ title: string; tagline: string; detail: string; spec: string }
	> = {
		max: {
			title: 'Max',
			tagline: 'Best suggestions. Slowest training.',
			detail:
				'The most connections per track. Best for libraries under about 10k tracks, or overnight runs.',
			spec: '96 dimensions, 64 neighbors per track, 8-track listening window.',
		},
		medium: {
			title: 'Medium',
			tagline: 'Balanced. The default.',
			detail: 'Close to Max for everyday listening, in about half the time.',
			spec: '64 dimensions, 32 neighbors per track, 5-track listening window.',
		},
		low: {
			title: 'Low',
			tagline: 'Fastest. Listening history only.',
			detail:
				'About a quarter of the time of Max. Skips audio matching, so tracks you have never played get weaker suggestions.',
			spec: '48 dimensions, 24 neighbors per track, 3-track listening window, no audio stage.',
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
		$pendingTidalLogin
			? 'Authorizing TIDAL'
			: serverStatus === 'offline'
				? 'TIDAL unknown'
				: $tidalStatus === 'connected'
					? 'TIDAL connected'
					: 'TIDAL offline'
	);
	let tidalBadgeTone = $derived<BadgeTone>(
		$pendingTidalLogin
			? 'active'
			: serverStatus === 'offline'
				? 'warning'
				: $tidalStatus === 'connected'
					? 'success'
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
	let portableSnapshotCopy = $derived(portableSnapshot?.exists ? 'Copy this enrichment folder to the same path on the other machine, then import it.' : 'Export enrichment for transfer. This is not a library backup.');

	// ─── Category rail ───────────────────────────────────────────────────
	// Splits the previously stacked panels into focused pages. Each panel
	// belongs to exactly one category; empty columns are hidden by CSS.
	type SettingsCategory = SettingsCategoryId;
	let activeCategory = $state<SettingsCategory>('appearance');
	let handledTidalLoginRequest = $state('');
	$effect(() => {
		const destination = resolveSettingsLocation(page.url);
		activeCategory = destination.category;
		if (typeof window === 'undefined') return;
		untrack(() => void loadVisibleSettingsCategory());
		if (destination.entry) void revealSetting(destination.entry);
		if (page.url.searchParams.get('tidalLogin') !== '1' || handledTidalLoginRequest === page.url.href) return;
		handledTidalLoginRequest = page.url.href;
		activeCategory = 'services';
		void goto(settingsHref('services', 'connect-tidal'), { replaceState: true, noScroll: true });
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
		{ value: 'LOSSLESS', label: 'Lossless' },
		{ value: 'HI_RES_LOSSLESS', label: 'Hi-Res Lossless' }
	];

	const VIDEO_QUALITY_OPTIONS: { value: VideoQualityMode; label: string }[] = [
		{ value: 'MAX', label: 'Highest' },
		{ value: 'AUTO', label: 'Auto' }
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
		if (activeCategory === 'library') {
			void loadMbStatus();
			void loadPortableSnapshot();
			void loadRadioSimilarityStatus();
			void loadLastfmStatus();
			void loadDatabaseStats();
			return;
		}
		if (activeCategory === 'playback') {
			void loadPlaybackRuntime();
			void loadAudioStats();
			void syncAnalysisStatus();
			void loadPassiveDspState();
			void loadAudioOutput();
			return;
		}
		if (activeCategory === 'services') {
			void loadLastfmStatus();
		}
	}

	function selectSettingsCategory(category: SettingsCategory) {
		activeCategory = category;
		void goto(settingsHref(category), { noScroll: true, keepFocus: true });
	}
	let settingsQuery = $state('');
	let searchFocused = $state(false);
	let searchSelected = $state(0);
	let searchMatches = $derived(searchSettings(settingsQuery));
	let searchAnnouncement = $state('');
	async function revealSetting(entry: SettingsSearchEntry) {
		await tick();
		const el = document.querySelector('[data-setting-id="' + (entry.target ?? entry.id) + '"]');
		if (!(el instanceof HTMLElement)) return;
		const requested = entry.focus ? el.querySelector(entry.focus) : el;
		for (let ancestor: HTMLElement | null = (requested ?? el) as HTMLElement; ancestor; ancestor = ancestor.parentElement) {
			if (ancestor instanceof HTMLDetailsElement) ancestor.open = true;
		}
		await tick();
		const focus = requested instanceof HTMLDetailsElement ? requested.querySelector('summary')
			: requested !== el ? requested : el.querySelector('input, select, button, summary, a');
		el.scrollIntoView({ behavior: prefersReducedMotion() ? 'instant' : 'smooth', block: 'center' });
		if (focus instanceof HTMLElement) focus.focus({ preventScroll: true });
		else { el.tabIndex = -1; el.focus({ preventScroll: true }); }
		searchAnnouncement = requested ? entry.label : entry.label + ': choose its prerequisite to make this control available.';
	}
	function jumpToSetting(entry: SettingsSearchEntry) {
		settingsQuery = ''; searchFocused = false; searchSelected = 0;
		void goto(settingsHref(entry.category, entry.id), { noScroll: true, keepFocus: true }).then(() => revealSetting(entry));
	}
	function searchKeydown(event: KeyboardEvent) {
		if (event.key === 'Escape') { settingsQuery = ''; searchFocused = false; return; }
		if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
			event.preventDefault();
			searchSelected = Math.max(0, Math.min(searchMatches.length - 1, searchSelected + (event.key === 'ArrowDown' ? 1 : -1)));
		} else if (event.key === 'Enter' && searchMatches.length) {
			event.preventDefault(); jumpToSetting(searchMatches[Math.min(searchSelected, searchMatches.length - 1)]);
		}
	}
	function changeAppearance(change: Partial<AppearanceValues>) {
		if (change.palette) setPalette(change.palette);
		if (change.theme) surfaceMode.set(change.theme);
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
	let bitPerfectActive = $derived(bitPerfectSettingsActive && $exclusiveStatus.engaged && !djProcessingActive);

	function setOutputPreset(enable: boolean) {
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

	// Desktop app only: browsers always use the Fullscreen API.
	const nativeVideoFullscreen = hasNativeVideoFullscreen();

	const settingsCategories = SETTINGS_CATEGORIES;

	let activeCategoryMeta = $derived(
		settingsCategories.find((category) => category.id === activeCategory) ?? settingsCategories[0]
	);

</script>

<svelte:head><title>Settings | NOORwave</title></svelte:head>
<div class="page-shell settings-page settings-scope">
<header class="settings-command"><h1 class="t-page-title">Settings</h1><div class="settings-search">
<input type="search" placeholder="Search settings" bind:value={settingsQuery} oninput={() => { searchSelected = 0; searchFocused = true; }} onfocus={() => searchFocused = true} onblur={() => setTimeout(() => searchFocused = false, 150)} onkeydown={searchKeydown} aria-label="Search settings" role="combobox" aria-autocomplete="list" aria-expanded={searchFocused && !!settingsQuery.trim()} aria-controls="settings-results" aria-activedescendant={searchFocused && searchMatches.length ? 'setting-result-' + Math.min(searchSelected, searchMatches.length - 1) : undefined} />
{#if searchFocused && settingsQuery.trim()}<ul id="settings-results" class="settings-search-results" role="listbox" aria-label="Matching settings">{#each searchMatches as match, index (match.id)}<li role="presentation"><button id={'setting-result-' + index} role="option" aria-selected={index === searchSelected} class:selected={index === searchSelected} onmousedown={(event) => event.preventDefault()} onclick={() => jumpToSetting(match)}><span>{match.label}</span><small>{categoryLabel(match.category)}</small></button></li>{/each}{#if !searchMatches.length}<li role="presentation">No matching settings.</li>{/if}</ul>{/if}
</div></header><span class="sr-only" role="status">{settingsQuery.trim() ? searchMatches.length + ' settings found' : searchAnnouncement}</span>
{#if errorMsg}<p class="error inline-notice" role="alert">{errorMsg}</p>{:else if serverStatus === 'offline'}<p class="error inline-notice" role="alert">Some settings could not be loaded. Check the server connection and retry.</p>{/if}
{#if discoverySafetyWatchdogRun}<div class="inline-notice" role="status"><strong>Discovery training paused for laptop safety.</strong><p>Try a lower intensity, keep your laptop plugged in, or run it later.</p><button class="btn btn-glass" onclick={() => dismissedSafetyRunId = discoverySafetyWatchdogRun?.id ?? null}>Dismiss</button></div>{/if}
<div class="settings-layout"><nav class="settings-rail" aria-label="Settings categories">{#each settingsCategories as cat}<a class:active={activeCategory === cat.id} href={settingsHref(cat.id)} aria-current={activeCategory === cat.id ? 'page' : undefined} onclick={(event) => { event.preventDefault(); selectSettingsCategory(cat.id); }}>{cat.label}</a>{/each}</nav><div class="settings-content"><h2>{activeCategoryMeta.label}</h2>
{#if activeCategory === 'appearance'}
<SettingGroup title="Interface"><AppearanceFields values={{ palette: $palette, theme: $surfaceMode, background: $wallpaper }} onchange={changeAppearance} showBackground={false} /><SettingRow label={'Interface size · ' + Math.round($uiZoom * 100) + '%'} id="interface-size"><div class="zoom-row">
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

					<button
						type="button"
						class="btn btn-glass btn-sm"
						onclick={resetZoom}
						aria-label="Reset interface size" title="Reset interface size" disabled={Math.abs($uiZoom - 1) < 1e-6}
					>↺</button>
				</div></SettingRow><details><summary>Keyboard shortcuts</summary><p class="setting-status">Ctrl + scroll or Ctrl + / − to resize; Ctrl + 0 to reset.</p></details><SettingRow label="Reduce motion" id="reduce-motion" hint="Turns off slides, scales and looping animation. Follow system uses your operating system's setting."><Segmented
					label="Reduce motion"
					options={[{ value: 'system', label: 'Follow system' }, { value: 'reduce', label: 'Always' }]}
					value={$motionPreference}
					onchange={(value) => motionPreference.set(value)}
				/></SettingRow></SettingGroup><SettingGroup title="Player"><SettingRow label="Position" id="player-position"><Segmented
					label="Preferred player position"
					options={[{ value: 'right', label: 'Right' }, { value: 'left', label: 'Left' }, { value: 'bottom', label: 'Bottom' }]}
					value={$playerPlacement}
					onchange={(value) => playerPlacement.set(value as PlayerPlacement)}
				/></SettingRow><SettingRow label="Side artwork" id="player-artwork"><Segmented
					label="Side player artwork style"
					options={[{ value: 'square', label: 'Square' }, { value: 'banner', label: 'Banner' }, { value: 'slim', label: 'Slim' }]}
					value={$playerArtworkStyle}
					onchange={(value) => playerArtworkStyle.set(value as PlayerArtworkStyle)}
				/></SettingRow><div data-setting-id="player-information"><SettingRow label="Side quality"><Segmented
					label="Side panel streaming quality"
					options={QUALITY_DISPLAY_OPTIONS.map((option) => ({ value: option.id, label: option.label }))}
					value={$sideQualityDisplay}
					onchange={(value) => sideQualityDisplay.set(value)}
				/></SettingRow><SettingRow label="Bottom quality"><Segmented
					label="Bottom player streaming quality"
					options={QUALITY_DISPLAY_OPTIONS.map((option) => ({ value: option.id, label: option.label }))}
					value={$bottomQualityDisplay}
					onchange={(value) => bottomQualityDisplay.set(value)}
				/></SettingRow></div></SettingGroup><SettingGroup title="Interaction"><div data-setting-id="horizontal-shelves"><div class="info-list">
					<div class="info-row">
						<div>
							<span>Scroll shelves with mouse wheel</span>

						</div>
						<strong>
							<Toggle
								checked={$horizontalShelfWheel}
								label="Use vertical wheel to browse horizontal shelves"
								onchange={(event) => horizontalShelfWheel.set(event.currentTarget.checked)}
							/>
						</strong>
					</div>
				</div></div></SettingGroup><section class="glass-tile section-panel" data-setting-id="background"><SectionHeader title="Background" /><div class="info-row"><span>Selected background</span><strong>{WALLPAPERS.find((item) => item.id === $wallpaper)?.label ?? "Off"}</strong></div><details><summary>Choose background</summary><div class="wallpaper-big-preview">
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
							<span>Focus or hover a background to preview</span>
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
								onpointerenter={() => onTileEnter(wallpaperNone!)} onfocus={() => onTileEnter(wallpaperNone!)}
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

							</button>
							{#if openGroups[group.key]}
								<div class="wallpaper-grid">
									{#each group.options as option (option.id)}
										<button
											type="button"
											class="wallpaper-tile"
											class:active={$wallpaper === option.id}
											class:previewing={previewTileId === option.id}
											onclick={() => { onTileEnter(option); setWallpaper(option.id); }}
											aria-pressed={$wallpaper === option.id}
											onpointerenter={() => onTileEnter(option)} onfocus={() => onTileEnter(option)}
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

				</details><details><summary>Background settings</summary><div class="wallpaper-tune">
					<div class="wallpaper-group">
						<div class="wallpaper-group-head">
							<h4>Reacts to music</h4>

						</div>

						<div class="wallpaper-control">
							<span>
								<strong>Beat reactivity</strong>

							</span>
							<div class="wallpaper-control-field">
								<Toggle label="Beat reactivity"
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
								<strong>Background motion</strong>
								<small>Calms the reaction. Auto follows Reduce motion above.</small>
							</span>
							<div class="wallpaper-control-field">
								<select
									class="audio-select"
									value={$wallpaperReduceMotion}
									onchange={(e) => setWallpaperReduceMotion((e.currentTarget as HTMLSelectElement).value as WallpaperReduceMotion)}
									aria-label="Background motion"
								>
									<option value="auto">Auto</option>
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
								<strong>Blur</strong>
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
									aria-label="Background blur"
								/>
								<output>{$wallpaperBlur}px</output>
							</div>
						</label>

						<label class="wallpaper-control">
							<span>
								<strong>Frame rate</strong>
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
									aria-label="Frame rate"
								/>
								<output>{$wallpaperFps} FPS</output>
							</div>
						</label>
					</div>
				</div></details></section>
{:else if activeCategory === 'playback'}
<section data-setting-id="playback-output" class="glass-tile section-panel">
				<SectionHeader title="Playback output" />
				{#if $audioSettings.settings}
					{@const s = $audioSettings.settings}
					{#if isWindows}<label class="audio-field"><span>Output preset</span><select aria-label="Output preset" value={bitPerfectSettingsActive ? 'bit-perfect' : !s.exclusive_mode && !s.sample_rate_follow && s.quality === 'LOSSLESS' ? 'shared' : 'custom'} onchange={(event) => setOutputPreset(event.currentTarget.value === 'bit-perfect')} disabled={$audioSettings.pendingApply}><option value="shared">Shared</option><option value="bit-perfect">Bit-perfect</option><option value="custom" disabled>Custom</option></select></label><p class="setting-status">Presets change stream quality, exclusive access and source-rate matching.</p>{/if}<p class="setting-status">{bitPerfectActive ? 'Exclusive output engaged; DJ processing off' : s.exclusive_mode && djProcessingActive ? 'DJ processing active' : s.exclusive_mode && !$exclusiveStatus.engaged ? 'Exclusive output requested; engagement not confirmed' : 'Shared output'}</p><div class="audio-field-grid">
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
								{/if}<details class="audio-advanced">
							<summary>
								<span>Advanced output</span>

							</summary>
							<div class="info-list">
								<div class="info-row">
									<span>Exclusive output (WASAPI)</span>
									<strong>
										<Toggle label="Exclusive output (WASAPI)"
											checked={s.exclusive_mode}
											onchange={onAudioExclusiveToggle}
										/>
									</strong>
								</div>
								<p class="page-copy setting-caption">
									Takes over the device while playing. Crossfade is bypassed; prepared same-rate tracks still hand off gaplessly.
								</p>

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
											class="exclusive-grace-slider" aria-label="Idle release" disabled={s.exclusive_release_on_pause}
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
										<Toggle label="Release on pause"
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
										<Toggle label="Sample rate follows source"
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
								{/if}<details class="audio-advanced">
						<summary>
							<span>Video playback</span>

						</summary>
						<label class="audio-field audio-field-single">
							<span>Video quality</span>
							<select
								class="audio-select"
								aria-label="Video quality" value={s.video_quality_mode}
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
						{#if nativeVideoFullscreen}
							<label class="audio-field audio-field-single">
								<span>Fullscreen transition</span>
								<select
									class="audio-select"
									aria-label="Fullscreen transition"
									value={$videoFullscreenStyle}
									onchange={(e) => videoFullscreenStyle.set((e.currentTarget as HTMLSelectElement).value as VideoFullscreenStyle)}
								>
									{#each VIDEO_FULLSCREEN_STYLES as opt (opt.value)}
										<option value={opt.value}>{opt.label}</option>
									{/each}
								</select>
							</label>
							<p class="page-copy setting-caption">
								Grow takes the window fullscreen, then grows the video into it. Dim, then grow darkens the page first. Classic is the window's own fullscreen.
							</p>
							<label class="wallpaper-control">
								<span>
									<strong>Grow speed</strong>
									<small>How long the video takes to fill the screen and shrink back.</small>
								</span>
								<div class="wallpaper-control-field">
									<input
										type="range"
										min={VIDEO_FULLSCREEN_GROW_MIN}
										max={VIDEO_FULLSCREEN_GROW_MAX}
										step="10"
										value={$videoFullscreenGrowMs}
										oninput={(e) => videoFullscreenGrowMs.set(parseInt((e.currentTarget as HTMLInputElement).value, 10))}
										aria-label="Fullscreen grow speed"
									/>
									<output>{$videoFullscreenGrowMs} ms</output>
								</div>
							</label>
							{#if $videoFullscreenStyle === 'dim'}
								<label class="wallpaper-control">
									<span>
										<strong>Dim length</strong>
										<small>How long the page takes to fade to black first.</small>
									</span>
									<div class="wallpaper-control-field">
										<input
											type="range"
											min={VIDEO_FULLSCREEN_DIM_MIN}
											max={VIDEO_FULLSCREEN_DIM_MAX}
											step="10"
											value={$videoFullscreenDimMs}
											oninput={(e) => videoFullscreenDimMs.set(parseInt((e.currentTarget as HTMLInputElement).value, 10))}
											aria-label="Fullscreen dim length"
										/>
										<output>{$videoFullscreenDimMs} ms</output>
									</div>
								</label>
							{/if}
						{/if}
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
			</section><section class="glass-tile section-panel" data-setting-id="transitions"><SectionHeader title="Transitions" />
				<SettingRow label="Crossfade" hint="How long one track fades into the next when DJ transitions are off.">
					<Segmented label="Crossfade" options={CROSSFADE_OPTIONS} value={crossfadeStep} onchange={(value) => void setPlayerCrossfadeMs(Number(value))} />
				</SettingRow>
				<p class="setting-status">DJ transition style, mix intent and speed are on the <a href="/mix">Mix page</a>.</p>
			</section><section class="glass-tile section-panel" data-setting-id="library-audio-data"><SectionHeader title="Analysis" />
<SettingRow label="Analyse while playing" hint="Save BPM, key and energy as tracks play for DJ transitions and harmonic shuffle. Turning this off keeps existing analysis."><Toggle label="Analyse while playing" checked={$audioAnalysis.passiveEnabled} disabled={!$passiveDspKnown || $passiveDspPending} onchange={(event) => void setPassiveDspEnabled(event.currentTarget.checked)} /></SettingRow>
{#if $audioAnalysisError}<p class="error" role="alert">{$audioAnalysisError}</p><button class="btn btn-glass" onclick={() => void loadPassiveDspState()} disabled={$passiveDspPending}>Retry setting</button>{/if}
<details><summary>Analysis statistics</summary>
<div class="info-row"><span>Analysed tracks</span><strong>{$audioAnalysis.stats ? $audioAnalysis.analyzed.toLocaleString() : '—'}</strong></div>
<div class="info-row"><span>Average BPM</span><strong>{$audioAnalysis.stats?.avg_bpm?.toFixed(1) ?? '—'}</strong></div>
<div class="info-row"><span>Most common key</span><strong>{$audioAnalysis.stats?.top_key ?? '—'}</strong></div>
<div class="info-row"><span>Average energy</span><strong>{$audioAnalysis.stats?.avg_energy?.toFixed(2) ?? '—'}</strong></div></details></section><details data-setting-id="now-playing-path" class="glass-tile section-panel"><summary>Output details</summary><div class="info-list">
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
				{/if}</details>{#if playbackRuntime?.last_error}<p class="error" role="alert">{playbackRuntime.last_error}</p>{/if}
{:else if activeCategory === 'library'}
<SettingGroup title="Songs"><SettingRow label="Songs tab shows" id="library-songs-scope" hint="Just the songs you've liked, or every song in your library, including tracks from saved albums and local imports."><Segmented
	label="Songs tab shows"
	options={[{ value: 'liked', label: 'Liked songs' }, { value: 'library', label: 'All library songs' }]}
	value={$librarySongsScope}
	onchange={(value) => librarySongsScope.set(value)}
/></SettingRow></SettingGroup>
<SettingGroup title="Videos"><SettingRow label="Saving a video likes its song" id="library-video-likes-song" hint="When you save a music video, the matching song (same artist and title) is liked too, so it shows in your Library and TIDAL favorites. Removing a video never unlikes the song."><Segmented
	label="Saving a video likes its song"
	options={[{ value: 'off', label: 'Off' }, { value: 'on', label: 'On' }]}
	value={$likeSongOnVideoSave}
	onchange={(value) => likeSongOnVideoSave.set(value)}
/></SettingRow></SettingGroup>
<SettingGroup title="Artwork"><SettingRow label="Artwork cache" id="library-artwork-cache" hint={artworkCacheHint(artworkCache)}><Dropdown
	label="Artwork cache"
	options={(artworkCache?.options_mb ?? [0, 100, 150, 250, 500, 1000]).map((mb) => ({ value: String(mb), label: cacheSizeLabel(mb) }))}
	value={String(artworkCache?.max_mb ?? 150)}
	disabled={artworkCache === null}
	onchange={(value) => void setArtworkCacheSize(value)}
/></SettingRow></SettingGroup>
<section data-setting-id="library-sync" class="glass-tile section-panel"><SectionHeader title="Sync" />{#if $tidalStatus === "connected"}					<div class="info-list">

						<div class="info-row">
							<span>Last sync</span>
							<strong>
								{#if $syncStatus === 'syncing'}
									{$syncProgress ?? 0}% complete
								{:else if $syncStatus === 'error'}
									Failed
								{:else if $syncStatus === 'cancelled'}
									Cancelled
								{:else if $syncInfo?.last_sync_kind && $syncInfo.last_sync_at}
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
									{$syncInfo ? 'Never synced' : 'Status unavailable'}
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
							<div><span>Sync daily</span><p class="info-row-hint">Check for changes when NOORwave starts if the last sync is more than a day old.</p></div>
							<strong>
								<Toggle label="Sync daily" disabled={!$syncInfo || syncPreferencesBusy}
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

						{#if $syncStatus === 'syncing'}
							<button class="btn btn-glass" onclick={handleCancelSync}>Cancel</button>
						{/if}


					</div>
					<details><summary>Sync options</summary><div class="info-row">
							<div><span>Learn from favorite albums</span><p class="info-row-hint">Use other tracks on your favorite albums for discovery. They stay hidden from your library and Genre Galaxy unless you like them.</p></div>
							<strong>
								<Toggle label="Learn from favorite albums" disabled={!$syncInfo || syncPreferencesBusy}
									checked={$syncInfo?.enrich_from_favorite_albums ?? true}
									onchange={() => void toggleSyncEnrichment()}
								/>
							</strong>
						</div><div class="action-row"><button class="btn btn-glass" onclick={() => void syncLibrary('full')} disabled={$syncStatus === 'syncing'}>
							Full resync
						</button></div><p class="setting-status">A full resync checks your entire TIDAL collection; use it if a normal sync misses changes.</p></details>{#if recleanSummary}
						<p class="reclean-summary">{recleanSummary}</p>
					{/if}
				{:else}<p class="setting-status">Connect TIDAL to sync your library.</p><a class="btn btn-glass" href={settingsHref("services", "connect-tidal")}>Connect TIDAL</a>{/if}</section><section data-setting-id="musicbrainz-enrichment" class="glass-tile section-panel">
				<SectionHeader title="MusicBrainz genres" /><p class="setting-status">Fill missing genres from MusicBrainz for Genre Galaxy and genre-aware playback. Progress is saved between runs.</p>

				<div class="stat-grid inner-metrics">
					<div class="info-row"><span>Tagged</span><strong>{mbStats ? mbStats.enriched_tracks.toLocaleString() : '—'}</strong></div>
					<div class="info-row"><span>Remaining</span><strong>{mbStats ? mbStats.remaining.toLocaleString() : '—'}</strong></div>
				</div>

				{#if mbStatus === 'running'}<div class="enrichment-progress">
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
				</div>{/if}

				<div class="action-row">
					<button class="btn btn-primary" onclick={startEnrichment} disabled={mbStatus === 'running' || mbStats?.remaining === 0}>
						{mbStatus === 'running'
							? 'Running…'
							: mbStats?.remaining === 0
								? 'All enriched'
								: mbStats && mbStats.checked_tracks > 0
									? 'Resume enrichment'
									: 'Enrich genres'}
					</button>
					<button class="btn btn-glass" onclick={refreshGalaxy}>Refresh genre galaxy</button>
				</div>
				{#if galaxyRefreshLabel}
					<p class="galaxy-refresh-label">{galaxyRefreshLabel}</p>
				{/if}
			</section><section data-setting-id="last-fm-tags" class="glass-tile section-panel"><SectionHeader title="Last.fm tags" /><p class="setting-status">Add community tags to liked tracks and albums. A Last.fm API key is enough; account approval is optional for tags.</p>{#if lastfmError}
					<p class="page-copy is-error" role="alert">{lastfmError}</p>
				{/if}

				{#if !lastfmStatusKnown}<p class="setting-status">Tag status unavailable.</p><button class="btn btn-glass" onclick={() => void loadLastfmStatus()}>Refresh status</button>{:else if !lastfmConfigured}<p class="setting-status">Configure Last.fm to enrich tags.</p><a class="btn btn-glass" href={settingsHref('services', 'lastfm-service')}>Set up Last.fm</a>{:else}
					<div class="stat-grid inner-metrics">
						<div class="info-row"><span>Tagged</span><strong>{lastfmEnrichedCount.toLocaleString()}</strong></div>
						<div class="info-row"><span>Checked</span><strong>{`${lastfmChecked.toLocaleString()} / ${lastfmTotal.toLocaleString()}`}</strong></div>
						<div class="info-row"><span>Remaining</span><strong>{lastfmRemaining.toLocaleString()}</strong></div>
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
									{lastfmRemaining.toLocaleString()} favorited tracks pending. Estimated {lastfmEtaLabel}. Enrichment continues in the background while NOORwave is running.
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


					</div>
				{/if}</section><section data-setting-id="downloads" class="glass-tile section-panel">
				<SectionHeader title="Downloads" />

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
								FLAC
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultDownloadFormat === 'aac'}
								aria-pressed={$defaultDownloadFormat === 'aac'}
								onclick={() => setDownloadFormat('aac')}
							>
								AAC
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultDownloadFormat === 'mp3'}
								aria-pressed={$defaultDownloadFormat === 'mp3'}
								onclick={() => setDownloadFormat('mp3')}
							>
								MP3
							</button>
						</div>
					</div>

					{#if $defaultDownloadFormat === 'flac'}<div class="audio-field">
						<span>FLAC quality</span>
						<div class="download-format-toggle" role="group" aria-label="FLAC download quality">
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultFlacQuality === 'hires'}
								aria-pressed={$defaultFlacQuality === 'hires'}
								onclick={() => setFlacQuality('hires')}
							>
								Hi-Res
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultFlacQuality === 'cd'}
								aria-pressed={$defaultFlacQuality === 'cd'}
								onclick={() => setFlacQuality('cd')}
							>
								CD
							</button>
						</div>
					</div>{/if}

					{#if $defaultDownloadFormat === 'mp3'}<div class="audio-field">
						<span>MP3 source</span>
						<div class="download-format-toggle" role="group" aria-label="MP3 transcode source">
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultMp3Source === 'aac'}
								aria-pressed={$defaultMp3Source === 'aac'}
								onclick={() => setMp3Source('aac')}
							>
								AAC
							</button>
							<button
								type="button"
								class="download-format-option"
								class:active={$defaultMp3Source === 'lossless'}
								aria-pressed={$defaultMp3Source === 'lossless'}
								onclick={() => setMp3Source('lossless')}
							>
								Lossless
							</button>
						</div>
					</div>{/if}


				</div>
			</section><details data-setting-id="discovery-engine" class="glass-tile section-panel" open={discoveryIsRunning}><summary>Discovery<span class="disclosure-status">{discoveryIsRunning ? "Training - " + Math.round((discoveryStatus?.latest_run?.progress ?? 0) * 100) + "%" : discoveryUpgrade?.pending ? "Upgrade pending" : discoveryStatus ? Math.round(discoveryStatus.coverage_ratio * 100) + "% coverage" : "Status unavailable"}</span></summary><p class="setting-status">Manual runs can use substantial CPU: check the estimate and safety profile first. You can stop any run. After an update that changes how recommendations learn, NOOR relearns once on its own at low priority.</p>

				<details class="discovery-guide"><summary>About discovery training</summary><p class="setting-status">Training finds connections across your library. Refresh after adding music or listening history; a full retrain also rebuilds cached audio features.</p><p class="setting-status">A finished run replaces the active model only if it predicts your listening at least as well; otherwise the current model stays in use.</p></details>

				<div class="stat-grid inner-metrics">
					<div class="info-row"><span>Coverage</span><strong>{discoveryStatus ? `${Math.round(discoveryStatus.coverage_ratio * 100)}%` : '-'}</strong></div>
					<div class="info-row"><span>Tracks learned</span><strong>{discoveryStatus?.embedded_tracks?.toLocaleString() ?? '0'}</strong></div>
				</div>

				<div class="portable-card">
					<div class="info-list">
						<div class="info-row">
							<span>Active model</span>
							<strong title={discoveryStatus?.active_model?.model_key}>{discoveryModelLabel(discoveryStatus?.active_model ?? null, discoveryUpgrade)}</strong>
						</div>
						<div class="info-row">
							<span>Last trained</span>
							<strong>{discoveryStatusLastTrainedAt ? new Date(discoveryStatusLastTrainedAt + 'Z').toLocaleString() : 'Never'}</strong>
						</div>
						<div class="info-row">
							<span>Audio features</span>
							<strong>{discoveryStatus?.clip_cache_tracks?.toLocaleString() ?? '0'}</strong>
						</div>
						<div class="info-row">
							<span>Latest run</span>
							<strong title={discoveryStatus?.latest_run?.error_text ?? undefined}>{describeDiscoveryRun(discoveryStatus?.latest_run)}</strong>
						</div>
						{#if discoveryUpgradeText}
							<div class="info-row">
								<span>Update</span>
								<strong>{discoveryUpgradeText}</strong>
							</div>
						{/if}
						{#if discoveryStatusModelHeldBack}
							<div class="info-row">
								<span>Newer model</span>
								<strong>Held back: it did not beat the active model</strong>
							</div>
						{/if}
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

					{#if !discoveryEngineTrainable}
						<div class="legacy-engine-note">
							V1 is read-only in this build. Switch to V2 to train or refresh discovery.
						</div>
					{/if}
				</div>

				<div class="intensity-block">
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
					<div class="intensity-header">
						<span class="intensity-eyebrow">Training intensity</span>
						<span class="intensity-tagline">{INTENSITY_PRESETS[discoveryIntensity].tagline}</span>
					</div>
					<div class="intensity-grid">
						{#each (['max', 'medium', 'low'] as const) as tier (tier)}
							<button
								type="button"
								class="intensity-option"
								class:selected={discoveryIntensity === tier}
								title={INTENSITY_PRESETS[tier].spec}
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
									Safe to run - about {formatDurationSeconds(safety.estimated_seconds)} expected.
								{:else if safety.recommendation === 'moderate'}
									Moderate cost - about {formatDurationSeconds(safety.estimated_seconds)} expected.
								{:else}
									Heavy run - about {formatDurationSeconds(safety.estimated_seconds)} expected. Consider Medium or Low.
								{/if}
							</div>
							<div class="safety-detail">
								<span>{safety.track_count.toLocaleString()} tracks</span>
								<span aria-hidden="true">|</span>
								<span>~{safety.estimated_ram_mb} MB peak RAM</span>
								<span aria-hidden="true">|</span>
								<span>{safety.worker_threads} worker{safety.worker_threads === 1 ? '' : 's'}</span>
								<span aria-hidden="true">|</span>
								<span>{formatDurationSeconds(safety.safety_timeout_seconds)} safety cap</span>
								{#if safety.last_run_seconds !== null}
									<span aria-hidden="true">|</span>
									<span>last run {formatDurationSeconds(safety.last_run_seconds)}</span>
								{/if}
							</div>
							{#if discoveryIsRunning && discoveryUpgrade?.running}
								<div class="safety-detail">
									This automatic run uses fewer threads at low priority, so it can take longer than the estimate.
								</div>
							{/if}
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
								{discoveryStageLabel(run.stage)} <span class="discovery-pct">{pct}%</span>
							</div>
						</div>
					{/if}
				</div>

				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void startDiscoveryTraining('incremental')} disabled={discoveryIsRunning || !discoveryEngineTrainable}>Refresh discovery</button>
					<button class="btn btn-glass" onclick={() => void startDiscoveryTraining('full', true)} disabled={discoveryIsRunning || !discoveryEngineTrainable}>Full retrain</button>
					{#if discoveryIsRunning}
						<button class="btn btn-glass" onclick={() => void stopDiscoveryTraining()}>Stop training</button>
					{/if}
				</div></details><details data-setting-id="radio-similarity-index" class="glass-tile section-panel" open={radioSimilarityBusy}><summary>Radio index<span class="disclosure-status">{radioSimilarityRowCount === null ? "Unknown" : radioSimilarityRowCount.toLocaleString() + " pairs"}</span></summary><p class="setting-status">A separate radio index. Automatic rebuilds wait while the app is busy.</p><details><summary>Automatic rebuilds</summary><p class="setting-status">Rebuilds may wait for playback, sync, enrichment, analysis or training. The scheduler checks hourly after a six-hour rebuild interval; startup checks are delayed.</p></details>
				<div class="info-list">
					<div class="info-row">
						<span>Indexed pairs</span>
						<strong>{radioSimilarityRowCount === null ? '—' : radioSimilarityRowCount.toLocaleString()}</strong>
					</div>
					<div class="info-row">
						<span>Last built</span>
						<strong>{radioSimilarityBuiltAt ?? (radioSimilarityRowCount === null ? 'Unknown' : 'Never')}</strong>
					</div>
				</div>
				<div class="action-row">
					<button class="btn btn-primary" onclick={() => void buildRadioSimilarity()} disabled={radioSimilarityBusy}>
						{radioSimilarityBusy ? 'Building…' : (radioSimilarityBuiltAt ? 'Rebuild radio index' : 'Build radio index')}
					</button>
				</div>
				<button class="btn btn-glass" onclick={() => void loadRadioSimilarityStatus()}>Refresh status</button>{#if radioSimilarityLabel}
					<p class="galaxy-refresh-label">{radioSimilarityLabel}</p>
				{/if}</details><details data-setting-id="portable-snapshot" class="glass-tile section-panel"><summary>Enrichment transfer</summary><div class="stat-grid inner-metrics">
					<div class="info-row"><span>Snapshot checked</span><strong>{portableSnapshot?.checked_rows?.toLocaleString() ?? '0'}</strong></div>
					<div class="info-row"><span>Snapshot genres</span><strong>{portableSnapshot?.genre_rows?.toLocaleString() ?? '0'}</strong></div>
					<div class="info-row"><span>Last.fm checked</span><strong>{portableSnapshot?.lastfm_checked_rows?.toLocaleString() ?? '0'}</strong></div>
					<div class="info-row"><span>Context tags</span><strong>{portableSnapshot?.context_tag_rows?.toLocaleString() ?? '0'}</strong></div>
				</div>

				<div class="portable-card">
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
						{portableAction === 'export' ? 'Exporting…' : 'Export enrichment'}
					</button>
					<button
						class="btn btn-glass"
						onclick={importPortableSnapshot}
						disabled={portableAction !== null || !portableSnapshot?.exists}
					>
						{portableAction === 'import' ? 'Importing…' : 'Import enrichment'}
					</button>
				</div></details><details class="glass-tile section-panel" data-setting-id="library-maintenance"><summary>Library management</summary>
<SettingRow label="Clean library" hint="Move non-liked album tracks to the hidden discovery pool and merge exact duplicates. Liked songs stay.">
	<button class="btn btn-glass" onclick={() => void handleReclean()} disabled={recleanRunning || $syncStatus === 'syncing'}>{recleanRunning ? 'Cleaning…' : 'Clean library'}</button>
</SettingRow>
<SettingRow label="Reset Last.fm tags" hint="Clear Last.fm tags and check markers so the next enrichment run fetches them again.">
	<button class="btn btn-glass danger" onclick={resetLastfmEnrichment} disabled={lastfmIsRunning}>Reset tags</button>
</SettingRow>
<SettingRow label="Clear audio analysis" hint="Remove saved BPM, key and energy analysis. Your music and listening history stay.">
	<button class="btn btn-glass danger" onclick={clearAllAnalysis}>Clear analysis</button>
</SettingRow>
{#if $audioAnalysisError}<p class="error" role="alert">{$audioAnalysisError}</p>{/if}{#if recleanSummary}<p class="setting-status">{recleanSummary}</p>{/if}{#if lastfmError}<p class="error" role="alert">{lastfmError}</p>{/if}<details data-setting-id="database-size" class="glass-tile section-panel" open={databaseCompacting}><summary>Database storage</summary>{#if databaseStats}
					<p class="page-copy database-size-headline">
						<strong>{formatBytes(databaseStats.file_bytes)}</strong>
						{#if databaseStats.estimated_reclaimable_bytes > 0}
							<span class="database-size-arrow" aria-hidden="true">-&gt;</span>
							<strong>~{formatBytes(databaseStats.estimated_after_bytes)}</strong>
							<span class="database-size-note">after compacting</span>
						{/if}
					</p>
					{#if databaseStats.estimated_reclaimable_bytes > 0}
						<p class="page-copy">
							About <strong>{formatBytes(databaseStats.estimated_reclaimable_bytes)}</strong> can be
							freed{#if databaseStats.retired_neighbor_rows > 0}, mostly
							{databaseStats.retired_neighbor_rows.toLocaleString()} leftover rows from
							{databaseStats.retired_models} superseded discovery model{databaseStats.retired_models === 1 ? '' : 's'}{/if}.
						</p>
					{:else}
						<p class="page-copy">Already compact - nothing worth reclaiming.</p>
					{/if}
					{#if databaseStats.wal_bytes > 0}
						<p class="page-copy">
							Write-ahead log <strong>{formatBytes(databaseStats.wal_bytes)}</strong> (folded back in
							when you compact).
						</p>
					{/if}
					<p class="page-copy is-warning">
						Compacting rewrites the entire file. It can take several minutes on a large library,
						needs about as much free disk as the database currently uses, and the app will be
						unresponsive while it runs. Everything keeps working without it - the space is reused
						internally either way.
					</p>
				{:else}
					<p class="page-copy">Reading database size…</p>
				{/if}
				{#if databaseCompactResult}
					<p class="page-copy">{databaseCompactResult}</p>
				{/if}
				{#if databaseCompactError}
					<p class="page-copy is-error" role="alert">{databaseCompactError}</p>
				{/if}
				<div class="action-row">
					<button class="btn btn-glass" onclick={() => void loadDatabaseStats()} disabled={databaseCompacting}>
						Refresh
					</button>
					<button
						class="btn btn-glass danger"
						onclick={compactDatabase}
						disabled={databaseCompacting || !databaseStats}
					>
						{databaseCompacting ? 'Compacting…' : 'Compact database'}
					</button>
				</div></details><details data-setting-id="clear-non-library-entries" class="glass-tile section-panel" open={purgeRunning}><summary>Remove unused recommendations</summary><p class="setting-status">Remove recommendations never played, liked, queued or added to playlists.</p>
				<p class="setting-status">Associated trained data is also removed. Review the confirmation before continuing.</p>
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
						{purgeRunning ? 'Purging…' : 'Remove unused recommendations'}
					</button>
				</div></details></details>
{:else if activeCategory === 'services'}
<section data-setting-id="connect-tidal" class="glass-tile section-panel"><SectionHeader title="TIDAL" />{#if $pendingTidalLogin}
					<div class="auth-card glass">
						<p class="page-copy">{$pendingTidalLogin.phase === 'starting' ? 'Opening TIDAL sign-in…' : 'Finish your TIDAL sign-in.'}</p>
						<p class="page-copy">After sign-in, copy the full address from the final TIDAL page, even if it says page not found. Paste it here to finish.</p>
						<div class="action-row">
							<button type="button" class="btn btn-glass" onclick={() => void openTidalVerifyUrl()} disabled={$pendingTidalLogin.phase !== 'awaiting'}>
								Open TIDAL sign-in
							</button>
						</div>
						{#if $pendingTidalLogin.externalOpenError}
							<p class="error" role="alert">{$pendingTidalLogin.externalOpenError}</p>
							<input class="text-field" type="url" readonly value={$pendingTidalLogin.verifyUrl} aria-label="TIDAL sign-in URL" />
						{/if}
						<input
							class="text-field"
							type="url"
							bind:value={$pendingTidalLogin.redirectUrl}
							disabled={$pendingTidalLogin.phase !== 'awaiting'}
							aria-label="Final TIDAL redirect URL"
							placeholder="https://tidal.com/android/login/auth?code=..."
						/>
						{#if $pendingTidalLogin.error}
							<p class="error" role="alert">{$pendingTidalLogin.error}</p>
						{/if}
						<div class="action-row">
							<button class="btn btn-glass" onclick={pasteTidalRedirectUrl} disabled={$pendingTidalLogin.phase !== 'awaiting'}>
								Paste from clipboard
							</button>
							<button class="btn btn-primary" onclick={completeTidalLogin} disabled={$pendingTidalLogin.phase !== 'awaiting' || !$pendingTidalLogin.redirectUrl.trim()}>
								{$pendingTidalLogin.phase === 'completing' ? 'Finishing login…' : 'Finish login'}
							</button>
							<button class="btn btn-glass" onclick={cancelTidalLogin} disabled={$pendingTidalLogin.phase !== 'awaiting'}>Cancel login</button>
						</div>
					</div>
				{:else if serverStatus === 'offline'}
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
				{:else}<div class="info-row"><span>Account</span><strong>{$tidalUserId ?? 'Connected'}</strong></div><div class="action-row"><button class="btn btn-glass" onclick={disconnectTidal}>Disconnect</button><a class="btn btn-glass" href={settingsHref('library', 'library-sync')}>Library sync</a></div>{/if}
<VideoDiscoverySetting />
<ExploreStationsSetting />
<details><summary>More content settings</summary>
    <TidalContentSetting />
	<p class="setting-status">Manage explicit content in the TIDAL app.</p>
	<div class="action-row"><ExternalLink href="https://support.tidal.com/hc/en-us/articles/48031883413521-AI-Policy">About TIDAL’s AI labels</ExternalLink><ExternalLink href="https://support.tidal.com/hc/en-us/articles/9936639051153-Explicit-Content">Explicit content settings</ExternalLink></div>
</details></section><IntegrationsPanel />
{:else if activeCategory === 'remote'}
<PhoneRemotePanel />
{:else if activeCategory === 'app'}
<StartupSetting /><CloseBehaviorSetting /><section data-setting-id="app-updates" class="glass-tile section-panel">
				<SectionHeader title="App updates" />
				<div class="inner-metrics">
					<div class="info-row"><span>Version</span><strong>{appVersion || 'Unknown'}</strong></div>
					<div class="info-row"><span>Install mode</span><strong>{installModeLabel}</strong></div>
					<div class="info-row"><span>Updates</span><strong>{updateStatus}</strong></div>
				</div>
				<div class="action-row">
					{#if updateAvailableVersion}
						<button
							type="button"
							class="btn btn-primary"
							onclick={() => void openPatchInfoFromSettings()}
						>
							View update details
						</button>
					{/if}
					<button
						type="button"
						class={updateAvailableVersion ? 'btn btn-glass' : 'btn btn-primary'}
						onclick={() => void checkForUpdatesNow()}
						disabled={!desktopAppAvailable || updateChecking}
					>
						{updateChecking ? 'Checking...' : 'Check for updates'}
					</button>

				</div>
				{#if updateError}
					<p class="field-error" role="alert">{updateError}</p>
				{/if}
			</section>
{/if}</div></div></div>
{#if databaseCompacting}
	<!-- Blocking on purpose: the database is being rewritten and quitting midway
	     is the one thing that can hurt. No close button, no dismiss-on-click. -->
	<div class="modal-backdrop compact-backdrop" role="presentation" use:portal>
		<div
			class="modal-panel glass-panel compact-panel"
			role="alertdialog"
			aria-modal="true"
			aria-live="assertive"
			aria-label="Compacting database"
		>
			<div class="compact-spinner" aria-hidden="true"></div>
			<h2 class="compact-title">Compacting database</h2>
			<p class="compact-copy">
				Rewriting the database file. This can take several minutes on a large library.
			</p>
			<p class="compact-copy compact-warn">
				Please leave NOORwave open until it finishes. The app will not respond while this runs -
				that is expected, not a crash.
			</p>
		</div>
	</div>
{/if}

<style>

	.database-size-headline {
		display: flex;
		flex-wrap: wrap;
		align-items: baseline;
		gap: 0.5rem;
		font-size: var(--font-size-lg);
	}

	.database-size-arrow {
		opacity: 0.5;
	}

	.database-size-note {
		font-size: var(--font-size-sm);
		opacity: 0.7;
	}

	.compact-backdrop {
		display: grid;
		place-items: center;
	}

	.compact-panel {
		max-width: 26rem;
		padding: 2rem;
		text-align: center;
	}

	.compact-title {
		margin: 0.75rem 0 0.5rem;
		font-size: var(--font-size-lg);
		line-height: var(--line-height-tight);
	}

	.compact-copy {
		margin: 0 0 0.5rem;
		opacity: 0.8;
		font-size: var(--font-size-sm);
		line-height: var(--line-height-normal);
	}

	.compact-warn {
		opacity: 1;
		font-weight: var(--font-weight-semibold);
	}

	.compact-spinner {
		width: 2rem;
		height: 2rem;
		margin: 0 auto;
		border-radius: 50%;
		border: 2px solid color-mix(in srgb, currentColor 25%, transparent);
		border-top-color: currentColor;
		animation: compact-spin 0.9s linear infinite;
	}

	@keyframes compact-spin {
		to {
			transform: rotate(360deg);
		}
	}

	@media (prefers-reduced-motion: reduce) {
		.compact-spinner {
			animation-duration: 3s;
		}
	}

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

	.download-settings {
		display: grid;
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

	.audio-advanced summary::-webkit-details-marker {
		display: none;
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
		padding: var(--space-3) 0;
		border-top: 1px solid var(--border-subtle);
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

	.engine-copy p {
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

	/* Discovery intensity selector + safety preview */
	.intensity-block {
		display: grid;
		gap: 14px;
		padding: var(--space-3) 0;
		border-top: 1px solid var(--border-subtle);
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
		transition: background var(--motion-fast), border-color var(--motion-fast);
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
		transition: width var(--motion-slow);
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

	:global(.setting-flash) {
		animation: settingFlash 1.6s ease; /* motion-ok: a deliberate search-hit highlight */
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

	/* Sources and Account split their cards across two even columns (main + side).
	   Each whole card lives in one column, so they fill the width without the
	   multicol split that tore tall cards (like the integrations panel) in half. */

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
		transition: transform var(--motion-fast);
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
		transition: border-color var(--motion-fast), background var(--motion-fast), box-shadow var(--motion-fast);
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

	/* Counts are rows of their group, one per line (no side-by-side tiles). */
	.inner-metrics {
		grid-template-columns: minmax(0, 1fr);
		gap: 0;
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
		transition: width var(--motion-base);
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
		transition: transform var(--motion-fast);
		margin-right: 4px;
	}

	.discovery-guide[open] > summary::before {
		transform: rotate(90deg);
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
		display: grid;
		gap: var(--space-2);
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

	@media (max-width: 640px) {

		.inner-metrics {
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
	.reclean-summary {
		margin: 8px 0 0;
		font-size: var(--font-size-sm);
		color: rgba(255, 255, 255, 0.6);
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
