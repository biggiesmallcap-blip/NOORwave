<script lang="ts">
	import '../app.css';
	import { onDestroy, onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto, onNavigate } from '$app/navigation';
	import { markNavigated } from '$lib/navigation/back';
	import { connectWebSocket, wsConnected } from '$lib/api/ws';
	import { loadDownloadSettings, refreshDownloadStatus } from '$lib/stores/downloads';
	import DownloadProgressPill from '$lib/components/DownloadProgressPill.svelte';
	import {
		currentTrack,
		currentQueueItemId,
		currentStreamDisplay,
		playbackRuntimeInfo,
		isPlaying,
		position,
		buffered,
		volume,
		automixEnabled,
		automixDiscoverNew,
		shuffleMode,
		repeatMode,
		playbackQueue,
		playerReady,
		playerError,
		refreshPlaybackState,
		playTrackNow,
		playQueueItemNow,
		togglePlayback,
		playPreviousTrack,
		playNextTrack,
		setPlayerVolume,
		setPlayerPosition,
		cyclePlayerShuffleMode,
		cyclePlayerRepeatMode,
		togglePlayerAutomix,
		setPlayerDiscoverNew,
		moveQueueTrackNext,
		removeTrackFromQueue,
		toggleTrackFavorite,
		toggleMute,
		moveQueueItem,
		reorderDropIndex,
		clearQueue as clearQueueAction,
		restoreQueueItems,
		saveQueueAsPlaylist,
		playTidalTrackNow
	} from '$lib/stores/player';
	import { get } from 'svelte/store';
	import type { QueueItem, TidalPlayable, Track } from '$lib/api/client';
	import { showToast } from '$lib/stores/toast';
	import { queueAnnouncement } from '$lib/stores/queue_announcer';
	import { pendingUndo, consumeUndo } from '$lib/stores/queue_undo';
	import { formatTrackDuration, getQualityClass } from '$lib/utils/format';
	import {
		tidalArtworkFallbackSizes,
		upscaleTidalArtwork,
		type TidalArtworkSize,
	} from '$lib/utils/artwork';
	import { api, getStoredToken, setStoredToken, clearStoredToken } from '$lib/api/client';
	import ContextMenu from '$lib/components/ContextMenu.svelte';
	import Toast from '$lib/components/Toast.svelte';
	import CommandPalette from '$lib/components/CommandPalette.svelte';
	import ShortcutHelp from '$lib/components/ShortcutHelp.svelte';
	import PlayerBar from '$lib/shell/PlayerBar.svelte';
	import SidebarNav from '$lib/shell/SidebarNav.svelte';
	import QuietMode from '$lib/components/QuietMode.svelte';
	import { openQuietMode } from '$lib/stores/quiet_mode';
	import { commandPaletteOpen } from '$lib/stores/command_palette';
	import { exclusiveStatus } from '$lib/stores/exclusive_status';
	import { contextMenu, openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import {
		currentQueueAnchorItem,
		currentQueueAnchorPosition,
		isQueueItemActive,
	} from '$lib/player/queue_active';
	import {
		SILENT_SOURCE_LABELS,
		formatQueueSource,
		queueSourceSlug,
	} from '$lib/player/queue_source';
	import { formatPlayerStreamDetail, formatResolutionShort } from '$lib/player/stream_display';
	import { queueItemToTidalPlayable, trackToTidalPlayable } from '$lib/utils/track';
	import ShaderWallpaper from '$lib/components/wallpaper/ShaderWallpaper.svelte';
	import { wallpaperById } from '$lib/components/wallpaper/shaders';
	import { wallpaper, wallpaperFps, wallpaperQuality } from '$lib/stores/wallpaper';
	import { palette } from '$lib/stores/palette';
	import { uiZoom, zoomIn, zoomOut, resetZoom, nudgeZoom, applyZoom } from '$lib/stores/uiZoom';
	import { isTauri } from '$lib/util/external';
	import { tidalAuthFlow, tidalStatus, loadTidalStatus } from '$lib/stores/tidal';
	import {
		TIDAL_PKCE_RELOGIN_DISMISSED_KEY,
		shouldShowLegacyReloginNotice,
	} from '$lib/tidal/login';
	import { scheduleStartupPrewarm } from '$lib/cache/prewarm';
	import { dataCache } from '$lib/cache/query';
	import {
		clearLocalOnboardingComplete,
		hasLocalOnboardingComplete,
		markLocalOnboardingComplete
	} from '$lib/onboarding/status';
	import { paletteById, rgbaCss } from '$lib/components/wallpaper/palettes';
	import {
		MOBILE_MORE_ROUTES,
		MOBILE_TAB_ROUTES,
	} from '$lib/routes/navigation';
	import {
		requestVideoClear,
		requestVideoAutoplayToggle,
		requestVideoJump,
		videoSession,
		videoSessionUpcoming,
	} from '$lib/stores/video_session';
	import VideoDock from '$lib/components/video/VideoDock.svelte';

	let { children } = $props();

	let isOnboardingRouteEarly = $derived(page.url.pathname.startsWith('/onboarding'));
	// During onboarding, force the standing-wave wallpaper so the brand identity
	// is consistent for every user (and so the wallpaper element persists across
	// the navigation to home — same DOM node, no shader remount, no flash).
	let activeWallpaper = $derived(
		isOnboardingRouteEarly ? wallpaperById('standing-wave') : wallpaperById($wallpaper),
	);

	// ─── Auth gate ───────────────────────────────────────────────
	let authReady = $state(false);
	let onboardingChecked = $state(false);
	let isOnboardingRoute = $derived(page.url.pathname.startsWith('/onboarding'));
	let isRemoteRoute = $derived(page.url.pathname.startsWith('/remote'));
	let showConnect = $state(false);
	let connectTokenInput = $state('');
	let connectError = $state('');
	let connectBusy = $state(false);
	let pinInputEl = $state<HTMLInputElement | null>(null);
	let pkceReloginDismissedThisSession = $state(false);
	let pkceReloginDismissedForever = $state(false);
	let cancelStartupPrewarm: (() => void) | null = null;

	function onboardingScope(): string | null {
		return getStoredToken();
	}

	function setSessionToken(token: string): void {
		if (getStoredToken() !== token) dataCache.clear();
		setStoredToken(token);
	}

	function clearSessionToken(): void {
		clearStoredToken();
		dataCache.clear();
	}
	// Remove this migration notice after 2026-05-25. Keep PKCE auth and encrypted token migration.
	let showPkceReloginNotice = $derived(
		authReady &&
			onboardingChecked &&
			!isOnboardingRoute &&
			shouldShowLegacyReloginNotice(
				{ connected: $tidalStatus === 'connected', auth_flow: $tidalAuthFlow },
				{
					dismissedForever: pkceReloginDismissedForever,
					dismissedThisSession: pkceReloginDismissedThisSession,
				}
			)
	);

	function handlePinInput(event: Event) {
		const el = event.target as HTMLInputElement;
		const digits = el.value.replace(/\D/g, '').slice(0, 6);
		connectTokenInput = digits;
		el.value = digits;
		connectError = '';
		if (digits.length === 6) void submitConnect();
	}

	function focusPin() {
		pinInputEl?.focus();
	}

	async function submitConnect() {
		connectError = '';
		const t = connectTokenInput.trim();
		if (!t) { connectError = 'Enter your 6-digit PIN.'; return; }
		connectBusy = true;
		try {
			setSessionToken(t);
			const ok = await api.ping();
			if (!ok) { clearSessionToken(); connectError = 'Could not reach the server. Check the URL / network.'; return; }
			const resp = await fetch(`${(await import('$lib/api/client')).getApiBase()}/api/status`, {
				headers: { authorization: `Bearer ${t}` }
			});
			if (resp.status === 401) {
				clearSessionToken();
				connectError = 'PIN rejected — double-check the 6 digits.';
				connectTokenInput = '';
				setTimeout(focusPin, 0);
				return;
			}
			showConnect = false;
			onConnected();
		} catch {
			clearSessionToken();
			connectError = 'Connection failed. Is the server running?';
		} finally {
			connectBusy = false;
		}
	}

	let isScrubbing = $state(false);
	let scrubPosition = $state(0);
	let theme = $state<'dark' | 'light'>('dark');
	let displayVolume = $state(Math.round($volume * 100));

	// Auto-dismiss the error toast after 6s. Cancel on next change so a new
	// error doesn't inherit the previous timer.
	let _errorDismissTimer: ReturnType<typeof setTimeout> | null = null;
	$effect(() => {
		const err = $playerError;
		if (_errorDismissTimer) {
			clearTimeout(_errorDismissTimer);
			_errorDismissTimer = null;
		}
		if (err) {
			_errorDismissTimer = setTimeout(() => playerError.set(null), 6000);
		}
	});
	let nowPlayingOpen = $state(false);
	let moreOpen = $state(false);
	let shortcutHelpOpen = $state(false);

	async function setupDesktopUpdateToasts(unlisteners: Array<() => void>) {
		if (!isTauri()) return;
		try {
			const { listen } = await import('@tauri-apps/api/event');
			const unlistenAvailable = await listen<string>('update-available', (event) => {
				showToast(
					`NOORwave v${event.payload} is available. Use the tray or Settings > About to install.`,
					'success',
					8000
				);
			});
			unlisteners.push(unlistenAvailable);

			const unlistenError = await listen<string>('update-error', (event) => {
				showToast(`Update check failed: ${event.payload}`, 'error', 7000);
			});
			unlisteners.push(unlistenError);
		} catch (err) {
			console.warn('update notification listener setup failed', err);
		}
	}

	let mobileFavoritePending = $state(false);
	let desktopFavoritePending = $state(false);

	const shuffleLabels: Record<string, string> = {
		off: 'Shuffle off',
		genre: 'Genre mix',
		weighted: 'Smart shuffle',
		true: 'True random'
	};

	const shuffleStatusLabels: Record<string, string> = {
		genre: 'Genre mix',
		weighted: 'Smart shuffle',
		true: 'True random'
	};

	// Ambient session state for the sidebar pill: the modes that are actually
	// on, in one line, instead of a paragraph each.
	let serverVersion = $state('');
	let sessionModeLine = $derived(
		[
			// Keep the mode named, not just its value: a bare "Genre mix" in the
			// sidebar reads as a notification rather than current state.
			$shuffleMode !== 'off' ? `Shuffle: ${shuffleStatusLabels[$shuffleMode]}` : null,
			$automixEnabled ? 'Automix on' : null,
		]
			.filter(Boolean)
			.join(' - '),
	);

	const shuffleIcons: Record<string, string> = {
		off: '⇄',
		genre: '◈',
		weighted: '◉',
		true: '⤮'
	};

	const repeatLabels: Record<string, string> = {
		off: 'Repeat off',
		all: 'Repeat all',
		one: 'Repeat one'
	};

	const repeatIcons: Record<string, string> = {
		off: '↻',
		all: '↺',
		one: '⊙'
	};

	const shuffleModeNames: Record<string, string> = {
		off: 'off',
		genre: 'genre',
		weighted: 'smart',
		true: 'true'
	};

	const repeatModeNames: Record<string, string> = {
		off: 'off',
		all: 'all',
		one: 'one'
	};

	function handleUnauthorized() {
		clearSessionToken();
		authReady = false;
		onboardingChecked = false;
		cancelStartupPrewarm?.();
		cancelStartupPrewarm = null;
		void tryAutoSetup();
	}

	// Liquid-glass crossfade — scoped to the onboarding → home handoff only.
	// Every other navigation uses the default (instant) transition. Falls through
	// silently on browsers without the View Transitions API (pre-Chromium 111).
	onNavigate((nav) => {
		// Record that we've moved within the app, so detail-page back buttons
		// can safely pop history instead of jumping to a fixed page.
		markNavigated(Boolean(nav.from));
		if (!nav.from?.url.pathname.startsWith('/onboarding')) return;
		if (typeof document === 'undefined' || !('startViewTransition' in document)) return;
		return new Promise((resolve) => {
			(document as any).startViewTransition(async () => {
				resolve();
				await nav.complete;
			});
		});
	});

	onMount(() => {
		const tauriUpdateUnlisteners: Array<() => void> = [];
		// Show connect screen if no token is stored
		if (!getStoredToken()) {
			void tryAutoSetup();
		} else {
			authReady = true;
			if (hasLocalOnboardingComplete(onboardingScope())) onboardingChecked = true;
			connectWebSocket();
			void refreshPlaybackState();
			void checkOnboarding();
			void loadDownloadSettings();
			void refreshDownloadStatus();
			startStartupPrewarm();
		}

		// Listen for 401 responses from any API call. On loopback the backend
		// will hand us a fresh token via /api/setup/token, so retry auto-setup
		// before falling back to the PIN modal — keeps local launches silent
		// even if the stored token is stale (e.g. server regenerated).
		window.addEventListener('noor:unauthorized', handleUnauthorized);

		// Build string for the sidebar pill. Cosmetic, so a failure is silent.
		void api
			.getStatus()
			.then((status) => {
				serverVersion = status.version ?? '';
			})
			.catch(() => {});

		const storedTheme = localStorage.getItem('noor-theme');
		if (storedTheme === 'light' || storedTheme === 'dark') {
			theme = storedTheme;
		}
		pkceReloginDismissedForever =
			localStorage.getItem(TIDAL_PKCE_RELOGIN_DISMISSED_KEY) === '1';

		applyTheme(theme);

		const unsubPalette = palette.subscribe((id) => applyPalette(id));

		// Re-apply persisted UI zoom now that the Tauri webview is ready.
		// In a regular browser this no-ops (and the OS Ctrl+/Ctrl- handles zoom natively).
		void applyZoom(get(uiZoom));
		void setupDesktopUpdateToasts(tauriUpdateUnlisteners);

		window.addEventListener('keydown', handleGlobalKeydown);
		window.addEventListener('wheel', handleGlobalWheel, { passive: false });
		return () => {
			window.removeEventListener('keydown', handleGlobalKeydown);
			window.removeEventListener('wheel', handleGlobalWheel);
			cancelStartupPrewarm?.();
			for (const unlisten of tauriUpdateUnlisteners) unlisten();
			unsubPalette();
		};
	});

	onDestroy(() => {
		window.removeEventListener('noor:unauthorized', handleUnauthorized);
	});

	function isTypingTarget(target: EventTarget | null): boolean {
		if (!(target instanceof HTMLElement)) return false;
		const tag = target.tagName;
		if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
		return target.isContentEditable;
	}

	function shortcutsBlocked(): boolean {
		return showConnect || nowPlayingOpen || moreOpen || get(commandPaletteOpen) || get(contextMenu).open;
	}

	function openShortcutHelp() {
		shortcutHelpOpen = true;
	}

	function closeShortcutHelp() {
		shortcutHelpOpen = false;
	}

	function handleGlobalWheel(event: WheelEvent) {
		if (!(event.ctrlKey || event.metaKey)) return;
		// In a regular browser, let native Ctrl+wheel zoom through — preventing it
		// would suppress browser zoom while our store no-ops outside Tauri.
		if (!isTauri()) return;
		event.preventDefault();
		nudgeZoom(event.deltaY > 0 ? -1 : 1);
	}

	function handleGlobalKeydown(event: KeyboardEvent) {
		if (shortcutHelpOpen) {
			if (event.key === 'Escape') {
				event.preventDefault();
				closeShortcutHelp();
			}
			return;
		}
		// Cmd/Ctrl+K → open command palette
		if ((event.ctrlKey || event.metaKey) && event.key === 'k') {
			event.preventDefault();
			commandPaletteOpen.update(v => !v);
			return;
		}
		// Browser-style UI zoom: Ctrl/Cmd + (+ | - | 0). Tauri-only — in a regular
		// browser, let the native zoom shortcuts through (otherwise we'd block
		// them and replace them with a no-op).
		if (isTauri() && (event.ctrlKey || event.metaKey) && !event.altKey) {
			if (event.key === '+' || event.key === '=') {
				event.preventDefault();
				zoomIn();
				return;
			}
			if (event.key === '-' || event.key === '_') {
				event.preventDefault();
				zoomOut();
				return;
			}
			if (event.key === '0') {
				event.preventDefault();
				resetZoom();
				return;
			}
		}
		if (event.ctrlKey || event.metaKey || event.altKey) return;
		if (isTypingTarget(event.target)) return;

		if (event.key === '?' || (event.key === '/' && event.shiftKey)) {
			if (shortcutsBlocked()) return;
			event.preventDefault();
			openShortcutHelp();
			return;
		}

		if (shortcutsBlocked()) return;

		if (event.key === 'q' || event.key === 'Q') {
			event.preventDefault();
			toggleQueueExpanded();
			return;
		}

		const target = event.target as HTMLElement | null;
		const inQueueSection = target?.closest?.('.queue-section') != null;
		if (inQueueSection && event.key === 'ArrowUp' && !queueExpanded) {
			event.preventDefault();
			toggleQueueExpanded();
			return;
		}
		if (inQueueSection && event.key === 'ArrowDown' && queueExpanded) {
			event.preventDefault();
			toggleQueueExpanded();
			return;
		}

		switch (event.key) {
			case ' ':
			case 'Spacebar':
				event.preventDefault();
				void togglePlayback();
				break;
			case 'ArrowRight':
				event.preventDefault();
				if (event.shiftKey) {
					void playNextTrack();
				} else {
					void setPlayerPosition(get(position) + 5000);
				}
				break;
			case 'ArrowLeft':
				event.preventDefault();
				if (event.shiftKey) {
					void playPreviousTrack();
				} else {
					void setPlayerPosition(Math.max(0, get(position) - 5000));
				}
				break;
			case 'ArrowUp':
				event.preventDefault();
				void setPlayerVolume(get(volume) + 0.05);
				break;
			case 'ArrowDown':
				event.preventDefault();
				void setPlayerVolume(get(volume) - 0.05);
				break;
			case 'm':
			case 'M':
				event.preventDefault();
				void toggleMute();
				break;
			case 'l':
			case 'L': {
				const track = $currentTrack;
				if (!track) return;
				event.preventDefault();
				void toggleTrackFavorite(track.id);
				break;
			}
			case 's':
			case 'S':
				event.preventDefault();
				void cyclePlayerShuffleMode();
				break;
			case 'r':
			case 'R':
				event.preventDefault();
				void cyclePlayerRepeatMode();
				break;
			case 'z':
			case 'Z':
				if ($pendingUndo) {
					event.preventDefault();
					void handleUndoClear();
				}
				break;
		}
	}

	// On the local machine, the server exposes the token without auth (loopback-only).
	async function tryAutoSetup() {
		try {
			const { getApiBase } = await import('$lib/api/client');
			const resp = await fetch(`${getApiBase()}/api/setup/token`);
			if (resp.ok) {
				const data = await resp.json();
				if (data?.token) {
					setSessionToken(data.token);
					onConnected();
					return;
				}
			}
		} catch {}
		showConnect = true;
		setTimeout(focusPin, 50);
	}

	// After a successful connect, boot the WS + playback state
	function onConnected() {
		authReady = true;
		if (hasLocalOnboardingComplete(onboardingScope())) onboardingChecked = true;
		connectWebSocket();
		void refreshPlaybackState();
		void loadTidalStatus();
		void checkOnboarding();
		startStartupPrewarm();
	}

	function startStartupPrewarm() {
		cancelStartupPrewarm?.();
		cancelStartupPrewarm = scheduleStartupPrewarm();
	}

	function dismissPkceReloginForSession() {
		pkceReloginDismissedThisSession = true;
	}

	function dismissPkceReloginForever() {
		pkceReloginDismissedForever = true;
		pkceReloginDismissedThisSession = true;
		localStorage.setItem(TIDAL_PKCE_RELOGIN_DISMISSED_KEY, '1');
	}

	async function reconnectTidalWithPkce() {
		pkceReloginDismissedThisSession = true;
		await goto('/settings?tidalLogin=1');
	}

	// Redirects to /onboarding when the server reports first-run state. Fails
	// open: a transient error must not trap the user on a "Checking setup…"
	// screen — they land on home and the next session re-checks.
	async function checkOnboarding() {
		try {
			const { getApiBase } = await import('$lib/api/client');
			const resp = await fetch(`${getApiBase()}/api/setup/onboarding`);
			if (!resp.ok) throw new Error(`onboarding check failed: ${resp.status}`);
			const { complete } = await resp.json();
			onboardingChecked = true;
			if (complete) {
				markLocalOnboardingComplete(onboardingScope());
			} else {
				clearLocalOnboardingComplete(onboardingScope());
			}
			if (!complete && !page.url.pathname.startsWith('/onboarding')) {
				await goto('/onboarding', { replaceState: true });
			}
		} catch (err) {
			console.warn('onboarding check failed', err);
			onboardingChecked = true;
		}
	}

	function applyTheme(t: 'dark' | 'light') {
		theme = t;
		document.documentElement.setAttribute('data-theme', t);
		localStorage.setItem('noor-theme', t);
	}

	function applyPalette(id: import('$lib/components/wallpaper/palettes').PaletteId) {
		const p = paletteById(id);
		const root = document.documentElement.style;
		root.setProperty('--accent', p.ui.accent);
		root.setProperty('--accent-strong', p.ui.accentStrong);
		root.setProperty('--accent-soft', p.ui.accentSoft);
		root.setProperty('--accent-line', p.ui.accentLine);
		root.setProperty('--accent-glow', p.ui.accentGlow);
		root.setProperty('--atlas-haze-a', rgbaCss(p.shader.c2, 0.18));
		root.setProperty('--atlas-haze-b', rgbaCss(p.shader.c3, 0.13));
		root.setProperty('--atlas-haze-c', rgbaCss(p.shader.c4, 0.10));
	}

	function toggleTheme() {
		applyTheme(theme === 'dark' ? 'light' : 'dark');
	}

	function isNavItemActive(path: string) {
		if (path === '/') return page.url.pathname === '/';
		return page.url.pathname === path || page.url.pathname.startsWith(`${path}/`);
	}

	function runOnActivation(event: KeyboardEvent, action: () => void) {
		if (event.key !== 'Enter' && event.key !== ' ') return;
		event.preventDefault();
		action();
	}

	function beginScrub() {
		isScrubbing = true;
	}

	async function commitScrub() {
		isScrubbing = false;
		await setPlayerPosition(scrubPosition);
	}

	async function handleQueueTrackPlay(item: QueueItemType) {
		await playQueueItemNow(item.id);
		nowPlayingOpen = false;
	}

	function handleQueueTrackKeydown(item: QueueItemType, event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			// Pending rows are playable too: play-item resolves (imports) the row
			// on the way in before starting it.
			event.preventDefault();
			void handleQueueTrackPlay(item);
			return;
		}
		if (event.key === 'Delete' || event.key === 'Backspace') {
			event.preventDefault();
			void removeTrackFromQueue(item.id);
			return;
		}
		if (event.altKey && event.key === 'ArrowUp') {
			event.preventDefault();
			if (event.shiftKey) {
				void moveQueueTrackNext(item.id);
			} else {
				void reorderQueueRow(item, -1);
			}
			return;
		}
		if (event.altKey && event.key === 'ArrowDown') {
			event.preventDefault();
			void reorderQueueRow(item, 1);
		}
	}

	async function reorderQueueRow(item: QueueItemType, delta: -1 | 1) {
		const fullQueue = get(playbackQueue);
		const currentIdx = fullQueue.findIndex((q) => q.id === item.id);
		if (currentIdx === -1) return;
		const newIdx = currentIdx + delta;
		if (newIdx < 0 || newIdx >= fullQueue.length) return;
		// Refuse to move a row above the currently-playing item. The play head
		// is rendered as the "current" row and is not user-reorderable.
		const playingId = $currentTrack?.id ?? null;
		if (playingId != null) {
			const playingIdx = fullQueue.findIndex((q) => q.track.id === playingId);
			if (playingIdx >= 0 && newIdx <= playingIdx) return;
		}
		await moveQueueItem(item.id, newIdx);
	}

	function formatQuality(q: string | null) {
		if (!q) return '';
		if (q === 'HI_RES_LOSSLESS') return 'HiRes Lossless';
		if (q === 'LOSSLESS') return 'Lossless';
		if (q === 'HIGH') return 'High';
		if (q === 'LOW') return 'Low';
		return q.replaceAll('_', ' ');
	}

	type QueueItemType = (typeof $playbackQueue)[number];

	/**
	 * Pick the shared context-menu builder. Pending and transient TIDAL rows
	 * retain their provider id, so provider-specific actions stay available.
	 */
	function pickMenuBuilder(track: Track, options?: { queueItemId?: number; isPending?: boolean }) {
		const tidal = trackToTidalPlayable(track);
		if (tidal) {
			return buildTidalTrackMenu(tidal, options?.queueItemId === undefined ? undefined : { inQueue: true, queueItemId: options.queueItemId });
		}
		return buildTrackMenu(track, options);
	}

	function queueItemTidalPlayable(item: QueueItemType): TidalPlayable | null {
		return queueItemToTidalPlayable(item);
	}

	// TIDAL-backed queue rows use their stable queue item id for mutations.
	function queueRowMenuItems(item: QueueItemType) {
		const tidal = queueItemTidalPlayable(item);
		return tidal
			? buildTidalTrackMenu(tidal, { inQueue: true, queueItemId: item.id })
			: pickMenuBuilder(item.track, { queueItemId: item.id, isPending: item.is_pending });
	}

	function openQueueRowMenu(item: QueueItemType, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(event, queueRowMenuItems(item), item.track.title);
	}

	function openQueueRowMenuFromButton(item: QueueItemType, event: MouseEvent) {
		event.stopPropagation();
		openMenuAtElement(event.currentTarget as HTMLElement, queueRowMenuItems(item), item.track.title);
	}

	function openQueueArtistContextMenu(item: QueueItemType, event: MouseEvent) {
		const artistId = item.track.artist_id;
		if (artistId == null || artistId <= 0) return;
		event.preventDefault();
		event.stopPropagation();
		openContextMenu(
			event,
			buildArtistMenu({
				id: artistId,
				name: item.track.artist_name ?? 'Unknown artist',
				in_library: true
			}),
			item.track.artist_name ?? 'Unknown artist'
		);
	}

	// ─── Queue drag-to-reorder ────────────────────────────────────────────────
	let dragItemId = $state<number | null>(null);
	let dragOverItemId = $state<number | null>(null);

	function handleQueueDragStart(event: DragEvent, item: QueueItemType) {
		if (item.is_pending === true) return;
		dragItemId = item.id;
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
			// Required for Firefox to actually start a drag.
			event.dataTransfer.setData('text/plain', String(item.id));
			// Drag the whole row as the ghost (not the bare grip glyph) so the
			// preview reads as "moving this track", aligned under the cursor.
			const row = event.currentTarget;
			if (row instanceof HTMLElement && typeof event.dataTransfer.setDragImage === 'function') {
				const rect = row.getBoundingClientRect();
				event.dataTransfer.setDragImage(row, event.clientX - rect.left, event.clientY - rect.top);
			}
		}
	}

	function handleQueueDragOver(event: DragEvent, item: QueueItemType) {
		if (dragItemId === null) return;
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = 'move';
		dragOverItemId = item.id;
	}

	function handleQueueDragLeave(item: QueueItemType) {
		if (dragOverItemId === item.id) dragOverItemId = null;
	}

	async function handleQueueDrop(event: DragEvent, target: QueueItemType) {
		event.preventDefault();
		const sourceId = dragItemId;
		dragItemId = null;
		dragOverItemId = null;
		if (sourceId === null || sourceId === target.id) return;
		const fullQueue = $playbackQueue;
		const targetIndex = fullQueue.findIndex((q) => q.id === target.id);
		if (targetIndex === -1) return;
		// moveQueueItem (and the server) interpret the index AFTER the dragged row
		// is spliced out. For a downward drag (source sits above the target),
		// removing the source shifts the target up one slot, so reorderDropIndex
		// subtracts one to land ON the target's top edge (the "drops here"
		// indicator) instead of one row below it. Upward drags keep the index.
		const sourceIndex = fullQueue.findIndex((q) => q.id === sourceId);
		await moveQueueItem(sourceId, reorderDropIndex(sourceIndex, targetIndex));
	}

	function handleQueueDragEnd() {
		dragItemId = null;
		dragOverItemId = null;
	}

	// ─── Scroll active queue row into view ───────────────────────────────────
	let lastUserScrollAt = $state(0);
	let queueListEl: HTMLElement | null = $state(null);
	let currentRowVisible = $state(true);

	function refreshCurrentRowVisibility() {
		const id = $currentTrack?.id;
		if (!id || !queueListEl) {
			currentRowVisible = true;
			return;
		}
		const row = queueListEl.querySelector(`[data-track-id="${id}"]`);
		if (!row) {
			currentRowVisible = true;
			return;
		}
		const rect = (row as HTMLElement).getBoundingClientRect();
		const containerRect = queueListEl.getBoundingClientRect();
		currentRowVisible = !(rect.bottom < containerRect.top || rect.top > containerRect.bottom);
	}

	function handleQueueScroll() {
		lastUserScrollAt = Date.now();
		refreshCurrentRowVisibility();
	}

	function jumpToCurrentRow() {
		const id = $currentTrack?.id;
		if (!id || !queueListEl) return;
		const row = queueListEl.querySelector(`[data-track-id="${id}"]`);
		if (!row) return;
		(row as HTMLElement).scrollIntoView({
			block: 'center',
			behavior: prefersReducedMotion() ? 'auto' : 'smooth',
		});
		// Re-enable the auto-jump effect so subsequent track changes follow.
		lastUserScrollAt = 0;
		refreshCurrentRowVisibility();
	}

	function prefersReducedMotion(): boolean {
		if (typeof window === 'undefined' || !window.matchMedia) return false;
		return window.matchMedia('(prefers-reduced-motion: reduce)').matches;
	}

	$effect(() => {
		const id = $currentTrack?.id;
		if (!id || !queueListEl) {
			currentRowVisible = true;
			return;
		}
		const row = queueListEl.querySelector(`[data-track-id="${id}"]`);
		if (!row) {
			currentRowVisible = true;
			return;
		}
		const rect = row.getBoundingClientRect();
		const containerRect = queueListEl.getBoundingClientRect();
		const offscreen = rect.bottom < containerRect.top || rect.top > containerRect.bottom;
		// Bail on the auto-scroll if the user scrolled recently - don't yank
		// focus from their browse. The jump-to-current chip still shows.
		if (offscreen && Date.now() - lastUserScrollAt >= 5000) {
			row.scrollIntoView({
				block: 'nearest',
				behavior: prefersReducedMotion() ? 'auto' : 'smooth',
			});
			currentRowVisible = true;
		} else {
			currentRowVisible = !offscreen;
		}
	});

	// ─── Source attribution for now-playing card ─────────────────────────────
	function attributionFor(track: { id: number } | null): string | null {
		if (!track) return null;
		const item = currentQueueAnchorItem($playbackQueue, $currentTrack, $currentQueueItemId);
		if (!item) return null;
		const friendly = formatQueueSource(item.source);
		// Hand-queued / generic rows aren't worth surfacing.
		if (SILENT_SOURCE_LABELS.has(friendly)) return null;
		return friendly;
	}
	let nowPlayingAttribution = $derived(attributionFor($currentTrack));

	// ─── Clear / Save queue UI state ─────────────────────────────────────────
	let saveQueueOpen = $state(false);
	let saveQueueName = $state('');
	let savePending = $state(false);

	function focusOnMount(node: HTMLElement) {
		queueMicrotask(() => node.focus());
	}

	function defaultSaveName(): string {
		const attr = nowPlayingAttribution;
		if (attr && $currentTrack) {
			return `${attr} · ${$currentTrack.title}`;
		}
		const now = new Date();
		return `Saved queue · ${now.toLocaleDateString()}`;
	}

	function openSaveQueue() {
		saveQueueName = defaultSaveName();
		saveQueueOpen = true;
	}

	async function commitSaveQueue() {
		const name = saveQueueName.trim();
		if (!name || savePending) return;
		savePending = true;
		try {
			await saveQueueAsPlaylist(name);
			saveQueueOpen = false;
		} finally {
			savePending = false;
		}
	}

	async function handleClearQueue() {
		if (upcomingQueue.length === 0) return;
		await clearQueueAction();
		// The store offers an undo: the queue-section renders an Undo chip
		// bound to `pendingUndo`. Z is the power-user shortcut to fire the
		// same path without reaching for the mouse.
	}

	async function handleUndoClear() {
		const restorable = consumeUndo();
		if (!restorable) return;
		await restoreQueueItems(restorable);
	}

	function stopPropagation(event: Event) {
		event.stopPropagation();
	}

	let failedArtworkUrls = $state<Record<string, boolean>>({});

	function artworkCandidate(
		rawUrl: string | null | undefined,
		size: TidalArtworkSize,
	): string | null {
		if (!rawUrl) return null;
		for (const candidateSize of tidalArtworkFallbackSizes(rawUrl, size)) {
			const candidate = upscaleTidalArtwork(rawUrl, candidateSize);
			if (candidate && !failedArtworkUrls[candidate]) return candidate;
		}
		return null;
	}

	function markArtworkFailed(renderedUrl: string | null | undefined) {
		if (!renderedUrl) return;
		failedArtworkUrls = { ...failedArtworkUrls, [renderedUrl]: true };
	}

	let currentVideoArtwork = $derived(artworkCandidate($videoSession.current?.artwork_url, 320));
	let mobileMiniArtwork = $derived(artworkCandidate($currentTrack?.artwork_url, 320));
	let mobileNowPlayingArtwork = $derived(artworkCandidate($currentTrack?.artwork_url, 640));

	function openNowPlayingMenu(event: MouseEvent) {
		const track = $currentTrack;
		if (!track) return;
		event.stopPropagation();
		const items = pickMenuBuilder(track);
		openMenuAtElement(event.currentTarget as HTMLElement, items, track.title);
	}

	function openNowPlayingMenuAt(anchor: HTMLElement) {
		const track = $currentTrack;
		if (!track) return;
		const items = pickMenuBuilder(track);
		openMenuAtElement(anchor, items, track.title);
	}

	function openNowPlayingContextMenu(event: MouseEvent) {
		const track = $currentTrack;
		if (!track) return;
		const items = pickMenuBuilder(track);
		openContextMenu(event, items, track.title);
	}

	let upcomingQueue = $derived.by(() => {
		const currentPosition = currentQueueAnchorPosition(
			$playbackQueue,
			$currentTrack,
			$currentQueueItemId,
		) ?? -1;
		return $playbackQueue.filter((item) => item.position > currentPosition);
	});

	let queueCountLabel = $derived(
		upcomingQueue.length === 1 ? '1 track queued' : `${upcomingQueue.length} tracks queued`
	);

	let queueTotalMs = $derived(
		upcomingQueue.reduce((sum, item) => sum + (item.track.duration_ms ?? 0), 0)
	);

	function formatQueueTotal(ms: number): string {
		if (ms < 60_000) return '<1m';
		const totalMin = Math.round(ms / 60_000);
		if (totalMin < 60) return `${totalMin}m`;
		const hr = Math.floor(totalMin / 60);
		const min = totalMin % 60;
		return min === 0 ? `${hr}h` : `${hr}h ${min}m`;
	}

	let queueTotalLabel = $derived(formatQueueTotal(queueTotalMs));

	// Default visible-row cap for both desktop and mobile queue lists. The
	// cap exists so a 500-row library/radio session doesn't paint thousands
	// of DOM nodes at boot; users grow it with a "Load more" button so the
	// rest of the queue isn't silently truncated.
	const QUEUE_INITIAL_CAP = 40;
	const QUEUE_LOAD_MORE_STEP = 40;
	let queueVisibleCount = $state(QUEUE_INITIAL_CAP);

	$effect(() => {
		// Reset the cap when the queue gets smaller than what we are currently
		// showing (clear, big remove, snapshot shrink). Without this the
		// "Load more" button would linger at a count larger than the queue.
		if (upcomingQueue.length < queueVisibleCount) {
			queueVisibleCount = Math.max(QUEUE_INITIAL_CAP, Math.min(queueVisibleCount, upcomingQueue.length || QUEUE_INITIAL_CAP));
		}
	});

	function loadMoreQueue() {
		queueVisibleCount = Math.min(upcomingQueue.length, queueVisibleCount + QUEUE_LOAD_MORE_STEP);
	}

	const QUEUE_EXPANDED_KEY = 'noor.queueExpanded';

	function loadQueueExpanded(): boolean {
		if (typeof localStorage === 'undefined') return false;
		return localStorage.getItem(QUEUE_EXPANDED_KEY) === '1';
	}

	let queueExpanded = $state(loadQueueExpanded());

	function toggleQueueExpanded() {
		queueExpanded = !queueExpanded;
		if (typeof localStorage !== 'undefined') {
			localStorage.setItem(QUEUE_EXPANDED_KEY, queueExpanded ? '1' : '0');
		}
	}
	function formatVideoSourceLabel(source: string, label: string | null): string {
		if (source === 'mix') return label ?? 'Video mix';
		if (source === 'search') return label ? `Search: ${label}` : 'Video search';
		if (source === 'direct') return 'Direct video';
		return 'Video session';
	}

	let playerState = $derived(
		$currentTrack ? ($isPlaying ? 'Playing' : 'Paused') : $playerReady ? 'Ready' : 'Connecting'
	);
	let streamDetailLabel = $derived(formatPlayerStreamDetail({
		stream: $currentStreamDisplay,
		runtime: $playbackRuntimeInfo,
		exclusiveEngaged: $exclusiveStatus.engaged,
	}));
	let videoRouteActive = $derived(page.url.pathname.startsWith('/videos'));
	let videoChromeActive = $derived(videoRouteActive && $videoSession.active);
	let mobilePlayerVisible = $derived(Boolean($currentTrack) && !videoChromeActive);
	let progressWidth = $derived(
		$currentTrack?.duration_ms && $currentTrack.duration_ms > 0
			? `${Math.min((scrubPosition / $currentTrack.duration_ms) * 100, 100)}%`
			: '0%'
	);

	$effect(() => {
		if (!isScrubbing) {
			scrubPosition = $position;
		}
	});

	$effect(() => {
		displayVolume = Math.round($volume * 100);
	});

	$effect(() => {
		if (!$currentTrack) {
			nowPlayingOpen = false;
		}
	});

	$effect(() => {
		if (videoChromeActive) {
			nowPlayingOpen = false;
		}
	});

	$effect(() => {
		page.url.pathname;
		moreOpen = false;
		nowPlayingOpen = false;
	});

	async function handleMobileFavoriteToggle() {
		if (!$currentTrack || mobileFavoritePending) return;
		mobileFavoritePending = true;
		try {
			await toggleTrackFavorite($currentTrack.id);
		} finally {
			mobileFavoritePending = false;
		}
	}

	async function handleDesktopFavoriteToggle() {
		if (!$currentTrack || desktopFavoritePending) return;
		desktopFavoritePending = true;
		try {
			await toggleTrackFavorite($currentTrack.id);
		} finally {
			desktopFavoritePending = false;
		}
	}

	$effect(() => {
		if ($currentTrack) {
			desktopFavoritePending = false;
		}
	});
</script>

{#if showConnect}
	<div class="connect-backdrop">
		<div class="connect-panel glass-panel">
			<div class="connect-brand">
				<span class="connect-brand-mark">
					<img src="/noor-icon-transparent.svg" alt="" aria-hidden="true" />
				</span>
				<span class="connect-brand-name">NOOR</span>
			</div>
			<h2 class="connect-title">Enter PIN</h2>
			<p class="connect-copy">
				Check the NOOR terminal or Settings on your main device for the 6-digit access PIN.
			</p>

			<button type="button" class="pin-pad" onclick={focusPin} aria-label="PIN input">
				{#each [0,1,2,3,4,5] as i}
					<span
						class="pin-digit"
						class:filled={i < connectTokenInput.length}
						class:active={i === connectTokenInput.length && !connectBusy}
					>
						{connectTokenInput[i] ?? ''}
					</span>
				{/each}
			</button>

			<input
				bind:this={pinInputEl}
				class="pin-hidden-input"
				inputmode="numeric"
				pattern="[0-9]*"
				maxlength="6"
				autocomplete="one-time-code"
				value={connectTokenInput}
				oninput={handlePinInput}
				onkeydown={(e) => e.key === 'Enter' && connectTokenInput.length === 6 && void submitConnect()}
				disabled={connectBusy}
				aria-label="6-digit PIN"
			/>

			{#if connectError}
				<p class="connect-error">{connectError}</p>
			{/if}
			{#if connectBusy}
				<p class="connect-copy">Connecting…</p>
			{/if}
		</div>
	</div>
{/if}

<!-- Skipped on the phone remote: the fixed shader layer shows through and
     flickers when the mobile address bar collapses past 100svh, and the
     remote shell is opaque anyway. -->
<div class="wallpaper-layer" aria-hidden="true">
	{#if activeWallpaper.shader && !isRemoteRoute}
		<ShaderWallpaper
			shader={activeWallpaper.shader}
			interactive={false}
			maxDpr={$wallpaperQuality === 'high' ? 2 : 1}
			targetFps={$wallpaperFps}
			reactGain={activeWallpaper.reactGain ?? 1}
		/>
	{/if}
</div>

<ContextMenu />
<Toast />
<DownloadProgressPill />
<CommandPalette />
<QuietMode />
<ShortcutHelp open={shortcutHelpOpen} onClose={closeShortcutHelp} />

{#if showPkceReloginNotice}
	<div class="pkce-relogin-backdrop" role="presentation">
		<div class="pkce-relogin-modal glass-panel" role="dialog" aria-modal="true" aria-labelledby="pkce-relogin-title">
			<h2 id="pkce-relogin-title">TIDAL login changed</h2>
			<p>
				NOORwave now uses TIDAL's PKCE sign-in for Lossless and Hi-Res playback. Sign in again to keep full-quality streaming.
			</p>
			<div class="pkce-relogin-actions">
				<button class="btn btn-primary" onclick={() => void reconnectTidalWithPkce()}>Reconnect TIDAL</button>
				<button class="btn btn-glass" onclick={dismissPkceReloginForSession}>Not now</button>
				<button class="btn btn-ghost" onclick={dismissPkceReloginForever}>Don't show again</button>
			</div>
		</div>
	</div>
{/if}

{#if isOnboardingRoute}
	{@render children()}
{:else if !authReady}
	<div class="onboarding-check">
		<img class="check-mark" src="/noor-icon-transparent.svg" alt="" aria-hidden="true" />
		<p>Checking setup…</p>
	</div>
{:else if !onboardingChecked}
	<div class="onboarding-check">
		<img class="check-mark" src="/noor-icon-transparent.svg" alt="" aria-hidden="true" />
		<p>Checking setup…</p>
	</div>
{:else if isRemoteRoute}
	<div class="remote-shell">
		{@render children()}
	</div>
{:else}
<div class="app-shell" class:mobile-player-active={mobilePlayerVisible} class:has-wallpaper={$wallpaper !== 'none'}>
	<header class="mobile-top-bar">
		<a href="/" class="mobile-brand" aria-label="NOOR home">
			<span class="mobile-brand-mark">
				<img src="/noor-icon-transparent.svg" alt="" aria-hidden="true" />
			</span>
			<span class="mobile-brand-name">NOOR</span>
		</a>
		<button class="mobile-theme-btn btn btn-glass" onclick={toggleTheme}>
			{theme === 'dark' ? '☀' : '◑'}
		</button>
	</header>

	<aside class="sidebar">
		<a href="/" class="brand" aria-label="NOORwave home">
			<img class="brand-splash brand-splash-on-dark" src="/noor-logo-centered-transparent.svg" alt="NOORwave" />
			<img class="brand-splash brand-splash-on-light" src="/noor-logo-centered-transparent-dark.svg" alt="NOORwave" />
		</a>

		<SidebarNav pathname={page.url.pathname} />

		<div class="sidebar-footer">
			<!-- One pill for the session's ambient state: connection, build, the
			     modes that are on, and the always-on toggles that used to crowd
			     the queue header on the far side of the screen. -->
			<div class="live-status">
				<div class="live-status-head">
					<span class:offline={!$wsConnected} class="live-dot" aria-hidden="true"></span>
					<strong>{$wsConnected ? 'Connected' : 'Offline'}</strong>
					{#if serverVersion}
						<span class="live-version" title="Server build">v{serverVersion}</span>
					{/if}
				</div>

				{#if sessionModeLine}
					<p class="live-modes">{sessionModeLine}</p>
				{/if}

				<div class="live-actions" role="group" aria-label="Session controls">
					<button
						class="queue-icon-btn queue-automix-btn"
						class:active={$automixEnabled}
						title={$automixEnabled ? 'Automix on' : 'Automix off'}
						aria-label={$automixEnabled ? 'Disable automix' : 'Enable automix'}
						aria-pressed={$automixEnabled}
						onclick={() => void togglePlayerAutomix()}
					>
						<svg width="13" height="13" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
							<path
								d="M7.5 1.6A5.1 5.1 0 0 0 2.4 6.7v1.2h-.2a1 1 0 0 0-1 1v2.1a1 1 0 0 0 1 1h1.5a.6.6 0 0 0 .6-.6V6.7a3.2 3.2 0 0 1 6.4 0v4.7a.6.6 0 0 0 .6.6h1.5a1 1 0 0 0 1-1V8.9a1 1 0 0 0-1-1h-.2V6.7A5.1 5.1 0 0 0 7.5 1.6z"
								fill="currentColor"
							/>
						</svg>
					</button>
					<button
						class="queue-icon-btn queue-discover-btn"
						class:active={$automixDiscoverNew}
						title={$automixDiscoverNew ? 'Include New: on - pulling in tracks outside your library' : 'Include New: off - tap to find new music during automix'}
						aria-label={$automixDiscoverNew ? 'Disable discover new' : 'Enable discover new'}
						aria-pressed={$automixDiscoverNew}
						onclick={() => void setPlayerDiscoverNew(!$automixDiscoverNew)}
					>
						<svg width="12" height="12" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
							<path d="M7.5 1a6.5 6.5 0 1 0 0 13A6.5 6.5 0 0 0 7.5 1zm0 1a5.5 5.5 0 1 1 0 11A5.5 5.5 0 0 1 7.5 2zM7 4.5V7H4.5a.5.5 0 0 0 0 1H7v2.5a.5.5 0 0 0 1 0V8h2.5a.5.5 0 0 0 0-1H8V4.5a.5.5 0 0 0-1 0z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd"/>
						</svg>
					</button>
					<button
						class="queue-icon-btn queue-help-btn"
						type="button"
						title="Keyboard shortcuts"
						aria-label="Keyboard shortcuts"
						aria-haspopup="dialog"
						aria-expanded={shortcutHelpOpen}
						onclick={openShortcutHelp}
					>?</button>
				</div>
			</div>

			<button class="theme-toggle btn btn-glass" onclick={toggleTheme}>
				{theme === 'dark' ? 'Switch to light' : 'Switch to dark'}
			</button>
		</div>
	</aside>

	<main class="workspace">
		{@render children()}
	</main>

	<VideoDock />

	{#if videoChromeActive}
		<aside class="now-playing-panel video-queue-panel" aria-label="Video queue">
			<div class="video-panel-top">
				<p class="eyebrow">Video session</p>
				<div class="video-panel-art-wrap">
					{#if currentVideoArtwork}
						<img
							class="video-panel-art"
							src={currentVideoArtwork}
							alt=""
							onerror={() => markArtworkFailed(currentVideoArtwork)}
						/>
					{:else}
						<div class="video-panel-art placeholder">▶</div>
					{/if}
				</div>
				<div class="video-panel-copy">
					<strong>{$videoSession.current?.title ?? 'Video queue'}</strong>
					<span>{$videoSession.current?.artist_name ?? formatVideoSourceLabel($videoSession.source, $videoSession.sourceLabel)}</span>
				</div>
				<div class="video-panel-actions">
					<button
						class="video-panel-chip"
						class:active={$videoSession.autoplay}
						type="button"
						aria-pressed={$videoSession.autoplay}
						onclick={() => requestVideoAutoplayToggle()}
					>
						› {$videoSession.autoplay ? 'On' : 'Autoplay'}
					</button>
					<span class="video-panel-source">{formatVideoSourceLabel($videoSession.source, $videoSession.sourceLabel)}</span>
				</div>
				{#if $videoSession.error}
					<p class="video-panel-error">{$videoSession.error}</p>
				{/if}
			</div>

			<section class="video-panel-queue">
				<div class="video-panel-queue-head">
					<span class="eyebrow">Queue</span>
					<span>{$videoSessionUpcoming.length} up next</span>
					<button
						class="video-panel-queue-clear"
						type="button"
						title="Clear video queue"
						onclick={() => requestVideoClear()}
					>⌫</button>
				</div>
				{#if $videoSession.queue.length > 0}
					<div class="video-panel-list">
						{#each $videoSession.queue.slice(0, 60) as video, i (`video-${video.tidal_id}-${i}`)}
							{@const videoArt = artworkCandidate(video.artwork_url, 320)}
							<button
								type="button"
								class="video-panel-row"
								class:active={$videoSession.current?.tidal_id === video.tidal_id}
								onclick={() => requestVideoJump(video.tidal_id)}
							>
								{#if videoArt}
									<img
										class="video-panel-row-art"
										src={videoArt}
										alt=""
										onerror={() => markArtworkFailed(videoArt)}
									/>
								{:else}
									<span class="video-panel-row-art placeholder">▶</span>
								{/if}
								<span class="video-panel-row-copy">
									<strong>{video.title}</strong>
									<span>{video.artist_name ?? 'Unknown artist'}</span>
								</span>
								<span class="video-panel-row-time">{formatTrackDuration(video.duration_ms ?? 0)}</span>
							</button>
						{/each}
					</div>
				{:else}
					<div class="queue-empty">
						<p>No video queue yet.</p>
						<span>Search or open a video mix to build one.</span>
					</div>
				{/if}
			</section>
		</aside>
	{:else}
	<aside
		class="now-playing-panel"
		class:queue-expanded={queueExpanded}
		oncontextmenu={openNowPlayingContextMenu}
	>
		<PlayerBar
			track={$currentTrack}
			streamDisplay={$currentStreamDisplay}
			nowPlayingAttribution={nowPlayingAttribution}
			streamDetail={streamDetailLabel}
			playerState={playerState}
			isScrubbing={isScrubbing}
			position={$position}
			bufferedMs={$buffered}
			isPlaying={$isPlaying}
			shuffleMode={$shuffleMode}
			repeatMode={$repeatMode}
			volume={$volume}
			displayVolume={displayVolume}
			playerError={$playerError}
			favoritePending={desktopFavoritePending}
			queueExpanded={queueExpanded}
			onEnterQuietMode={openQuietMode}
			onToggleFavorite={() => void handleDesktopFavoriteToggle()}
			onSeek={(p) => void setPlayerPosition(p)}
			onScrubStart={() => { isScrubbing = true; }}
			onScrubEnd={() => { isScrubbing = false; }}
			onCycleShuffle={() => void cyclePlayerShuffleMode()}
			onPrev={() => void playPreviousTrack()}
			onPlayPause={() => void togglePlayback()}
			onNext={() => void playNextTrack()}
			onCycleRepeat={() => void cyclePlayerRepeatMode()}
			onOpenMore={(anchor) => openNowPlayingMenuAt(anchor)}
			onToggleMute={() => void toggleMute()}
			onVolumePreview={(percent) => { displayVolume = percent; }}
			onVolumeChange={(nextVolume) => void setPlayerVolume(nextVolume)}
			onRetryPlayerError={async (retry) => { await retry(); }}
			onDismissPlayerError={() => playerError.set(null)}
		/>

		<section class="queue-section">
			<div class="queue-sr-status" role="status" aria-live="polite" aria-atomic="true">{$queueAnnouncement}</div>
			<div class="queue-header">
				<button
					class="queue-banner"
					type="button"
					onclick={toggleQueueExpanded}
					aria-expanded={queueExpanded}
					aria-controls="queue-list"
					title={queueExpanded ? 'Collapse queue' : 'Expand queue'}
				>
					{#if upcomingQueue.length > 0}
						<span class="queue-count-num">{upcomingQueue.length}</span>
						<span class="queue-count-unit">{queueTotalLabel}</span>
					{:else}
						<span class="queue-eyebrow">Up next</span>
						<span class="queue-count-unit">empty</span>
					{/if}
				</button>
				<div class="queue-header-actions">
					<button
						class="queue-icon-btn queue-save-btn"
						type="button"
						title="Save queue as playlist"
						aria-label="Save queue as playlist"
						onclick={openSaveQueue}
						disabled={upcomingQueue.length === 0 && !$currentTrack}
					>★</button>
					<button
						class="queue-icon-btn queue-clear-btn"
						type="button"
						title="Clear all upcoming tracks"
						aria-label="Clear queue"
						onclick={() => void handleClearQueue()}
						disabled={upcomingQueue.length === 0}
					>⌫</button>
					<button
						class="queue-icon-btn queue-expand-btn"
						type="button"
						title={queueExpanded ? 'Collapse queue' : 'Expand queue'}
						aria-label={queueExpanded ? 'Collapse queue' : 'Expand queue'}
						aria-expanded={queueExpanded}
						onclick={toggleQueueExpanded}
					>▲</button>
				</div>
			</div>

			{#if !currentRowVisible && $currentTrack && upcomingQueue.length > 0}
				<button
					class="queue-jump-chip"
					type="button"
					onclick={jumpToCurrentRow}
					title="Scroll to the track that is currently playing"
				>
					<span aria-hidden="true">↓</span>
					Jump to now playing
				</button>
			{/if}

			{#if $pendingUndo}
				<div class="queue-undo-bar" role="status">
					<span class="queue-undo-text">
						Cleared {$pendingUndo.count} {$pendingUndo.count === 1 ? 'track' : 'tracks'}
					</span>
					<button
						class="queue-undo-btn"
						type="button"
						onclick={() => void handleUndoClear()}
					>Undo<span class="queue-undo-hint" aria-hidden="true">Z</span></button>
				</div>
			{/if}

			{#if saveQueueOpen}
				<form
					class="queue-save-form"
					onsubmit={(event) => {
						event.preventDefault();
						void commitSaveQueue();
					}}
				>
					<input
						class="queue-save-input"
						type="text"
						placeholder="Playlist name"
						bind:value={saveQueueName}
						aria-label="Playlist name"
						use:focusOnMount
					/>
					<button class="queue-save-confirm" type="submit" disabled={savePending || !saveQueueName.trim()}>
						{savePending ? '…' : 'Save'}
					</button>
					<button
						class="queue-save-cancel"
						type="button"
						onclick={() => { saveQueueOpen = false; }}
					>Cancel</button>
				</form>
			{/if}

			{#if upcomingQueue.length > 0}
				<div class="queue-list" id="queue-list" role="list" bind:this={queueListEl} onscroll={handleQueueScroll}>
					{#each upcomingQueue.slice(0, queueVisibleCount) as item (item.id)}
						{@const aid = item.track.artist_id}
						{@const isPending = item.is_pending === true}
						<div
							role="listitem"
							class:active={isQueueItemActive(item, $currentTrack, $currentQueueItemId, upcomingQueue)}
							class:dragging={dragItemId === item.id}
							class:drag-over={dragOverItemId === item.id && dragItemId !== item.id}
							class:pending={isPending}
							class="queue-row"
							title={isPending ? 'Resolving on TIDAL...' : undefined}
							data-track-id={item.track.id}
							draggable={!isPending}
							oncontextmenu={(event) => openQueueRowMenu(item, event)}
							ondragstart={(event) => handleQueueDragStart(event, item)}
							ondragover={(event) => handleQueueDragOver(event, item)}
							ondragleave={() => handleQueueDragLeave(item)}
							ondrop={(event) => void handleQueueDrop(event, item)}
							ondragend={handleQueueDragEnd}
						>
							<!-- Full-bleed hit target: clicking anywhere on the row that
							     isn't an interactive child plays/jumps to this track. This is a
							     div, NOT a button, on purpose: a <button> is an interactive
							     element and swallows the row's native HTML5 dragstart, so the row
							     could only be dragged by the 12px grip. role/tabindex keep it
							     keyboard- and screen-reader-operable. -->
							<div
								class="queue-row-hit"
								role="button"
								tabindex={0}
								aria-label={isPending ? `Play ${item.track.title} (resolving)` : `Play ${item.track.title}`}
								onclick={() => void handleQueueTrackPlay(item)}
								onkeydown={(event) => handleQueueTrackKeydown(item, event)}
							></div>
							<span class="queue-grip" aria-hidden="true" title="Drag to reorder">⋮⋮</span>
							<div class="queue-art-wrap" title={formatQueueSource(item.source)}>
								{#if isPending}
									<div class="queue-art placeholder pending-art" title="Resolving track...">
										<span class="queue-spinner" aria-hidden="true"></span>
									</div>
								{:else}
									{@const queueArt = artworkCandidate(item.track.artwork_url, 320)}
									{#if queueArt}
										<img
											class="queue-art"
											src={queueArt}
											alt=""
											onerror={() => markArtworkFailed(queueArt)}
										/>
								{:else}
									<div class="queue-art placeholder">♫</div>
									{/if}
								{/if}
								<span class="queue-source-dot source-{queueSourceSlug(item.source)}" aria-hidden="true"></span>
							</div>

							<div class="queue-meta">
								<span class="queue-title">{item.track.title}</span>
								{#if isPending}
									<span class="queue-artist pending-label">
										<span class="queue-inline-spinner" aria-hidden="true"></span>
										Resolving on TIDAL...
									</span>
								{:else if aid && aid > 0}
									<a
										class="queue-artist"
										href="/artists/{aid}"
										onclick={stopPropagation}
										oncontextmenu={(event) => openQueueArtistContextMenu(item, event)}
									>{item.track.artist_name ?? 'Unknown artist'}</a>
								{:else}
									<span class="queue-artist">{item.track.artist_name ?? 'Unknown artist'}</span>
								{/if}
							</div>

							<div class="queue-side">
								<span class="queue-time">{formatTrackDuration(item.track.duration_ms)}</span>
								{#if !isPending}
									<button
										class="queue-overflow"
										aria-label="More actions"
										title="More actions"
										onclick={(event) => openQueueRowMenuFromButton(item, event)}
									>⋯</button>
								{/if}
							</div>
						</div>
					{/each}
				</div>
			{:else}
				<div class="queue-empty">
					<p>Nothing is lined up yet.</p>
					<span>Pick a track from <a class="queue-empty-link" href="/library">your library</a>, <a class="queue-empty-link" href="/genres">a genre</a>, or <a class="queue-empty-link" href="/playlists">a playlist</a>. Press <kbd class="queue-empty-key">Q</kbd> to collapse the queue.</span>
				</div>
			{/if}

			{#if upcomingQueue.length > queueVisibleCount}
				<button class="queue-load-more" type="button" onclick={loadMoreQueue}>
					Load {Math.min(QUEUE_LOAD_MORE_STEP, upcomingQueue.length - queueVisibleCount)} more
					<span class="queue-load-more-rest">({upcomingQueue.length - queueVisibleCount} waiting)</span>
				</button>
			{/if}
		</section>
	</aside>
	{/if}

	<!-- Mini player bar (mobile only) -->
	{#if $currentTrack && !videoChromeActive}
		<div class="mobile-mini-player-bar" aria-label="Mini player">
			<div class="mobile-mini-progress">
				<div class="mobile-mini-progress-fill" style="width: {progressWidth}"></div>
			</div>
			<div class="mobile-mini-inner">
				<button
					class="mobile-mini-track-btn"
					type="button"
					aria-label="Open now playing"
					onclick={() => { nowPlayingOpen = true; }}
					oncontextmenu={openNowPlayingContextMenu}
				>
					{#if mobileMiniArtwork}
						<img
							class="mobile-mini-art"
							src={mobileMiniArtwork}
							alt=""
							onerror={() => markArtworkFailed(mobileMiniArtwork)}
						/>
					{:else}
						<div class="mobile-mini-art placeholder">♫</div>
					{/if}
					<div class="mobile-mini-copy">
						<strong>{$currentTrack.title}</strong>
						<span>{$currentTrack.artist_name ?? 'Unknown artist'}</span>
					</div>
				</button>
				<div class="mobile-mini-controls">
					<button
						class="mobile-mini-btn"
						type="button"
						aria-label="Play or pause"
						onclick={() => void togglePlayback()}
					>
						{$isPlaying ? '⏸' : '▶'}
					</button>
					<button
						class="mobile-mini-btn"
						type="button"
						aria-label="Next"
						onclick={() => void playNextTrack()}
					>⏭</button>
				</div>
			</div>
		</div>
	{/if}

	<!-- Bottom tab bar (mobile only) -->
	<nav class="mobile-tab-bar" aria-label="Primary navigation">
		{#each MOBILE_TAB_ROUTES as item}
			<a
				href={item.path}
				class="mobile-tab"
				class:active={isNavItemActive(item.path)}
				aria-current={isNavItemActive(item.path) ? 'page' : undefined}
			>
				<span class="mobile-tab-icon">{item.icon}</span>
				<span class="mobile-tab-label">{item.id === 'genres' ? 'Genres' : item.label}</span>
			</a>
		{/each}
		<button
			class="mobile-tab"
			class:active={moreOpen}
			type="button"
			aria-expanded={moreOpen}
			aria-label="More navigation"
			onclick={() => { moreOpen = !moreOpen; }}
		>
			<span class="mobile-tab-icon">···</span>
			<span class="mobile-tab-label">More</span>
		</button>
	</nav>

	<!-- More sheet (mobile only) -->
	{#if moreOpen}
		<button
			class="mobile-more-backdrop"
			type="button"
			aria-label="Close more menu"
			onclick={() => { moreOpen = false; }}
		></button>
		<div class="mobile-more-sheet" role="dialog" aria-label="More navigation">
			<div class="mobile-more-handle"></div>
			<nav class="mobile-more-nav">
				{#each MOBILE_MORE_ROUTES as item}
					<a href={item.path} class="mobile-more-item" class:active={isNavItemActive(item.path)} onclick={() => { moreOpen = false; }}>
						<span class="mobile-more-icon">{item.icon}</span>
						<span>{item.label}</span>
					</a>
				{/each}
			</nav>
		</div>
	{/if}

	<!-- Now Playing sheet (mobile only) -->
	{#if nowPlayingOpen && $currentTrack && !videoChromeActive}
		<button
			class="mobile-np-backdrop"
			type="button"
			aria-label="Close now playing"
			onclick={() => { nowPlayingOpen = false; }}
		></button>
		<div class="mobile-np-sheet" role="dialog" aria-label="Now playing" aria-modal="true">
			<div class="mobile-np-handle"></div>

			<div class="mobile-np-art-wrap">
				{#key $currentTrack.artwork_url}
					{#if mobileNowPlayingArtwork}
						<img
							class="mobile-np-art"
							src={mobileNowPlayingArtwork}
							alt=""
							onerror={() => markArtworkFailed(mobileNowPlayingArtwork)}
						/>
					{:else}
						<div class="mobile-np-art placeholder">♫</div>
					{/if}
				{/key}
				{#if $currentStreamDisplay}
					<span class={`quality-badge mobile-np-quality ${getQualityClass($currentStreamDisplay.audio_quality)}`}>
						{formatQuality($currentStreamDisplay.audio_quality)}
					</span>
					{#if formatResolutionShort($currentStreamDisplay)}
						<span class="quality-badge mobile-np-resolution" title="Actual playback resolution (bit-depth / kHz)">
							{formatResolutionShort($currentStreamDisplay)}
						</span>
					{/if}
				{:else if $currentTrack.best_quality}
					<span class={`quality-badge mobile-np-quality ${getQualityClass($currentTrack.best_quality)}`}>
						{formatQuality($currentTrack.best_quality)}
					</span>
				{/if}
			</div>

			<div class="mobile-np-info">
				<div class="mobile-np-copy">
					<strong class="mobile-np-title">{$currentTrack.title}</strong>
					<span class="mobile-np-artist">{$currentTrack.artist_name ?? 'Unknown artist'}</span>
				</div>
				<button
					class="mobile-np-like"
					class:active={$currentTrack.is_favorite}
					type="button"
					aria-label={$currentTrack.is_favorite ? 'Remove from favorites' : 'Add to favorites'}
					disabled={mobileFavoritePending}
					onclick={() => void handleMobileFavoriteToggle()}
				>
					{$currentTrack.is_favorite ? '♥' : '♡'}
				</button>
			</div>

			<div class="mobile-np-scrub">
				<div class="mobile-np-scrub-track" style="--pct: {progressWidth}">
					<div class="mobile-np-scrub-fill" style="width: {progressWidth}"></div>
					<input
						class="mobile-np-scrub-input"
						type="range"
						min="0"
						max={$currentTrack.duration_ms ?? 0}
						step="1000"
						bind:value={scrubPosition}
						oninput={beginScrub}
						onchange={() => void commitScrub()}
						disabled={!$currentTrack.duration_ms}
						aria-label="Seek playback"
					/>
				</div>
				<div class="mobile-np-times">
					<span>{formatTrackDuration(scrubPosition)}</span>
					<span>{formatTrackDuration($currentTrack.duration_ms ?? 0)}</span>
				</div>
			</div>

			<div class="mobile-np-transport">
				<button class="mobile-np-btn" type="button" aria-label="Previous" onclick={() => void playPreviousTrack()}>⏮</button>
				<button class="mobile-np-btn primary" type="button" aria-label="Play or pause" onclick={() => void togglePlayback()}>
					{$isPlaying ? '⏸' : '▶'}
				</button>
				<button class="mobile-np-btn" type="button" aria-label="Next" onclick={() => void playNextTrack()}>⏭</button>
			</div>

			<div class="mobile-np-secondary">
				<button
					class="mobile-np-chip"
					class:active={$shuffleMode !== 'off'}
					type="button"
					aria-label={shuffleLabels[$shuffleMode]}
					onclick={() => void cyclePlayerShuffleMode()}
				>
					<span>{shuffleIcons[$shuffleMode]}</span>
					<span>{$shuffleMode === 'off' ? 'Shuffle' : shuffleLabels[$shuffleMode]}</span>
				</button>
				<button
					class="mobile-np-chip"
					class:active={$repeatMode !== 'off'}
					type="button"
					aria-label={repeatLabels[$repeatMode]}
					onclick={() => void cyclePlayerRepeatMode()}
				>
					<span>{repeatIcons[$repeatMode]}</span>
					<span>{$repeatMode === 'off' ? 'Repeat' : repeatLabels[$repeatMode]}</span>
				</button>
				<button
					class="mobile-np-chip"
					class:active={$automixEnabled}
					type="button"
					aria-label={$automixEnabled ? 'Disable automix' : 'Enable automix'}
					onclick={() => void togglePlayerAutomix()}
				>
					<span aria-hidden="true">
						<svg width="13" height="13" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg">
							<path
								d="M7.5 1.6A5.1 5.1 0 0 0 2.4 6.7v1.2h-.2a1 1 0 0 0-1 1v2.1a1 1 0 0 0 1 1h1.5a.6.6 0 0 0 .6-.6V6.7a3.2 3.2 0 0 1 6.4 0v4.7a.6.6 0 0 0 .6.6h1.5a1 1 0 0 0 1-1V8.9a1 1 0 0 0-1-1h-.2V6.7A5.1 5.1 0 0 0 7.5 1.6z"
								fill="currentColor"
							/>
						</svg>
					</span>
					<span>{$automixEnabled ? 'Automix on' : 'Automix'}</span>
				</button>
			</div>

			<div class="mobile-np-queue-header">
				<span class="eyebrow">Up next</span>
				<span class="mobile-np-queue-count">{queueCountLabel}</span>
			</div>

			{#if upcomingQueue.length > 0}
				<div class="mobile-np-queue-list" role="list">
					{#each upcomingQueue.slice(0, queueVisibleCount) as item (item.id)}
						{@const aid = item.track.artist_id}
						{@const isPending = item.is_pending === true}
						<div
							role="listitem"
							class="queue-row"
							class:active={isQueueItemActive(item, $currentTrack, $currentQueueItemId, upcomingQueue)}
							class:pending={isPending}
							title={isPending ? 'Resolving on TIDAL...' : undefined}
							oncontextmenu={(event) => openQueueRowMenu(item, event)}
						>
							<button
								class="queue-row-hit"
								type="button"
								aria-label={isPending ? `Play ${item.track.title} (resolving)` : `Play ${item.track.title}`}
								onclick={() => void handleQueueTrackPlay(item)}
								onkeydown={(event) => handleQueueTrackKeydown(item, event)}
							></button>
							<div class="queue-art-wrap" title={formatQueueSource(item.source)}>
								{#if isPending}
									<div class="queue-art placeholder pending-art" title="Resolving track...">
										<span class="queue-spinner" aria-hidden="true"></span>
									</div>
								{:else}
									{@const queueArt = artworkCandidate(item.track.artwork_url, 320)}
									{#if queueArt}
										<img
											class="queue-art"
											src={queueArt}
											alt=""
											onerror={() => markArtworkFailed(queueArt)}
										/>
								{:else}
									<div class="queue-art placeholder">♫</div>
									{/if}
								{/if}
								<span class="queue-source-dot source-{queueSourceSlug(item.source)}" aria-hidden="true"></span>
							</div>
							<div class="queue-meta">
								<span class="queue-title">{item.track.title}</span>
								{#if isPending}
									<span class="queue-artist pending-label">
										<span class="queue-inline-spinner" aria-hidden="true"></span>
										Resolving on TIDAL...
									</span>
								{:else if aid && aid > 0}
									<a
										class="queue-artist"
										href="/artists/{aid}"
										onclick={stopPropagation}
										oncontextmenu={(event) => openQueueArtistContextMenu(item, event)}
									>{item.track.artist_name ?? 'Unknown artist'}</a>
								{:else}
									<span class="queue-artist">{item.track.artist_name ?? 'Unknown artist'}</span>
								{/if}
							</div>
							<div class="queue-side">
								<span class="queue-time">{formatTrackDuration(item.track.duration_ms)}</span>
								<button
									class="queue-overflow"
									aria-label="More actions"
									title="More actions"
									onclick={(e) => openQueueRowMenuFromButton(item, e)}
								>⋯</button>
							</div>
						</div>
					{/each}
				</div>
			{:else}
				<div class="queue-empty">
					<p>Nothing is lined up yet.</p>
					<span>Pick a track from <a class="queue-empty-link" href="/library">your library</a>, <a class="queue-empty-link" href="/genres">a genre</a>, or <a class="queue-empty-link" href="/playlists">a playlist</a>. Press <kbd class="queue-empty-key">Q</kbd> to collapse the queue.</span>
				</div>
			{/if}

			{#if upcomingQueue.length > queueVisibleCount}
				<button class="queue-load-more" type="button" onclick={loadMoreQueue}>
					Load {Math.min(QUEUE_LOAD_MORE_STEP, upcomingQueue.length - queueVisibleCount)} more
					<span class="queue-load-more-rest">({upcomingQueue.length - queueVisibleCount} waiting)</span>
				</button>
			{/if}
		</div>
	{/if}
</div>
{/if}

<style>
	.onboarding-check {
		position: fixed;
		inset: 0;
		display: flex;
		flex-direction: column;
		align-items: center;
		justify-content: center;
		gap: 18px;
		background: radial-gradient(circle at 50% 35%, #1a1f2e 0%, #0a0d14 65%, #05070b 100%);
		color: #8b93a7;
		font-family: var(--font-body);
		z-index: 50;
	}
	.onboarding-check .check-mark {
		width: 72px;
		height: 72px;
		object-fit: contain;
		opacity: 0.85;
		animation: noor-check-pulse 1.8s ease-in-out infinite;
	}
	.onboarding-check p {
		margin: 0;
		font-size: var(--font-size-sm);
		letter-spacing: 0.18em;
		text-transform: uppercase;
	}
	@keyframes noor-check-pulse {
		0%, 100% { opacity: 0.85; transform: scale(1); }
		50%      { opacity: 1;    transform: scale(1.04); }
	}

	.wallpaper-layer {
		position: fixed;
		inset: 0;
		z-index: 0;
		pointer-events: none;
		filter: blur(var(--wallpaper-blur, 10px)) saturate(1.08);
		transform: scale(var(--wallpaper-scale, 1.025));
		transform-origin: center;
	}

	:global([data-theme="light"]) .wallpaper-layer {
		opacity: 0.22;
		filter: blur(var(--wallpaper-blur, 10px)) saturate(0.78);
	}

	.remote-shell {
		position: relative;
		z-index: 1;
		/* Viewport-sized flex column with no body scroll. The page's main
		 * area becomes the single bounded scroll container — keeps iOS
		 * Safari from "latching" the scroll mid-list on long pages
		 * (a known issue with very tall body-scrolled content in PWA
		 * standalone mode). */
		height: 100dvh;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		background: var(--surface-0);
		color: var(--text-primary);
		/* The remote is a phone-first surface; long-press on track titles,
		 * artist text, etc. should never trigger iOS text selection — it's
		 * useless for a remote and gets in the way of the long-press menu.
		 * Suppress selection across the whole shell and opt back in only on
		 * actual text inputs (search bars, filter fields). */
		user-select: none;
		-webkit-user-select: none;
		-webkit-touch-callout: none;
	}

	.remote-shell :global(input),
	.remote-shell :global(textarea) {
		user-select: text;
		-webkit-user-select: text;
		-webkit-touch-callout: default;
	}

	.app-shell {
		position: relative;
		z-index: 1;
		height: 100dvh;
		min-height: 100dvh;
		width: 100%;
		min-width: 0;
		display: grid;
		grid-template-columns: var(--sidebar-width) minmax(0, 1fr) var(--panel-width);
		overflow: hidden;
		/* Force own compositor layer; works around a wry/WKWebView hit-testing
		   quirk on macOS where buttons in the windowed shell stay unclickable
		   until the window enters native fullscreen. */
		transform: translateZ(0);
		background:
			radial-gradient(circle at 8% 6%, var(--atlas-haze-a), transparent 34%),
			radial-gradient(circle at 90% 10%, var(--atlas-haze-b), transparent 28%),
			radial-gradient(circle at 76% 86%, var(--atlas-haze-c), transparent 30%),
			var(--atlas-bg);
	}

	.app-shell.has-wallpaper {
		background: transparent;
	}

	.app-shell.has-wallpaper .sidebar,
	.app-shell.has-wallpaper .now-playing-panel {
		backdrop-filter: blur(var(--wallpaper-blur, 10px)) saturate(1.12);
		-webkit-backdrop-filter: blur(var(--wallpaper-blur, 10px)) saturate(1.12);
	}

	/* The workspace scrim has to actually hide the shader: at 0.44 the moire
	   read straight through card artwork and shelf titles. */
	.app-shell.has-wallpaper .workspace {
		background: rgba(9, 9, 14, 0.62);
		backdrop-filter: blur(var(--wallpaper-blur, 10px)) saturate(1.1);
		-webkit-backdrop-filter: blur(var(--wallpaper-blur, 10px)) saturate(1.1);
	}

	:global([data-theme="light"]) .app-shell.has-wallpaper .sidebar {
		background:
			linear-gradient(180deg, rgba(255, 255, 255, 0.9), rgba(248, 250, 252, 0.82)),
			var(--sidebar-bg);
	}

	:global([data-theme="light"]) .app-shell.has-wallpaper .workspace {
		background: rgba(248, 250, 252, 0.9);
	}

	:global([data-theme="light"]) .app-shell.has-wallpaper .now-playing-panel {
		background:
			linear-gradient(180deg, rgba(255, 255, 255, 0.88), rgba(248, 250, 252, 0.84)),
			var(--right-panel-bg);
	}

	.sidebar {
		display: flex;
		flex-direction: column;
		padding: 20px 14px;
		border-right: 1px solid var(--border-subtle);
		background:
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface) 85%, transparent), color-mix(in srgb, var(--instrument-surface-strong) 72%, transparent)),
			var(--sidebar-bg);
		overflow-y: auto;
		-webkit-overflow-scrolling: touch;
	}

	.workspace {
		overflow-y: auto;
		/* Reserve the scrollbar gutter permanently. Without it, switching to a
		   view that overflows steals ~5px of width and every centered element
		   (the search field, the filter pills) jumps sideways. */
		scrollbar-gutter: stable;
		padding: 28px 30px 48px;
		min-width: 0;
		-webkit-overflow-scrolling: touch;
	}

	.now-playing-panel {
		display: flex;
		flex-direction: column;
		border-left: 1px solid var(--border-subtle);
		background:
			linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface) 70%, transparent), color-mix(in srgb, var(--instrument-surface-strong) 84%, transparent)),
			var(--right-panel-bg);
		overflow: hidden;
	}

	/* ── Mobile-only elements: hidden at desktop ─────────── */
	.video-queue-panel {
		padding: 18px;
		gap: 16px;
	}

	.video-panel-top,
	.video-panel-queue {
		min-width: 0;
	}

	.video-panel-top {
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.video-panel-art-wrap {
		aspect-ratio: 16 / 9;
		width: 100%;
		border-radius: 8px;
		overflow: hidden;
		background: color-mix(in srgb, var(--instrument-surface-strong) 75%, transparent);
		border: 1px solid var(--border-subtle);
	}

	.video-panel-art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}

	.video-panel-art.placeholder {
		display: flex;
		align-items: center;
		justify-content: center;
		color: var(--text-secondary);
		font-size: var(--font-size-2xl);
	}

	.video-panel-copy {
		display: flex;
		flex-direction: column;
		gap: 4px;
		min-width: 0;
	}

	.video-panel-copy strong {
		color: var(--text-primary);
		font-size: var(--font-size-md);
		line-height: var(--line-height-snug);
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.video-panel-copy span,
	.video-panel-source,
	.video-panel-queue-head {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.video-panel-actions {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 10px;
	}

	.video-panel-chip {
		border: 1px solid var(--border-subtle);
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 80%, transparent);
		color: var(--text-primary);
		font: inherit;
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-bold);
		padding: 6px 10px;
		cursor: pointer;
	}

	.video-panel-chip.active {
		border-color: color-mix(in srgb, var(--accent-line) 70%, transparent);
		background: color-mix(in srgb, var(--accent-soft) 75%, transparent);
	}

	.video-panel-error {
		margin: 0;
		color: var(--state-error);
		font-size: var(--font-size-xs);
	}

	.video-panel-queue {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		border-top: 1px solid var(--border-subtle);
		padding-top: 14px;
	}

	.video-panel-queue-head {
		display: flex;
		justify-content: space-between;
		align-items: center;
		gap: 10px;
		padding-bottom: 10px;
	}

	.video-panel-queue-clear {
		margin-left: auto;
		background: none;
		border: none;
		color: var(--text-muted);
		cursor: pointer;
		font-size: var(--font-size-sm);
		padding: 0.15rem 0.3rem;
		border-radius: 4px;
		line-height: 1;
	}
	.video-panel-queue-clear:hover {
		color: var(--text-primary);
		background: var(--bg-hover);
	}

	.video-panel-list {
		display: flex;
		flex-direction: column;
		gap: 6px;
		overflow-y: auto;
		padding-right: 2px;
	}

	.video-panel-row {
		width: 100%;
		min-width: 0;
		display: grid;
		grid-template-columns: 48px minmax(0, 1fr) auto;
		align-items: center;
		gap: 10px;
		border: 1px solid transparent;
		border-radius: 8px;
		background: transparent;
		color: inherit;
		font: inherit;
		text-align: left;
		padding: 7px;
		cursor: pointer;
	}

	.video-panel-row:hover,
	.video-panel-row:focus-visible,
	.video-panel-row.active {
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		border-color: var(--border-subtle);
		outline: none;
	}

	.video-panel-row.active {
		border-color: color-mix(in srgb, var(--accent-line) 60%, transparent);
	}

	.video-panel-row-art {
		width: 48px;
		aspect-ratio: 16 / 9;
		border-radius: 4px;
		object-fit: cover;
		background: color-mix(in srgb, var(--instrument-surface-strong) 85%, transparent);
	}

	.video-panel-row-art.placeholder {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		color: var(--text-tertiary);
		font-size: var(--font-size-sm);
	}

	.video-panel-row-copy {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.video-panel-row-copy strong,
	.video-panel-row-copy span {
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.video-panel-row-copy strong {
		font-size: var(--font-size-sm);
		color: var(--text-primary);
	}

	.video-panel-row-copy span,
	.video-panel-row-time {
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
	}

	.mobile-top-bar,
	.mobile-mini-player-bar,
	.mobile-tab-bar,
	.mobile-more-backdrop,
	.mobile-more-sheet,
	.mobile-np-backdrop,
	.mobile-np-sheet {
		display: none;
	}

	.brand {
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 4px 6px 18px;
	}

	.brand-splash {
		display: block;
		width: 100%;
		max-width: 160px;
		height: auto;
		margin: 0 auto;
	}
	/* Original white wordmark on dark (default); dark-text recolour only on light. */
	.brand-splash-on-light { display: none; }
	:global([data-theme='light']) .brand-splash-on-light { display: block; }
	:global([data-theme='light']) .brand-splash-on-dark { display: none; }

	.sidebar-footer {
		margin-top: auto;
		padding: 18px 6px 0;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.live-status {
		display: flex;
		flex-direction: column;
		gap: 8px;
		padding: 8px 10px;
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--instrument-surface) 80%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 52%, transparent);
	}

	.live-status-head {
		display: flex;
		align-items: center;
		gap: 8px;
		min-width: 0;
	}

	.live-status-head strong {
		font-size: var(--font-size-xs);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.live-version {
		margin-left: auto;
		flex-shrink: 0;
		color: var(--text-tertiary);
		font-size: var(--font-size-2xs);
		font-variant-numeric: tabular-nums;
	}

	.live-modes {
		color: var(--signal-text);
		font-size: var(--font-size-2xs);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.live-actions {
		display: flex;
		align-items: center;
		gap: 6px;
	}

	.live-dot {
		width: 8px;
		height: 8px;
		border-radius: 50%;
		background: var(--state-success);
		box-shadow: 0 0 0 4px color-mix(in srgb, var(--state-success) 18%, transparent);
		flex-shrink: 0;
	}

	.live-dot.offline {
		background: var(--text-muted);
		box-shadow: none;
	}

	.theme-toggle {
		width: 100%;
	}

	.queue-section {
		flex: 1;
		display: flex;
		flex-direction: column;
		overflow: hidden;
		border-top: 1px solid var(--border-subtle);
		margin-top: 16px;
		padding: 16px;
	}

	.queue-sr-status {
		position: absolute;
		width: 1px;
		height: 1px;
		padding: 0;
		margin: -1px;
		overflow: hidden;
		clip: rect(0, 0, 0, 0);
		white-space: nowrap;
		border: 0;
	}

	.queue-jump-chip {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		align-self: center;
		margin: 0 0 10px;
		padding: 6px 14px;
		border-radius: 999px;
		border: 1px solid var(--accent-line);
		background: color-mix(in srgb, var(--accent-soft) 80%, transparent);
		color: var(--accent-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: background var(--motion-fast), transform var(--motion-fast);
	}

	.queue-jump-chip:hover {
		background: var(--accent-soft);
		transform: translateY(-1px);
	}

	.queue-undo-bar {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		margin: 0 0 10px;
		padding: 8px 12px;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--accent-soft) 70%, transparent);
		border: 1px solid var(--accent-line);
		animation: queue-undo-slide-in 180ms ease-out;
	}

	@keyframes queue-undo-slide-in {
		from { opacity: 0; transform: translateY(-4px); }
		to { opacity: 1; transform: translateY(0); }
	}

	.queue-undo-text {
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
	}

	.queue-undo-btn {
		display: inline-flex;
		align-items: center;
		gap: 8px;
		padding: 5px 12px;
		border-radius: 999px;
		border: 1px solid var(--accent-line);
		background: var(--accent-strong);
		color: var(--bg-base);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		cursor: pointer;
		transition: background var(--motion-fast), transform var(--motion-fast);
	}

	.queue-undo-btn:hover {
		transform: translateY(-1px);
	}

	.queue-undo-hint {
		display: inline-grid;
		place-items: center;
		min-width: 18px;
		height: 18px;
		padding: 0 4px;
		border-radius: 4px;
		background: color-mix(in srgb, var(--bg-base) 22%, transparent);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
		letter-spacing: 0.04em;
	}

	/* Honor OS-level motion-reduction. We still allow opacity transitions
	   (they're conveying state changes that the user actively triggered),
	   but flatten translates, rotations, and the spinner so vestibular
	   users don't get unwanted motion in the queue surface. */
	@media (prefers-reduced-motion: reduce) {
		.queue-row,
		.queue-row:hover,
		.queue-row:focus-within,
		.queue-icon-btn:hover:not(:disabled),
		.queue-undo-btn:hover,
		.queue-jump-chip:hover,
		.queue-expand-btn:hover:not(:disabled) {
			transform: none;
		}
		.now-playing-panel.queue-expanded .queue-expand-btn {
			transform: none;
		}
		.queue-spinner,
		.queue-inline-spinner {
			animation: none;
		}
		.queue-undo-bar {
			animation: none;
		}
	}

	.queue-header {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 12px;
		padding-bottom: 12px;
	}

	.queue-banner {
		flex: 1 1 auto;
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 4px 6px;
		margin: -4px -6px;
		background: transparent;
		border: 1px solid transparent;
		border-radius: 8px;
		text-align: left;
		color: inherit;
		cursor: pointer;
		min-width: 0;
		white-space: nowrap;
		overflow: hidden;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.queue-banner:hover {
		background: var(--bg-hover);
		border-color: var(--border-subtle);
	}

	/* The count is a circled number: the word "tracks" used to eat the row and
	   push the duration into an ellipsis. */
	.queue-count-num {
		flex: 0 0 auto;
		display: grid;
		place-items: center;
		min-width: 26px;
		height: 26px;
		padding: 0 6px;
		border-radius: 999px;
		border: 1px solid var(--border-strong);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		line-height: 1;
	}

	.queue-count-unit {
		flex: 1 1 auto;
		min-width: 0;
		font-size: var(--font-size-xs);
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1;
	}

	.queue-header-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-shrink: 0;
	}

	.queue-automix-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		box-shadow: 0 0 10px var(--accent), 0 0 0 1px var(--accent-line);
	}

	.queue-discover-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
		box-shadow: 0 0 10px var(--accent), 0 0 0 1px var(--accent-line);
	}

	.queue-icon-btn {
		width: 24px;
		height: 24px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: 1;
		flex-shrink: 0;
		cursor: pointer;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			transform var(--motion-fast);
	}

	.queue-icon-btn:hover:not(:disabled) {
		background: var(--bg-hover);
		border-color: var(--border-strong);
		color: var(--text-primary);
	}

	.queue-icon-btn:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	.queue-expand-btn {
		color: var(--accent-strong);
		border-color: var(--accent-line);
		background: var(--accent-soft);
	}

	.queue-expand-btn:hover:not(:disabled) {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
		transform: translateY(-1px);
	}

	.now-playing-panel.queue-expanded .queue-expand-btn {
		transform: rotate(180deg);
	}

	.queue-clear-btn:hover:not(:disabled) {
		background: color-mix(in srgb, var(--state-error) 14%, transparent);
		border-color: color-mix(in srgb, var(--state-error) 36%, transparent);
		color: var(--state-error);
	}

	.queue-save-form {
		display: flex;
		align-items: center;
		gap: 6px;
		padding: 6px 0 10px;
	}

	.queue-save-input {
		flex: 1;
		min-width: 0;
		padding: 6px 10px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: var(--bg-surface);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
	}

	.queue-save-input:focus {
		outline: none;
		border-color: var(--accent-line);
		box-shadow: 0 0 0 2px color-mix(in srgb, var(--accent-glow) 40%, transparent);
	}

	.queue-save-confirm,
	.queue-save-cancel {
		height: 28px;
		padding: 0 10px;
		border-radius: 999px;
		border: 1px solid var(--border-subtle);
		background: var(--bg-surface);
		color: var(--text-primary);
		font-size: var(--font-size-xs);
		cursor: pointer;
	}

	.queue-save-confirm {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.queue-save-confirm:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.queue-eyebrow {
		color: var(--text-tertiary);
		font-size: var(--font-size-xs);
		text-transform: uppercase;
		letter-spacing: 0.08em;
		margin-bottom: 2px;
	}

	.queue-list {
		flex: 1;
		overflow-y: auto;
		display: flex;
		flex-direction: column;
		gap: 6px;
		-webkit-overflow-scrolling: touch;
	}

	.queue-row {
		position: relative;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 46%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		transition:
			border-color var(--motion-fast),
			background var(--motion-fast),
			transform var(--motion-fast);
	}

	/* Full-bleed click target sits behind the row content. Non-interactive
	   content (art, title, time) has pointer-events:none so clicks fall through
	   to it; interactive children (grip, artist link, overflow) re-enable. */
	.queue-row-hit {
		position: absolute;
		inset: 0;
		z-index: 0;
		margin: 0;
		padding: 0;
		border: none;
		background: transparent;
		border-radius: inherit;
		cursor: pointer;
	}

	.queue-row-hit:focus-visible {
		outline: 2px solid var(--accent-line);
		outline-offset: -2px;
	}

	.queue-row > .queue-grip,
	.queue-row > .queue-art-wrap,
	.queue-row > .queue-meta,
	.queue-row > .queue-side {
		position: relative;
		z-index: 1;
	}

	.queue-art-wrap,
	.queue-meta,
	.queue-time {
		pointer-events: none;
	}

	.queue-grip,
	.queue-meta .queue-artist[href],
	.queue-overflow {
		pointer-events: auto;
	}

	.queue-row:hover,
	.queue-row:focus-within {
		border-color: color-mix(in srgb, var(--instrument-border) 72%, transparent);
		background: color-mix(in srgb, var(--instrument-surface-strong) 86%, transparent);
		transform: translateY(-1px);
	}

	.queue-row.active .queue-title {
		color: var(--accent-strong);
	}

	.queue-row.dragging {
		opacity: 0.4;
		cursor: grabbing;
	}

	/* The dropped row lands at the target's index, i.e. above it, so the
	   accent line sits on the target's top edge to read as "drops here". */
	.queue-row.drag-over {
		border-color: var(--accent-line);
		background: color-mix(in srgb, var(--accent-soft) 55%, transparent);
		box-shadow: inset 0 2px 0 var(--accent-strong);
	}

	.queue-row.pending {
		cursor: default;
		opacity: 0.78;
	}

	.queue-row.pending:hover,
	.queue-row.pending:focus-within {
		transform: none;
	}

	.queue-row.pending .queue-title {
		color: var(--text-secondary);
	}

	.queue-art.placeholder.pending-art {
		opacity: 0.7;
	}

	.queue-spinner {
		width: 16px;
		height: 16px;
		border-radius: 50%;
		border: 2px solid var(--border-subtle, rgba(255, 255, 255, 0.15));
		border-top-color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		animation: queue-spinner-spin 0.9s linear infinite;
	}

	@keyframes queue-spinner-spin {
		to { transform: rotate(360deg); }
	}

	.queue-grip {
		flex-shrink: 0;
		width: 12px;
		text-align: center;
		font-size: var(--font-size-xs);
		line-height: 1;
		color: var(--text-tertiary);
		cursor: grab;
		opacity: 0.35;
		transition: opacity var(--motion-fast);
		user-select: none;
	}

	.queue-row:hover .queue-grip,
	.queue-row:focus-within .queue-grip {
		opacity: 0.8;
	}

	.queue-row.dragging .queue-grip {
		cursor: grabbing;
	}

	.queue-art-wrap {
		position: relative;
		flex-shrink: 0;
		line-height: 0;
	}

	.queue-art {
		width: 42px;
		height: 42px;
		border-radius: 12px;
		object-fit: cover;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		display: block;
	}

	.queue-art.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	/* The dot in the bottom-right of queue artwork encodes where the track came
	   from; its colours live in app.css so the legend on the automix page can
	   reuse them. Tooltip on .queue-art-wrap names the source. */

	.queue-meta {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.queue-title {
		font-weight: var(--font-weight-semibold);
		font-size: var(--font-size-sm);
		line-height: var(--line-height-snug);
		margin: 0;
		/* Two-line clamp lets long titles breathe instead of chopping words. */
		display: -webkit-box;
		-webkit-box-orient: vertical;
		-webkit-line-clamp: 2;
		line-clamp: 2;
		overflow: hidden;
		overflow-wrap: anywhere;
		word-break: break-word;
	}

	.queue-artist {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		line-height: var(--line-height-snug);
		text-decoration: none;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100%;
	}

	a.queue-artist {
		cursor: pointer;
	}

	a.queue-artist:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.queue-artist.pending-label {
		display: inline-flex;
		align-items: center;
		gap: 6px;
		color: var(--text-tertiary);
	}

	.queue-inline-spinner {
		width: 10px;
		height: 10px;
		border-radius: 999px;
		border: 1.5px solid var(--border-subtle, rgba(255, 255, 255, 0.15));
		border-top-color: var(--text-secondary, rgba(255, 255, 255, 0.7));
		animation: queue-spinner-spin 0.9s linear infinite;
		flex-shrink: 0;
	}

	.queue-time,
	.queue-empty span {
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
	}

	.queue-side {
		display: flex;
		align-items: center;
		justify-content: flex-end;
		gap: 6px;
		flex-shrink: 0;
		margin-left: auto;
	}

	/* Single overflow button replaces the old cluster of hover pills: low-key by
	   default, brightens on row hover/focus. The context menu holds every action
	   (play next, favourite, radio, remove), so the row stays calm. */
	.queue-overflow {
		width: 28px;
		height: 28px;
		padding: 0;
		display: inline-grid;
		place-items: center;
		border-radius: 999px;
		border: 1px solid transparent;
		background: transparent;
		color: var(--text-tertiary);
		font-size: var(--font-size-md);
		line-height: 1;
		cursor: pointer;
		opacity: 0.55;
		transition: background var(--motion-fast), color var(--motion-fast),
			border-color var(--motion-fast), opacity var(--motion-fast);
	}

	.queue-row:hover .queue-overflow,
	.queue-row:focus-within .queue-overflow {
		opacity: 1;
	}

	.queue-overflow:hover {
		background: color-mix(in srgb, var(--instrument-surface-strong) 92%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 70%, transparent);
		color: var(--text-primary);
	}

	.queue-empty {
		padding: 18px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.queue-empty p {
		font-weight: var(--font-weight-semibold);
	}

	.queue-empty-link {
		color: var(--text-secondary);
		text-decoration: underline;
		text-underline-offset: 2px;
	}

	.queue-empty-link:hover {
		color: var(--text-primary);
	}

	.queue-empty-key {
		display: inline-grid;
		place-items: center;
		min-width: 16px;
		padding: 0 4px;
		margin: 0 2px;
		border-radius: 4px;
		background: var(--bg-elevated, rgba(255, 255, 255, 0.06));
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-family: var(--font-mono, monospace);
		font-size: var(--font-size-2xs);
		font-weight: var(--font-weight-bold);
	}

	.queue-load-more {
		margin-top: 12px;
		padding: 8px 16px;
		border-radius: 999px;
		border: 1px dashed var(--border-strong);
		background: color-mix(in srgb, var(--instrument-surface) 60%, transparent);
		color: var(--text-secondary);
		font-size: var(--font-size-xs);
		font-weight: var(--font-weight-medium);
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast);
		align-self: center;
	}

	.queue-load-more:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
		border-color: var(--accent-line);
	}

	.queue-load-more-rest {
		margin-left: 6px;
		color: var(--text-tertiary);
	}

	@media (max-width: 1320px) {
		.app-shell {
			grid-template-columns: var(--sidebar-width) minmax(0, 1fr) minmax(280px, 32vw);
		}

		.workspace {
			padding: 24px 24px 40px;
		}
	}

	/* ── Mobile layout (≤ 1180px) ────────────────────────── */
	@media (max-width: 1180px) {
		/* Show mobile chrome */
		.mobile-top-bar { display: flex; }
		.mobile-tab-bar { display: flex; }

		/* App shell: single col, body is scroll container */
		.app-shell {
			height: auto;
			min-height: 100dvh;
			grid-template-columns: 1fr;
			background: transparent;
		}

		/* Sidebar hidden — nav lives in bottom tab bar */
		.sidebar { display: none; }

		/* Right panel hidden — player lives in mini player + NP sheet */
		.now-playing-panel { display: none; }

		/* Workspace: clears fixed bottom chrome */
		.workspace {
			padding: 16px 16px 0;
			padding-bottom: calc(var(--mob-bottom-chrome) + var(--safe-bottom) + 16px);
			overflow: visible;
			min-width: 0;
		}

		/* When no track playing, only clear tab bar */
		.app-shell:not(.mobile-player-active) .workspace {
			padding-bottom: calc(var(--mob-tab-bar-h) + var(--safe-bottom) + 16px);
		}

		/* ── Mobile top bar ── */
		.mobile-top-bar {
			align-items: center;
			justify-content: space-between;
			padding: 0 16px;
			height: var(--mob-top-bar-h);
			border-bottom: 1px solid var(--border-subtle);
			background: var(--bg-base);
			position: sticky;
			top: 0;
			z-index: 10;
			flex-shrink: 0;
		}

		.mobile-brand {
			display: flex;
			align-items: center;
			gap: 8px;
			text-decoration: none;
			color: inherit;
		}

		.mobile-brand-mark {
			width: 28px;
			height: 28px;
			display: grid;
			place-items: center;
			flex-shrink: 0;
		}

		.mobile-brand-mark img {
			width: 100%;
			height: 100%;
			object-fit: contain;
		}

		.mobile-brand-name {
			font-family: var(--font-display);
			font-size: var(--font-size-md);
			letter-spacing: 0.03em;
		}

		.mobile-theme-btn {
			font-size: var(--font-size-xs);
			padding: 6px 12px;
		}

		/* ── Mini player bar ── */
		.mobile-mini-player-bar {
			position: fixed;
			bottom: calc(var(--mob-tab-bar-h) + var(--safe-bottom));
			left: 0;
			right: 0;
			height: var(--mob-mini-player-h);
			background: var(--bg-elevated);
			border-top: 1px solid var(--border-subtle);
			box-shadow: 0 -4px 16px rgba(0, 0, 0, 0.18);
			z-index: 30;
			display: flex;
			flex-direction: column;
			animation: mini-player-in 200ms cubic-bezier(0.25, 0.8, 0.25, 1) both;
		}

		@keyframes mini-player-in {
			from { transform: translateY(100%); opacity: 0; }
			to   { transform: translateY(0);   opacity: 1; }
		}

		.mobile-mini-progress {
			height: 2px;
			background: var(--border-subtle);
			flex-shrink: 0;
		}

		.mobile-mini-progress-fill {
			height: 100%;
			background: var(--accent);
			transition: width 1000ms linear;
		}

		.mobile-mini-inner {
			flex: 1;
			display: flex;
			align-items: center;
			padding: 0 12px;
			gap: 4px;
			min-width: 0;
		}

		.mobile-mini-track-btn {
			flex: 1;
			display: flex;
			align-items: center;
			gap: 12px;
			min-width: 0;
			text-align: left;
			padding: 0;
			background: none;
			border: none;
			color: inherit;
			cursor: pointer;
		}

		.mobile-mini-art {
			width: 40px;
			height: 40px;
			border-radius: 8px;
			object-fit: cover;
			border: 1px solid var(--border-subtle);
			flex-shrink: 0;
			background: var(--bg-surface);
		}

		.mobile-mini-art.placeholder {
			display: grid;
			place-items: center;
			color: var(--text-tertiary);
			font-size: var(--font-size-md);
		}

		.mobile-mini-copy {
			display: flex;
			flex-direction: column;
			min-width: 0;
			gap: 2px;
		}

		.mobile-mini-copy strong {
			font-size: var(--font-size-sm);
			font-weight: var(--font-weight-semibold);
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			color: var(--text-primary);
		}

		.mobile-mini-copy span {
			font-size: var(--font-size-xs);
			color: var(--text-secondary);
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
		}

		.mobile-mini-controls {
			display: flex;
			align-items: center;
			gap: 2px;
			flex-shrink: 0;
		}

		.mobile-mini-btn {
			width: 42px;
			height: 42px;
			border-radius: 50%;
			display: grid;
			place-items: center;
			background: none;
			border: none;
			color: var(--text-primary);
			font-size: var(--font-size-lg);
			cursor: pointer;
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-mini-btn:active {
			opacity: 0.6;
			transform: scale(0.92);
		}

		/* ── Bottom tab bar ── */
		.mobile-tab-bar {
			position: fixed;
			bottom: 0;
			left: 0;
			right: 0;
			height: calc(var(--mob-tab-bar-h) + var(--safe-bottom));
			padding-bottom: var(--safe-bottom);
			background: var(--bg-elevated);
			border-top: 1px solid var(--border-subtle);
			align-items: flex-start;
			z-index: 31;
		}

		.mobile-tab {
			flex: 1;
			display: flex;
			flex-direction: column;
			align-items: center;
			justify-content: center;
			gap: 4px;
			padding: 8px 0 0;
			height: var(--mob-tab-bar-h);
			color: var(--text-tertiary);
			border: none;
			background: none;
			text-decoration: none;
			cursor: pointer;
			transition: color var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-tab.active { color: var(--accent-strong); }

		.mobile-tab-icon {
			font-size: var(--font-size-lg);
			line-height: 1;
			display: block;
			position: relative;
		}

		.mobile-tab.active .mobile-tab-icon::before {
			content: '';
			position: absolute;
			top: -7px;
			left: 50%;
			transform: translateX(-50%);
			width: 18px;
			height: 2px;
			border-radius: 999px;
			background: var(--accent);
		}

		.mobile-tab-label {
			font-size: var(--font-size-2xs);
			font-weight: var(--font-weight-semibold);
			letter-spacing: 0.02em;
			line-height: 1;
			display: block;
		}

		/* ── More sheet ── */
		.mobile-more-backdrop {
			position: fixed;
			inset: 0;
			background: rgba(0, 0, 0, 0.4);
			z-index: 40;
			border: none;
			padding: 0;
			cursor: default;
		}

		.mobile-more-sheet {
			position: fixed;
			left: 0;
			right: 0;
			bottom: calc(var(--mob-tab-bar-h) + var(--safe-bottom));
			background: var(--bg-elevated);
			border-top: 1px solid var(--border-subtle);
			border-radius: 20px 20px 0 0;
			z-index: 41;
			padding: 12px 0 8px;
			box-shadow: 0 -8px 32px rgba(0, 0, 0, 0.24);
			animation: sheet-up 240ms cubic-bezier(0.25, 0.8, 0.25, 1) both;
		}

		@keyframes sheet-up {
			from { transform: translateY(100%); }
			to   { transform: translateY(0); }
		}

		.mobile-more-handle {
			width: 36px;
			height: 4px;
			border-radius: 999px;
			background: var(--border-strong);
			margin: 0 auto 12px;
		}

		.mobile-more-nav {
			display: flex;
			flex-direction: column;
			gap: 2px;
			padding: 0 8px;
		}

		.mobile-more-item {
			display: flex;
			align-items: center;
			gap: 14px;
			padding: 13px 12px;
			border-radius: var(--radius-sm);
			color: var(--text-secondary);
			font-size: var(--font-size-sm);
			font-weight: var(--font-weight-medium);
			text-decoration: none;
			transition: background var(--motion-fast), color var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-more-item:active,
		.mobile-more-item.active {
			background: var(--bg-hover);
			color: var(--text-primary);
		}

		.mobile-more-item.active {
			color: var(--accent-strong);
			background: var(--accent-soft);
		}

		.mobile-more-icon {
			width: 22px;
			text-align: center;
			font-size: var(--font-size-md);
		}

		/* ── Now Playing sheet ── */
		.mobile-np-backdrop {
			position: fixed;
			inset: 0;
			background: rgba(0, 0, 0, 0.52);
			z-index: 50;
			border: none;
			padding: 0;
			cursor: default;
		}

		.mobile-np-sheet {
			position: fixed;
			left: 0;
			right: 0;
			bottom: 0;
			max-height: 92dvh;
			overflow-y: auto;
			-webkit-overflow-scrolling: touch;
			background: var(--bg-elevated);
			border-radius: var(--radius-lg) var(--radius-lg) 0 0;
			border-top: 1px solid var(--border-subtle);
			z-index: 51;
			padding: 12px 20px calc(var(--safe-bottom) + 24px);
			display: flex;
			flex-direction: column;
			gap: 16px;
			box-shadow: 0 -16px 48px rgba(0, 0, 0, 0.32);
			animation: np-sheet-up 280ms cubic-bezier(0.25, 0.8, 0.25, 1) both;
		}

		@keyframes np-sheet-up {
			from { transform: translateY(100%); }
			to   { transform: translateY(0); }
		}

		.mobile-np-handle {
			width: 36px;
			height: 4px;
			border-radius: 999px;
			background: var(--border-strong);
			margin: 0 auto;
			flex-shrink: 0;
		}

		.mobile-np-art-wrap {
			position: relative;
			width: min(260px, calc(100vw - 80px));
			aspect-ratio: 1;
			border-radius: 20px;
			overflow: hidden;
			align-self: center;
			background: var(--bg-surface);
			border: 1px solid var(--border-subtle);
			flex-shrink: 0;
		}

		.mobile-np-art {
			width: 100%;
			height: 100%;
			object-fit: cover;
			display: block;
		}

		.mobile-np-art.placeholder {
			display: grid;
			place-items: center;
			color: var(--text-tertiary);
			font-size: var(--font-size-4xl);
		}

		.mobile-np-quality {
			position: absolute;
			top: 10px;
			right: 10px;
		}

		.mobile-np-resolution {
			position: absolute;
			top: 38px;
			right: 10px;
			font-variant-numeric: tabular-nums;
			font-size: var(--font-size-xs);
			letter-spacing: 0.04em;
			opacity: 0.85;
		}

		.mobile-np-info {
			display: flex;
			align-items: center;
			justify-content: space-between;
			gap: 12px;
			min-width: 0;
		}

		.mobile-np-copy {
			display: flex;
			flex-direction: column;
			gap: 4px;
			min-width: 0;
			flex: 1;
		}

		.mobile-np-title {
			font-family: var(--font-display);
			font-size: var(--font-size-lg);
			line-height: var(--line-height-tight);
			letter-spacing: -0.01em;
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			display: block;
		}

		.mobile-np-artist {
			color: var(--text-secondary);
			font-size: var(--font-size-sm);
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			display: block;
		}

		.mobile-np-like {
			width: 42px;
			height: 42px;
			border-radius: 50%;
			display: grid;
			place-items: center;
			font-size: var(--font-size-lg);
			color: var(--text-secondary);
			flex-shrink: 0;
			border: none;
			background: none;
			cursor: pointer;
			transition: color var(--motion-fast), transform var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-np-like:active { transform: scale(0.88); }
		.mobile-np-like.active { color: #ff4d6d; }

		.mobile-np-scrub {
			display: flex;
			flex-direction: column;
			gap: 8px;
		}

		.mobile-np-scrub-track {
			position: relative;
			height: 4px;
			border-radius: 999px;
			background: var(--border-subtle);
		}

		.mobile-np-scrub-fill {
			position: absolute;
			top: 0;
			left: 0;
			height: 100%;
			background: var(--accent);
			border-radius: inherit;
			pointer-events: none;
		}

		.mobile-np-scrub-input {
			position: absolute;
			inset: -14px 0;
			width: 100%;
			opacity: 0;
			cursor: pointer;
		}

		.mobile-np-times {
			display: flex;
			justify-content: space-between;
			color: var(--text-secondary);
			font-size: var(--font-size-xs);
			font-variant-numeric: tabular-nums;
		}

		.mobile-np-transport {
			display: flex;
			align-items: center;
			justify-content: center;
			gap: 20px;
		}

		.mobile-np-btn {
			width: 48px;
			height: 48px;
			border-radius: 50%;
			display: grid;
			place-items: center;
			background: var(--bg-surface);
			border: 1px solid var(--border-subtle);
			color: var(--text-primary);
			font-size: var(--font-size-lg);
			cursor: pointer;
			transition: transform var(--motion-fast), opacity var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-np-btn:active { transform: scale(0.92); }

		.mobile-np-btn.primary {
			width: 60px;
			height: 60px;
			background: var(--accent);
			border-color: transparent;
			color: #fff;
			font-size: var(--font-size-xl);
			box-shadow: 0 8px 24px var(--accent-glow);
		}

		.mobile-np-secondary {
			display: flex;
			align-items: center;
			justify-content: center;
			gap: 10px;
			flex-wrap: wrap;
		}

		.mobile-np-chip {
			display: inline-flex;
			align-items: center;
			gap: 6px;
			padding: 8px 14px;
			border-radius: 999px;
			border: 1px solid var(--border-subtle);
			background: var(--bg-surface);
			color: var(--text-secondary);
			font-size: var(--font-size-xs);
			font-weight: var(--font-weight-semibold);
			cursor: pointer;
			transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast);
			-webkit-tap-highlight-color: transparent;
		}

		.mobile-np-chip.active {
			background: var(--accent-soft);
			border-color: var(--accent-line);
			color: var(--accent-strong);
		}

		.mobile-np-queue-header {
			display: flex;
			align-items: baseline;
			justify-content: space-between;
			padding-top: 8px;
			border-top: 1px solid var(--border-subtle);
		}

		.mobile-np-queue-count {
			color: var(--text-secondary);
			font-size: var(--font-size-sm);
		}

		.mobile-np-queue-list {
			display: flex;
			flex-direction: column;
			gap: 6px;
		}
	}

	/* ── Small phones (≤ 760px): queue touch tweaks ─────── */
	@media (max-width: 760px) {
		.queue-row { align-items: flex-start; }
		.queue-side { align-items: flex-end; }
		.queue-time { display: none; }
		/* Overflow stays tappable without a hover state on touch. */
		.queue-overflow { opacity: 1; }
	}

	/* ─── Connect screen ───────────────────── */

	.pkce-relogin-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-modal, 80);
		display: grid;
		place-items: center;
		padding: 24px;
		background: rgba(0, 0, 0, 0.56);
		backdrop-filter: blur(10px);
	}

	.pkce-relogin-modal {
		width: min(100%, 460px);
		display: flex;
		flex-direction: column;
		gap: 16px;
		padding: 28px;
		border-radius: var(--radius-md, 12px);
	}

	.pkce-relogin-modal h2 {
		margin: 0;
		font-size: var(--font-size-xl);
		letter-spacing: 0;
	}

	.pkce-relogin-modal p {
		margin: 0;
		color: var(--text-secondary);
		line-height: var(--line-height-normal);
	}

	.pkce-relogin-actions {
		display: flex;
		flex-wrap: wrap;
		gap: 10px;
	}

	.connect-backdrop {
		position: fixed;
		inset: 0;
		z-index: var(--z-tooltip);
		background: var(--bg-base, #0d0d12);
		display: flex;
		align-items: center;
		justify-content: center;
		padding: 24px;
	}

	.connect-panel {
		width: 100%;
		max-width: 400px;
		display: flex;
		flex-direction: column;
		gap: 16px;
		padding: 32px;
		border-radius: var(--radius-lg, 16px);
	}

	.connect-brand {
		display: flex;
		align-items: center;
		gap: 10px;
		margin-bottom: 4px;
	}

	.connect-brand-mark {
		width: 36px;
		height: 36px;
		display: flex;
		align-items: center;
		justify-content: center;
	}

	.connect-brand-mark img {
		width: 100%;
		height: 100%;
		object-fit: contain;
	}

	.connect-brand-name {
		font-size: var(--font-size-lg);
		font-weight: 800;
		letter-spacing: 0.12em;
		color: var(--text-primary);
	}

	.connect-title {
		font-size: var(--font-size-lg);
		font-weight: var(--font-weight-bold);
		color: var(--text-primary);
	}

	.connect-copy {
		font-size: var(--font-size-sm);
		color: var(--text-secondary);
		line-height: var(--line-height-normal);
	}

	.pin-pad {
		display: flex;
		gap: 10px;
		justify-content: center;
		margin: 8px 0 4px;
		background: none;
		border: none;
		padding: 0;
		cursor: text;
	}

	.pin-digit {
		flex: 0 0 auto;
		width: 44px;
		height: 56px;
		border-radius: var(--radius-sm, 8px);
		background: rgba(255, 255, 255, 0.04);
		border: 1px solid rgba(255, 255, 255, 0.1);
		display: flex;
		align-items: center;
		justify-content: center;
		font-family: var(--font-mono, monospace);
		font-size: var(--font-size-xl);
		font-weight: var(--font-weight-semibold);
		color: var(--text-primary);
		transition: border-color 0.15s ease, background 0.15s ease;
	}

	.pin-digit.filled {
		background: rgba(124, 128, 255, 0.12);
		border-color: rgba(124, 128, 255, 0.45);
	}

	.pin-digit.active {
		border-color: rgba(124, 128, 255, 0.8);
		box-shadow: 0 0 0 3px rgba(124, 128, 255, 0.15);
	}

	.pin-hidden-input {
		position: absolute;
		opacity: 0;
		pointer-events: none;
		width: 1px;
		height: 1px;
	}

	.connect-error {
		font-size: var(--font-size-sm);
		color: #ffb0b0;
		text-align: center;
	}

	@media (max-width: 420px) {
		.pin-digit {
			width: 40px;
			height: 52px;
			font-size: var(--font-size-xl);
		}
		.pin-pad { gap: 8px; }
	}
</style>
