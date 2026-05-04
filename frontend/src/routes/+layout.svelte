<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { goto } from '$app/navigation';
	import { connectWebSocket, wsConnected } from '$lib/api/ws';
	import {
		currentTrack,
		currentQueueItemId,
		currentStreamDisplay,
		isPlaying,
		position,
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
		clearQueue as clearQueueAction,
		restoreQueueItems,
		saveQueueAsPlaylist,
		playTidalTrackNext
	} from '$lib/stores/player';
	import { get } from 'svelte/store';
	import type { QueueItem, TidalPlayable, Track } from '$lib/api/client';
	import { showToast } from '$lib/stores/toast';
	import { formatDuration, getQualityClass } from '$lib/stores/library';
	import { api, getStoredToken, setStoredToken, clearStoredToken } from '$lib/api/client';
	import ContextMenu from '$lib/components/ContextMenu.svelte';
	import Toast from '$lib/components/Toast.svelte';
	import CommandPalette from '$lib/components/CommandPalette.svelte';
	import QueueReasonCard from '$lib/components/QueueReasonCard.svelte';
	import NowPlayingMetadata from '$lib/components/now-playing/NowPlayingMetadata.svelte';
	import NowPlayingProgress from '$lib/components/now-playing/NowPlayingProgress.svelte';
	import NowPlayingTransport from '$lib/components/now-playing/NowPlayingTransport.svelte';
	import QuietMode from '$lib/components/QuietMode.svelte';
	import { openQuietMode } from '$lib/stores/quiet_mode';
	import { commandPaletteOpen } from '$lib/stores/command_palette';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTrackMenu, buildTidalTrackMenu } from '$lib/player/track_menu';
	import { trackToTidalPlayable } from '$lib/utils/track';
	import ShaderWallpaper from '$lib/components/wallpaper/ShaderWallpaper.svelte';
	import { wallpaperById } from '$lib/components/wallpaper/shaders';
	import { wallpaper } from '$lib/stores/wallpaper';
	import { palette } from '$lib/stores/palette';
	import { paletteById } from '$lib/components/wallpaper/palettes';

	let { children } = $props();

	let activeWallpaper = $derived(wallpaperById($wallpaper));

	// ─── Auth gate ───────────────────────────────────────────────
	let authReady = $state(false);
	let onboardingChecked = $state(false);
	let isOnboardingRoute = $derived(page.url.pathname.startsWith('/onboarding'));
	let showConnect = $state(false);
	let connectTokenInput = $state('');
	let connectError = $state('');
	let connectBusy = $state(false);
	let pinInputEl = $state<HTMLInputElement | null>(null);

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
			setStoredToken(t);
			const ok = await api.ping();
			if (!ok) { clearStoredToken(); connectError = 'Could not reach the server. Check the URL / network.'; return; }
			const resp = await fetch(`${(await import('$lib/api/client')).getApiBase()}/api/status`, {
				headers: { authorization: `Bearer ${t}` }
			});
			if (resp.status === 401) {
				clearStoredToken();
				connectError = 'PIN rejected — double-check the 6 digits.';
				connectTokenInput = '';
				setTimeout(focusPin, 0);
				return;
			}
			showConnect = false;
			onConnected();
		} catch {
			clearStoredToken();
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

	// Phase 2b: queue-row "why is this here" tooltip. Tracks the
	// reason string of the currently-hovered row plus the cursor
	// position so QueueReasonCard can place itself near the trigger.
	let hoveredReason = $state<string | null>(null);
	let reasonMouseX = $state(0);
	let reasonMouseY = $state(0);

	function showQueueReason(reason: string | null | undefined, event: MouseEvent) {
		if (!reason) return;
		hoveredReason = reason;
		reasonMouseX = event.clientX;
		reasonMouseY = event.clientY;
	}

	function moveQueueReason(event: MouseEvent) {
		if (hoveredReason === null) return;
		reasonMouseX = event.clientX;
		reasonMouseY = event.clientY;
	}

	function hideQueueReason() {
		hoveredReason = null;
	}
	let mobileFavoritePending = $state(false);
	let desktopFavoritePending = $state(false);

	const navZones = [
		{
			label: 'Atlas',
			items: [
				{ path: '/', label: 'Home', icon: '⌂' },
				{ path: '/library', label: 'Library', icon: '♫' },
				{ path: '/search', label: 'Search', icon: '⌕' },
				{ path: '/genres', label: 'Genre Galaxy', icon: '✦' },
				{ path: '/playlists', label: 'Playlists', icon: '☰' },
				{ path: '/discoverspace', label: 'Discover', icon: '◈' }
			]
		},
		{
			label: 'Signals',
			items: [
				{ path: '/automix', label: 'Automix', icon: '⟁' },
				{ path: '/analytics', label: 'Analytics', icon: '◉' },
				{ path: '/duplicates', label: 'Duplicates', icon: '⊘' }
			]
		},
		{
			label: 'System',
			items: [{ path: '/settings', label: 'Settings', icon: '⚙' }]
		}
	] as const;

	const shuffleLabels: Record<string, string> = {
		off: 'Shuffle off',
		genre: 'Genre mix',
		weighted: 'Smart shuffle',
		true: 'True shuffle'
	};

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

	onMount(() => {
		// Show connect screen if no token is stored
		if (!getStoredToken()) {
			void tryAutoSetup();
		} else {
			authReady = true;
			connectWebSocket();
			void refreshPlaybackState();
			void checkOnboarding();
		}

		// Listen for 401 responses from any API call. On loopback the backend
		// will hand us a fresh token via /api/setup/token, so retry auto-setup
		// before falling back to the PIN modal — keeps local launches silent
		// even if the stored token is stale (e.g. server regenerated).
		window.addEventListener('noor:unauthorized', () => {
			clearStoredToken();
			authReady = false;
			onboardingChecked = false;
			void tryAutoSetup();
		});

		const storedTheme = localStorage.getItem('noor-theme');
		if (storedTheme === 'light' || storedTheme === 'dark') {
			theme = storedTheme;
		}

		applyTheme(theme);

		const unsubPalette = palette.subscribe((id) => applyPalette(id));

		window.addEventListener('keydown', handleGlobalKeydown);
		return () => {
			window.removeEventListener('keydown', handleGlobalKeydown);
			unsubPalette();
		};
	});

	function isTypingTarget(target: EventTarget | null): boolean {
		if (!(target instanceof HTMLElement)) return false;
		const tag = target.tagName;
		if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
		return target.isContentEditable;
	}

	function handleGlobalKeydown(event: KeyboardEvent) {
		// Cmd/Ctrl+K → open command palette
		if ((event.ctrlKey || event.metaKey) && event.key === 'k') {
			event.preventDefault();
			commandPaletteOpen.update(v => !v);
			return;
		}
		if (event.ctrlKey || event.metaKey || event.altKey) return;
		if (isTypingTarget(event.target)) return;

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
					setStoredToken(data.token);
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
		connectWebSocket();
		void refreshPlaybackState();
		void checkOnboarding();
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

	async function handleQueueTrackPlay(trackId: number) {
		await playTrackNow(trackId);
		nowPlayingOpen = false;
	}

	function handleQueueTrackKeydown(trackId: number, event: KeyboardEvent) {
		runOnActivation(event, () => void handleQueueTrackPlay(trackId));
	}

	async function handleQueueRemove(queueItemId: number, event: MouseEvent) {
		event.stopPropagation();
		await removeTrackFromQueue(queueItemId);
	}

	async function handleQueueMoveNext(queueItemId: number, event: MouseEvent) {
		event.stopPropagation();
		await moveQueueTrackNext(queueItemId);
	}

	function formatQuality(q: string | null) {
		if (!q) return '';
		if (q === 'HI_RES_LOSSLESS') return 'HiRes Lossless';
		if (q === 'LOSSLESS') return 'Lossless';
		if (q === 'HIGH') return 'High';
		if (q === 'LOW') return 'Low';
		return q.replaceAll('_', ' ');
	}

	function formatStreamDetail(stream: { sample_rate: number | null; bit_depth: number | null } | null): string {
		if (!stream) return '';
		const parts: string[] = [];
		if (stream.sample_rate) {
			const khz = stream.sample_rate / 1000;
			parts.push(Number.isInteger(khz) ? `${khz} kHz` : `${khz.toFixed(1)} kHz`);
		}
		if (stream.bit_depth) parts.push(`${stream.bit_depth}-bit`);
		return parts.join(' · ');
	}

	function formatQueueSource(source: string): string {
		const normalized = source.trim().toLowerCase();
		if (normalized.includes('automix')) return 'Automix';
		if (normalized.includes('genre')) return 'Genre';
		if (normalized.includes('discover')) return 'Discover';
		if (normalized.includes('playlist')) return 'Playlist';
		if (normalized.includes('library')) return 'Library';
		if (normalized.includes('manual') || normalized.includes('queue')) return 'Manual';
		return source || 'Queued';
	}

	// Source slug drives the `.source-*` CSS class that paints the 4px dot on
	// queue artwork. Keep in sync with formatQueueSource above.
	function queueSourceSlug(source: string): string {
		return formatQueueSource(source).toLowerCase();
	}

	type QueueItemType = (typeof $playbackQueue)[number];

	/**
	 * Pick the right context-menu builder for a `Track`. Ephemeral
	 * Tidal tracks (negative `id`, set by `play_tidal_ephemeral` on
	 * the backend) need the Tidal-aware builder so "Song radio" goes
	 * to `/api/discovery/radio` with `seed_tidal_id` rather than the
	 * library-only `/api/radio/song` which doesn't know how to look
	 * up negative ids and 500s.
	 */
	function pickMenuBuilder(track: Track, options?: { queueItemId?: number; isPending?: boolean }) {
		const tidal = trackToTidalPlayable(track);
		if (tidal) {
			if (options?.queueItemId !== undefined) {
				// Ephemeral Tidal tracks should never end up in the
				// persisted queue table — `play_tidal_ephemeral`
				// overlays them via AppState rather than writing
				// rows. Surface the assumption violation rather than
				// silently strip queue-row-specific menu options.
				console.warn(
					`pickMenuBuilder: ephemeral Tidal track in persisted queue (queueItemId=${options.queueItemId}, tidal_id=${track.tidal_id})`
				);
			}
			return buildTidalTrackMenu(tidal);
		}
		return buildTrackMenu(track, options);
	}

	function queueItemTidalPlayable(item: QueueItemType): TidalPlayable | null {
		return trackToTidalPlayable(item.track);
	}

	function isEphemeralQueueItem(item: QueueItemType): boolean {
		return item.id < 0 && queueItemTidalPlayable(item) != null;
	}

	async function handleQueuePlayNext(item: QueueItemType, event: MouseEvent) {
		event.stopPropagation();
		const tidal = queueItemTidalPlayable(item);
		if (tidal && item.id < 0) {
			await playTidalTrackNext(tidal);
			return;
		}
		await handleQueueMoveNext(item.id, event);
	}

	function openQueueRowMenu(item: QueueItemType, event: MouseEvent) {
		event.preventDefault();
		event.stopPropagation();
		const tidal = queueItemTidalPlayable(item);
		const items = tidal
			? buildTidalTrackMenu(tidal)
			: pickMenuBuilder(item.track, { queueItemId: item.id, isPending: item.is_pending });
		openContextMenu(event, items, item.track.title);
	}

	function openQueueRowMenuFromButton(item: QueueItemType, event: MouseEvent) {
		event.stopPropagation();
		const tidal = queueItemTidalPlayable(item);
		const items = tidal
			? buildTidalTrackMenu(tidal)
			: pickMenuBuilder(item.track, { queueItemId: item.id, isPending: item.is_pending });
		openMenuAtElement(event.currentTarget as HTMLElement, items, item.track.title);
	}

	async function handleQueueRowFavorite(trackId: number, event: MouseEvent) {
		event.stopPropagation();
		try {
			await toggleTrackFavorite(trackId);
		} catch {
			// toggleTrackFavorite surfaces its own error in the player store.
		}
	}

	// ─── Queue drag-to-reorder ────────────────────────────────────────────────
	let dragItemId = $state<number | null>(null);
	let dragOverItemId = $state<number | null>(null);

	function handleQueueDragStart(event: DragEvent, item: QueueItemType) {
		dragItemId = item.id;
		if (event.dataTransfer) {
			event.dataTransfer.effectAllowed = 'move';
			// Required for Firefox to actually start a drag.
			event.dataTransfer.setData('text/plain', String(item.id));
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
		await moveQueueItem(sourceId, targetIndex);
	}

	function handleQueueDragEnd() {
		dragItemId = null;
		dragOverItemId = null;
	}

	// ─── Scroll active queue row into view ───────────────────────────────────
	let lastUserScrollAt = $state(0);
	let queueListEl: HTMLElement | null = $state(null);

	function handleQueueScroll() {
		lastUserScrollAt = Date.now();
	}

	$effect(() => {
		const id = $currentTrack?.id;
		if (!id || !queueListEl) return;
		// Bail if the user scrolled recently — don't yank focus from their browse.
		if (Date.now() - lastUserScrollAt < 5000) return;
		const row = queueListEl.querySelector(`[data-track-id="${id}"]`);
		if (!row) return;
		const rect = row.getBoundingClientRect();
		const containerRect = queueListEl.getBoundingClientRect();
		const offscreen = rect.bottom < containerRect.top || rect.top > containerRect.bottom;
		if (offscreen) {
			row.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
		}
	});

	// ─── Source attribution for now-playing card ─────────────────────────────
	function attributionFor(track: { id: number } | null): string | null {
		if (!track) return null;
		const item = $playbackQueue.find((q) => q.track.id === track.id);
		if (!item) return null;
		const friendly = formatQueueSource(item.source);
		// "Manual" / generic queue isn't worth surfacing.
		if (friendly === 'Manual' || friendly === 'Queued') return null;
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
		const restorable = await clearQueueAction();
		if (restorable.length > 0) {
			// Replace the auto-toast with a richer one that has a real undo button.
			// We can't bind a click handler to the simple text toast, so expose
			// undo via the keyboard shortcut Z within 6s.
			const onKey = (event: KeyboardEvent) => {
				if (isTypingTarget(event.target)) return;
				if (event.key === 'z' || event.key === 'Z') {
					event.preventDefault();
					window.removeEventListener('keydown', onKey);
					void restoreQueueItems(restorable);
					showToast(`Restored ${restorable.length} tracks`, 'success');
				}
			};
			window.addEventListener('keydown', onKey);
			setTimeout(() => window.removeEventListener('keydown', onKey), 6000);
		}
	}

	function stopPropagation(event: Event) {
		event.stopPropagation();
	}

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
		const currentId = $currentTrack?.id;
		if (!currentId) return $playbackQueue;
		const currentPosition = $playbackQueue.find((item) => item.track.id === currentId)?.position ?? -1;
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
	let playerState = $derived(
		$currentTrack ? ($isPlaying ? 'Playing' : 'Paused') : $playerReady ? 'Ready' : 'Connecting'
	);
	let mobilePlayerVisible = $derived(Boolean($currentTrack));
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
					<img src="/mark-animated-dark.svg" alt="" aria-hidden="true" />
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

<div class="wallpaper-layer" aria-hidden="true">
	{#if activeWallpaper.shader}
		<ShaderWallpaper shader={activeWallpaper.shader} interactive={false} maxDpr={1.5} />
	{/if}
</div>

<ContextMenu />
<Toast />
<CommandPalette />
<QuietMode />
<QueueReasonCard reason={hoveredReason} mouseX={reasonMouseX} mouseY={reasonMouseY} />

{#if isOnboardingRoute}
	{@render children()}
{:else if authReady && !onboardingChecked}
	<div class="onboarding-check">
		<img class="check-mark" src="/mark-animated-dark.svg" alt="" aria-hidden="true" />
		<p>Checking setup…</p>
	</div>
{:else}
<div class="app-shell" class:mobile-player-active={mobilePlayerVisible} class:has-wallpaper={$wallpaper !== 'none'}>
	<header class="mobile-top-bar">
		<a href="/" class="mobile-brand" aria-label="NOOR home">
			<span class="mobile-brand-mark">
				<img src="/mark-animated-dark.svg" alt="" aria-hidden="true" />
			</span>
			<span class="mobile-brand-name">NOOR</span>
		</a>
		<button class="mobile-theme-btn btn btn-glass" onclick={toggleTheme}>
			{theme === 'dark' ? '☀' : '◑'}
		</button>
	</header>

	<aside class="sidebar">
		<a href="/" class="brand" aria-label="NOORwave home">
			<img class="brand-splash" src="/mark-animated-dark.svg" alt="NOORwave" />
		</a>

		<nav class="nav" aria-label="Primary">
			{#each navZones as zone}
				<div class="nav-zone">
					<p class="nav-zone-label">{zone.label}</p>
					{#each zone.items as item}
						<a
							href={item.path}
							class="nav-item"
							class:special={item.path === '/genres'}
							class:active={isNavItemActive(item.path)}
							aria-current={isNavItemActive(item.path) ? 'page' : undefined}
						>
							<span class="nav-icon">{item.icon}</span>
							<span class="nav-label">{item.label}</span>
						</a>
					{/each}
				</div>
			{/each}
		</nav>

		<div class="sidebar-footer">
			<div class="live-status">
				<span class:offline={!$wsConnected} class="live-dot"></span>
				<div class="live-copy">
					<strong>{$wsConnected ? 'Observatory live' : 'Signal offline'}</strong>
					<span>{$wsConnected ? 'Realtime stream is locked in' : 'Waiting for websocket relay'}</span>
				</div>
			</div>

			{#if $shuffleMode !== 'off'}
				<p class="status-line">{shuffleLabels[$shuffleMode]}</p>
			{/if}

			{#if $automixEnabled}
				<p class="status-line">Automix extending the session</p>
			{/if}

			<button class="theme-toggle btn btn-glass" onclick={toggleTheme}>
				{theme === 'dark' ? 'Switch to light' : 'Switch to dark'}
			</button>
		</div>
	</aside>

	<main class="workspace">
		{@render children()}
	</main>

	<aside
		class="now-playing-panel"
		class:queue-expanded={queueExpanded}
		oncontextmenu={openNowPlayingContextMenu}
	>
		<div class="np-top">
			<div class="np-artwork-wrap">
				{#key $currentTrack?.artwork_url}
					{#if $currentTrack?.artwork_url}
						<img class="np-artwork" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="np-artwork placeholder">♫</div>
					{/if}
				{/key}

				{#if $currentStreamDisplay}
					<span class={`quality-badge np-quality ${getQualityClass($currentStreamDisplay.audio_quality)}`}>
						{formatQuality($currentStreamDisplay.audio_quality)}
					</span>
				{:else if $currentTrack?.best_quality}
					<span class={`quality-badge np-quality ${getQualityClass($currentTrack.best_quality)}`}>
						{formatQuality($currentTrack.best_quality)}
					</span>
				{/if}

				{#if $currentTrack}
					<button
						class="np-fullscreen-btn"
						aria-label="Enter quiet mode"
						title="Quiet mode"
						onclick={openQuietMode}
					>⛶</button>
				{/if}
			</div>

			<NowPlayingMetadata
				track={$currentTrack}
				nowPlayingAttribution={nowPlayingAttribution}
				streamDetail={formatStreamDetail($currentStreamDisplay)}
				playerState={playerState}
				isScrubbing={isScrubbing}
			/>

			<NowPlayingProgress
				position={$position}
				duration={$currentTrack?.duration_ms ?? 0}
				onSeek={(p) => void setPlayerPosition(p)}
				onScrubStart={() => { isScrubbing = true; }}
				onScrubEnd={() => { isScrubbing = false; }}
			/>

			<NowPlayingTransport
				track={$currentTrack}
				isPlaying={$isPlaying}
				shuffleMode={$shuffleMode}
				repeatMode={$repeatMode}
				favoritePending={desktopFavoritePending}
				onToggleFavorite={() => void handleDesktopFavoriteToggle()}
				onCycleShuffle={() => void cyclePlayerShuffleMode()}
				onPrev={() => void playPreviousTrack()}
				onPlayPause={() => void togglePlayback()}
				onNext={() => void playNextTrack()}
				onCycleRepeat={() => void cyclePlayerRepeatMode()}
				onOpenMore={(anchor) => openNowPlayingMenuAt(anchor)}
			/>

			<div class="np-controls">
				<button
					class="np-mute-btn"
					type="button"
					title={$volume === 0 ? 'Unmute' : 'Mute'}
					aria-label={$volume === 0 ? 'Unmute' : 'Mute'}
					aria-pressed={$volume === 0}
					onclick={() => void toggleMute()}
				>{$volume === 0 ? '🔇' : '🔊'}</button>
				<label class="volume-control">
					<span>Vol</span>
					<input
						type="range"
						min="0"
						max="1"
						step="0.01"
						value={$volume}
						oninput={(event) => {
							displayVolume = Math.round(Number((event.currentTarget as HTMLInputElement).value) * 100);
						}}
						onchange={(event) =>
							void setPlayerVolume(Number((event.currentTarget as HTMLInputElement).value))}
						aria-label="Volume"
					/>
					<span class="volume-pct">{displayVolume}%</span>
				</label>

			</div>

			{#if $playerError}
				<div class="player-error" role="alert">
					<span class="player-error-msg">{$playerError.message}</span>
					{#if $playerError.retry}
						<button
							class="player-error-btn"
							onclick={async () => {
								const retry = $playerError?.retry;
								playerError.set(null);
								if (retry) await retry();
							}}
						>Retry</button>
					{/if}
					<button
						class="player-error-close"
						aria-label="Dismiss"
						onclick={() => playerError.set(null)}
					>×</button>
				</div>
			{/if}
		</div>

		<section class="queue-section">
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
						<span class="queue-count-unit">
							{upcomingQueue.length === 1 ? 'track' : 'tracks'} · {queueTotalLabel}
						</span>
					{:else}
						<span class="queue-eyebrow">Up next</span>
						<span class="queue-count-unit">empty</span>
					{/if}
				</button>
				<div class="queue-header-actions">
					<button
						class="queue-icon-btn queue-automix-btn"
						class:active={$automixEnabled}
						title={$automixEnabled ? 'Automix on' : 'Automix off'}
						aria-label={$automixEnabled ? 'Disable automix' : 'Enable automix'}
						onclick={() => void togglePlayerAutomix()}
					>🎧</button>
					<button
						class="queue-icon-btn queue-discover-btn"
						class:active={$automixDiscoverNew}
						title={$automixDiscoverNew ? 'Include New: on — pulling in tracks outside your library' : 'Include New: off — tap to find new music during automix'}
						aria-label={$automixDiscoverNew ? 'Disable discover new' : 'Enable discover new'}
						aria-pressed={$automixDiscoverNew}
						onclick={() => void setPlayerDiscoverNew(!$automixDiscoverNew)}
					>
						<svg width="12" height="12" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
							<path d="M7.5 1a6.5 6.5 0 1 0 0 13A6.5 6.5 0 0 0 7.5 1zm0 1a5.5 5.5 0 1 1 0 11A5.5 5.5 0 0 1 7.5 2zM7 4.5V7H4.5a.5.5 0 0 0 0 1H7v2.5a.5.5 0 0 0 1 0V8h2.5a.5.5 0 0 0 0-1H8V4.5a.5.5 0 0 0-1 0z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd"/>
						</svg>
					</button>
					<button
						class="queue-icon-btn queue-save-btn"
						type="button"
						title="Save queue as playlist"
						aria-label="Save queue as playlist"
						onclick={openSaveQueue}
						disabled={upcomingQueue.length === 0 && !$currentTrack}
					>＋</button>
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
				<div class="queue-list" id="queue-list" bind:this={queueListEl} onscroll={handleQueueScroll}>
					{#each upcomingQueue.slice(0, 40) as item, i (`${item.id}-${i}`)}
						{@const aid = item.track.artist_id}
						{@const isPending = item.is_pending === true}
						<div
							class:active={$currentQueueItemId != null
								? $currentQueueItemId === item.id
								: $currentTrack?.id === item.track.id}
							class:dragging={dragItemId === item.id}
							class:drag-over={dragOverItemId === item.id && dragItemId !== item.id}
							class:pending={isPending}
							class="queue-row"
							role="button"
							tabindex={isPending ? undefined : 0}
							aria-disabled={isPending}
							title={isPending ? 'Resolving on TIDAL...' : undefined}
							draggable={true}
							data-track-id={item.track.id}
							onclick={isPending ? undefined : () => void handleQueueTrackPlay(item.track.id)}
							onkeydown={isPending ? undefined : (event) => handleQueueTrackKeydown(item.track.id, event)}
							oncontextmenu={(event) => openQueueRowMenu(item, event)}
							ondragstart={(event) => handleQueueDragStart(event, item)}
							ondragover={(event) => handleQueueDragOver(event, item)}
							ondragleave={() => handleQueueDragLeave(item)}
							ondrop={(event) => void handleQueueDrop(event, item)}
							ondragend={handleQueueDragEnd}
						>
							<span class="queue-grip" aria-hidden="true" title="Drag to reorder">⋮⋮</span>
							<div class="queue-art-wrap" title={formatQueueSource(item.source)}>
								{#if isPending}
									<div class="queue-art placeholder pending-art" title="Resolving track...">
										<span class="queue-spinner" aria-hidden="true"></span>
									</div>
								{:else if item.track.artwork_url}
									<img class="queue-art" src={item.track.artwork_url} alt="" />
								{:else}
									<div class="queue-art placeholder">♫</div>
								{/if}
								<span class="queue-source-dot source-{queueSourceSlug(item.source)}" aria-hidden="true"></span>
							</div>

							<div class="queue-meta">
								<p class="queue-title">{item.track.title}</p>
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
									>{item.track.artist_name ?? 'Unknown artist'}</a>
								{:else}
									<span class="queue-artist">{item.track.artist_name ?? 'Unknown artist'}</span>
								{/if}
							</div>

							<div class="queue-side">
								<span class="queue-time">{formatDuration(item.track.duration_ms)}</span>
								<div class="queue-actions">
									{#if item.reason}
										<button
											class="queue-action icon reason"
											aria-label="Why is this here?"
											title="Why is this here?"
											onmouseenter={(event) => showQueueReason(item.reason, event)}
											onmousemove={moveQueueReason}
											onmouseleave={hideQueueReason}
											onfocus={(event) => showQueueReason(item.reason, event as unknown as MouseEvent)}
											onblur={hideQueueReason}
											onclick={stopPropagation}
										>ⓘ</button>
									{/if}
									<button
										class="queue-action icon"
										class:active={item.track.is_favorite}
										aria-label={item.track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
										title={item.track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
										disabled={isPending}
										onclick={(event) => void handleQueueRowFavorite(item.track.id, event)}
									>{item.track.is_favorite ? '♥' : '♡'}</button>
									<button
										class="queue-action icon"
										aria-label="More actions"
										title="More actions"
										onclick={(event) => openQueueRowMenuFromButton(item, event)}
									>⋯</button>
									<button
										class="queue-action icon"
										aria-label={isEphemeralQueueItem(item) ? 'Promote to play next' : 'Play next'}
										title={isEphemeralQueueItem(item) ? 'Add this TIDAL mix track as next in the queue' : 'Play next'}
										onclick={(event) => void handleQueuePlayNext(item, event)}
									>↑</button>
									<button
										class="queue-action icon remove"
										aria-label="Remove from queue"
										title={isEphemeralQueueItem(item) ? 'TIDAL mix rows cannot be removed yet' : 'Remove from queue'}
										disabled={isEphemeralQueueItem(item)}
										onclick={(event) => void handleQueueRemove(item.id, event)}
									>×</button>
								</div>
							</div>
						</div>
					{/each}
				</div>
			{:else}
				<div class="queue-empty">
					<p>Nothing is lined up yet.</p>
					<span>Start from the library, genres, playlists, or discovery.</span>
				</div>
			{/if}

			{#if upcomingQueue.length > 40}
				<p class="queue-overflow">+ {upcomingQueue.length - 40} more tracks waiting in the queue</p>
			{/if}
		</section>
	</aside>

	<!-- Mini player bar (mobile only) -->
	{#if $currentTrack}
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
				>
					{#if $currentTrack.artwork_url}
						<img class="mobile-mini-art" src={$currentTrack.artwork_url} alt="" />
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
		<a
			href="/"
			class="mobile-tab"
			class:active={isNavItemActive('/')}
			aria-current={isNavItemActive('/') ? 'page' : undefined}
		>
			<span class="mobile-tab-icon">⌂</span>
			<span class="mobile-tab-label">Home</span>
		</a>
		<a
			href="/library"
			class="mobile-tab"
			class:active={isNavItemActive('/library')}
			aria-current={isNavItemActive('/library') ? 'page' : undefined}
		>
			<span class="mobile-tab-icon">♫</span>
			<span class="mobile-tab-label">Library</span>
		</a>
		<a
			href="/genres"
			class="mobile-tab"
			class:active={isNavItemActive('/genres')}
			aria-current={isNavItemActive('/genres') ? 'page' : undefined}
		>
			<span class="mobile-tab-icon">✦</span>
			<span class="mobile-tab-label">Genres</span>
		</a>
		<a
			href="/discoverspace"
			class="mobile-tab"
			class:active={isNavItemActive('/discoverspace')}
			aria-current={isNavItemActive('/discoverspace') ? 'page' : undefined}
		>
			<span class="mobile-tab-icon">◈</span>
			<span class="mobile-tab-label">Discover</span>
		</a>
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
				<a href="/playlists" class="mobile-more-item" class:active={isNavItemActive('/playlists')} onclick={() => { moreOpen = false; }}>
					<span class="mobile-more-icon">☰</span>
					<span>Playlists</span>
				</a>
				<a href="/automix" class="mobile-more-item" class:active={isNavItemActive('/automix')} onclick={() => { moreOpen = false; }}>
					<span class="mobile-more-icon">⟁</span>
					<span>Automix</span>
				</a>
				<a href="/analytics" class="mobile-more-item" class:active={isNavItemActive('/analytics')} onclick={() => { moreOpen = false; }}>
					<span class="mobile-more-icon">◉</span>
					<span>Analytics</span>
				</a>
				<a href="/duplicates" class="mobile-more-item" class:active={isNavItemActive('/duplicates')} onclick={() => { moreOpen = false; }}>
					<span class="mobile-more-icon">⊘</span>
					<span>Duplicates</span>
				</a>
				<a href="/settings" class="mobile-more-item" class:active={isNavItemActive('/settings')} onclick={() => { moreOpen = false; }}>
					<span class="mobile-more-icon">⚙</span>
					<span>Settings</span>
				</a>
			</nav>
		</div>
	{/if}

	<!-- Now Playing sheet (mobile only) -->
	{#if nowPlayingOpen && $currentTrack}
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
					{#if $currentTrack.artwork_url}
						<img class="mobile-np-art" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="mobile-np-art placeholder">♫</div>
					{/if}
				{/key}
				{#if $currentStreamDisplay}
					<span class={`quality-badge mobile-np-quality ${getQualityClass($currentStreamDisplay.audio_quality)}`}>
						{formatQuality($currentStreamDisplay.audio_quality)}
					</span>
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
					<span>{formatDuration(scrubPosition)}</span>
					<span>{formatDuration($currentTrack.duration_ms ?? 0)}</span>
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
					<span>🎧</span>
					<span>{$automixEnabled ? 'Automix on' : 'Automix'}</span>
				</button>
			</div>

			<div class="mobile-np-queue-header">
				<span class="eyebrow">Up next</span>
				<span class="mobile-np-queue-count">{queueCountLabel}</span>
			</div>

			{#if upcomingQueue.length > 0}
				<div class="mobile-np-queue-list">
					{#each upcomingQueue.slice(0, 40) as item, i (`${item.id}-${i}`)}
						{@const aid = item.track.artist_id}
						{@const isPending = item.is_pending === true}
						<div
							class="queue-row"
							class:active={$currentQueueItemId != null
								? $currentQueueItemId === item.id
								: $currentTrack?.id === item.track.id}
							class:pending={isPending}
							role="button"
							tabindex={isPending ? undefined : 0}
							aria-disabled={isPending}
							title={isPending ? 'Resolving on TIDAL...' : undefined}
							onclick={isPending ? undefined : () => void handleQueueTrackPlay(item.track.id)}
							onkeydown={isPending ? undefined : (event) => handleQueueTrackKeydown(item.track.id, event)}
							oncontextmenu={(event) => openQueueRowMenu(item, event)}
						>
							<div class="queue-art-wrap" title={formatQueueSource(item.source)}>
								{#if isPending}
									<div class="queue-art placeholder pending-art" title="Resolving track...">
										<span class="queue-spinner" aria-hidden="true"></span>
									</div>
								{:else if item.track.artwork_url}
									<img class="queue-art" src={item.track.artwork_url} alt="" />
								{:else}
									<div class="queue-art placeholder">♫</div>
								{/if}
								<span class="queue-source-dot source-{queueSourceSlug(item.source)}" aria-hidden="true"></span>
							</div>
							<div class="queue-meta">
								<p class="queue-title">{item.track.title}</p>
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
									>{item.track.artist_name ?? 'Unknown artist'}</a>
								{:else}
									<span class="queue-artist">{item.track.artist_name ?? 'Unknown artist'}</span>
								{/if}
							</div>
							<div class="queue-side">
								<span class="queue-time">{formatDuration(item.track.duration_ms)}</span>
								<div class="queue-actions">
									<button
										class="queue-action icon"
										aria-label="More actions"
										onclick={(e) => openQueueRowMenuFromButton(item, e)}
									>⋯</button>
									<button
										class="queue-action icon"
										aria-label={isEphemeralQueueItem(item) ? 'Promote to play next' : 'Play next'}
										title={isEphemeralQueueItem(item) ? 'Add this TIDAL mix track as next in the queue' : 'Play next'}
										onclick={(e) => void handleQueuePlayNext(item, e)}
									>↑</button>
									<button
										class="queue-action icon remove"
										aria-label="Remove from queue"
										title={isEphemeralQueueItem(item) ? 'TIDAL mix rows cannot be removed yet' : 'Remove from queue'}
										disabled={isEphemeralQueueItem(item)}
										onclick={(e) => void handleQueueRemove(item.id, e)}
									>×</button>
								</div>
							</div>
						</div>
					{/each}
				</div>
			{:else}
				<div class="queue-empty">
					<p>Nothing is lined up yet.</p>
					<span>Start from the library, genres, playlists, or discovery.</span>
				</div>
			{/if}

			{#if upcomingQueue.length > 40}
				<p class="queue-overflow">+ {upcomingQueue.length - 40} more tracks in the queue</p>
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
		font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif;
		z-index: 50;
	}
	.onboarding-check .check-mark {
		width: 144px;
		height: 72px;
		object-fit: contain;
		opacity: 0.85;
		animation: noor-check-pulse 1.8s ease-in-out infinite;
	}
	.onboarding-check p {
		margin: 0;
		font-size: 13px;
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
		backdrop-filter: blur(18px) saturate(1.2);
		-webkit-backdrop-filter: blur(18px) saturate(1.2);
	}

	/* Content pane frost: subtle dark tint + mild blur so all page content
	   remains readable over any wallpaper, without fully hiding the animation. */
	.app-shell.has-wallpaper .workspace {
		background: rgba(9, 9, 14, 0.44);
		backdrop-filter: blur(10px) saturate(1.05);
		-webkit-backdrop-filter: blur(10px) saturate(1.05);
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

	/* Animated NOORwave OO mark — replaces the small icon + "NOOR /
	   Music command center" stack. The SVG is 320x160 (2:1) so it sits
	   cleanly in the sidebar header without pushing nav items down. */
	.brand-splash {
		display: block;
		width: 100%;
		max-width: 100px;
		height: auto;
	}

	.nav {
		display: flex;
		flex-direction: column;
		gap: 14px;
		padding-top: 4px;
	}

	.nav-zone {
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.nav-zone-label {
		padding: 0 10px 5px;
		color: var(--signal-text);
		font-size: 0.66rem;
		letter-spacing: 0.14em;
		text-transform: uppercase;
		font-weight: 700;
	}

	.nav-item {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 9px 10px;
		border-radius: var(--radius-sm);
		color: var(--text-secondary);
		position: relative;
		overflow: hidden;
		border: 1px solid transparent;
		transition:
			background var(--motion-fast),
			color var(--motion-fast),
			border-color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.nav-item:hover {
		background: color-mix(in srgb, var(--instrument-surface) 75%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 48%, transparent);
		color: var(--text-primary);
	}

	.nav-item.active {
		background: color-mix(in srgb, var(--accent-soft) 76%, var(--instrument-surface));
		border-color: color-mix(in srgb, var(--accent-line) 72%, transparent);
		color: var(--text-primary);
		box-shadow:
			0 0 0 1px color-mix(in srgb, var(--accent-line) 42%, transparent),
			0 0 24px color-mix(in srgb, var(--accent-glow) 55%, transparent);
	}

	.nav-item.special.active {
		box-shadow:
			0 0 0 1px color-mix(in srgb, var(--accent-line) 52%, transparent),
			0 0 28px color-mix(in srgb, var(--accent-glow) 78%, transparent);
	}

	.nav-item.special.active .nav-icon {
		animation: galaxy-pulse 2.6s ease-in-out infinite;
	}

	.nav-item.active::before {
		content: '';
		position: absolute;
		left: 0;
		top: 7px;
		bottom: 7px;
		width: 2px;
		border-radius: 0 2px 2px 0;
		background: var(--accent);
		box-shadow: 0 0 14px color-mix(in srgb, var(--accent-glow) 88%, transparent);
	}

	.nav-icon {
		width: 18px;
		text-align: center;
		color: var(--text-tertiary);
	}

	.nav-item.active .nav-icon {
		color: var(--accent-strong);
	}

	.nav-label {
		white-space: nowrap;
		letter-spacing: 0.01em;
	}

	@keyframes galaxy-pulse {
		0%, 100% { transform: scale(1); opacity: 0.92; }
		50% { transform: scale(1.1); opacity: 1; }
	}

	.sidebar-footer {
		margin-top: auto;
		padding: 18px 6px 0;
		display: flex;
		flex-direction: column;
		gap: 12px;
	}

	.live-status {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 12px;
		border-radius: var(--radius);
		background: color-mix(in srgb, var(--instrument-surface) 80%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 52%, transparent);
	}

	.live-dot {
		width: 9px;
		height: 9px;
		border-radius: 50%;
		background: var(--state-success);
		box-shadow: 0 0 0 6px color-mix(in srgb, var(--state-success) 18%, transparent);
		flex-shrink: 0;
	}

	.live-dot.offline {
		background: var(--text-muted);
		box-shadow: none;
	}

	.live-copy {
		display: flex;
		flex-direction: column;
		gap: 2px;
		min-width: 0;
	}

	.live-copy strong {
		font-size: 0.82rem;
	}

	.live-copy span,
	.status-line {
		color: var(--signal-text);
		font-size: 0.77rem;
	}

	.status-line {
		padding: 0 2px;
	}

	.theme-toggle {
		width: 100%;
	}

	.np-top {
		padding: 16px 16px 0;
		display: flex;
		flex-direction: column;
		gap: 16px;
		flex-shrink: 0;
	}

	.np-artwork-wrap {
		position: relative;
		aspect-ratio: 1;
		border-radius: 22px;
		overflow: hidden;
		background:
			linear-gradient(135deg, var(--bg-hover), transparent),
			var(--bg-surface);
		border: 1px solid var(--border-subtle);
		flex-shrink: 0;
	}

	.np-fullscreen-btn {
		position: absolute;
		top: 10px;
		left: 10px;
		width: 30px;
		height: 30px;
		border-radius: 8px;
		display: grid;
		place-items: center;
		font-size: 14px;
		color: #fff;
		background: rgba(0, 0, 0, 0.45);
		border: 1px solid rgba(255, 255, 255, 0.18);
		backdrop-filter: blur(8px);
		opacity: 0;
		transform: translateY(-4px);
		transition: opacity 160ms ease, transform 160ms ease, background 160ms ease;
		cursor: pointer;
	}

	.np-artwork-wrap:hover .np-fullscreen-btn,
	.np-fullscreen-btn:focus-visible {
		opacity: 1;
		transform: translateY(0);
	}

	.np-fullscreen-btn:hover {
		background: rgba(0, 0, 0, 0.65);
	}

	.np-artwork {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
		animation: artwork-fade-in 320ms ease both;
	}

	@keyframes artwork-fade-in {
		from { opacity: 0; transform: scale(1.04); }
		to   { opacity: 1; transform: scale(1); }
	}

	.np-artwork.placeholder,
	.queue-art.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	.np-artwork.placeholder {
		font-size: 2rem;
	}

	.np-quality {
		position: absolute;
		top: 10px;
		right: 10px;
	}

	.np-info,
	.np-copy {
		display: flex;
		flex-direction: column;
		gap: 6px;
		min-width: 0;
	}

	.np-eyebrow {
		color: var(--signal-text);
		font-size: 0.66rem;
		letter-spacing: 0.13em;
		text-transform: uppercase;
		font-weight: 700;
	}

	.np-title,
	.np-artist,
	.np-album {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		display: block;
		max-width: 100%;
	}

	.np-title {
		font-size: 1.35rem;
		font-family: var(--font-display);
		line-height: 1.1;
		letter-spacing: -0.02em;
	}

	/* Marquee on hover when the title overflows. text-overflow:clip drops the
	   ellipsis on hover, then a slide animation scrolls the full title back
	   and forth. Pure CSS — relies on the parent clipping the overflow. */
	.np-title:hover {
		text-overflow: clip;
		animation: np-title-marquee 9s ease-in-out infinite;
	}

	@keyframes np-title-marquee {
		0%, 15% { transform: translateX(0); }
		50%, 60% { transform: translateX(calc(-1 * (100% - 220px))); }
		95%, 100% { transform: translateX(0); }
	}

	.np-artist {
		color: var(--text-primary);
		font-size: 0.9rem;
	}

	.np-album {
		color: var(--text-secondary);
		font-size: 0.8rem;
	}

	.np-stream-detail {
		font-size: 0.7rem;
		color: var(--text-secondary);
		opacity: 0.6;
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.02em;
		margin-top: 0.1rem;
	}

	.np-source {
		font-size: 0.72rem;
		color: var(--text-secondary);
		opacity: 0.75;
		letter-spacing: 0.02em;
		margin-top: 0.1rem;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.np-progress {
		display: flex;
		flex-direction: column;
		gap: 8px;
	}

	.np-progress-track {
		position: relative;
		height: 3px;
		border-radius: 99px;
		background: color-mix(in srgb, var(--instrument-border) 35%, transparent);
		overflow: visible;
	}

	.np-progress-fill {
		height: 100%;
		background: var(--accent);
		border-radius: inherit;
		pointer-events: none;
		box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 70%, transparent);
	}

	.np-progress-track::after {
		content: '';
		position: absolute;
		left: var(--pct, 0%);
		top: 50%;
		transform: translate(-50%, -50%);
		width: 12px;
		height: 12px;
		border-radius: 50%;
		background: var(--accent);
		box-shadow: 0 0 0 3px var(--accent-glow);
		opacity: 0;
		transition: opacity var(--motion-fast), transform var(--motion-fast);
		pointer-events: none;
		z-index: 1;
	}

	.np-progress-track:hover::after,
	.np-progress-track:focus-within::after {
		opacity: 1;
	}

	.np-progress-track:active::after {
		transform: translate(-50%, -50%) scale(1.25);
	}

	.np-progress-input {
		position: absolute;
		inset: -8px 0;
		width: 100%;
		opacity: 0;
		cursor: pointer;
	}

	.np-times {
		display: flex;
		justify-content: space-between;
		color: var(--text-secondary);
		font-size: 0.74rem;
		font-variant-numeric: tabular-nums;
	}

	.tp-mode-btn {
		position: relative;
	}



	.transport {
		display: flex;
		align-items: center;
		justify-content: center;
		gap: 10px;
	}

	.tp-btn,
	.tp-play {
		width: 36px;
		height: 36px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		color: var(--text-primary);
		transition:
			transform var(--motion-fast),
			background var(--motion-fast),
			border-color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.tp-btn:hover,
	.tp-play:hover,
	.queue-action:hover {
		transform: translateY(-1px);
	}

	.tp-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
		box-shadow: 0 0 14px color-mix(in srgb, var(--accent-glow) 70%, transparent);
	}

	.tp-like-btn {
		font-size: 18px;
		color: var(--text-secondary);
		transition:
			transform var(--motion-fast),
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.tp-like-btn:active {
		transform: scale(0.92);
	}

	.tp-like-btn:disabled {
		opacity: 0.5;
		cursor: not-allowed;
	}

	.tp-like-btn.active {
		color: #ff4d6d;
		background: color-mix(in srgb, #ff4d6d 15%, transparent);
		border-color: color-mix(in srgb, #ff4d6d 40%, transparent);
		box-shadow: 0 0 12px color-mix(in srgb, #ff4d6d 30%, transparent);
	}

	.tp-play {
		background: var(--accent);
		color: #fff;
		width: 42px;
		height: 42px;
		box-shadow: 0 10px 26px var(--accent-glow);
	}

	.np-controls {
		display: flex;
		align-items: center;
		gap: 12px;
	}

	.np-mute-btn {
		width: 32px;
		height: 32px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		color: var(--text-primary);
		font-size: 14px;
		flex-shrink: 0;
		cursor: pointer;
		transition: background var(--motion-fast), border-color var(--motion-fast);
	}

	.np-mute-btn:hover {
		background: color-mix(in srgb, var(--instrument-surface-strong) 92%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 82%, transparent);
	}

	.np-mute-btn[aria-pressed='true'] {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
	}

	.volume-control {
		flex: 1;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px 12px;
		border-radius: 999px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
	}

	.volume-control span {
		color: var(--text-secondary);
		font-size: 0.76rem;
	}

	.volume-pct {
		color: var(--text-tertiary);
		font-size: 0.72rem;
		font-variant-numeric: tabular-nums;
		min-width: 3ch;
		text-align: right;
		flex-shrink: 0;
	}

	.volume-control input {
		flex: 1;
		min-width: 0;
	}


	.player-error {
		display: flex;
		align-items: center;
		gap: 8px;
		padding: 10px 12px;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-error) 12%, transparent);
		border: 1px solid color-mix(in srgb, var(--state-error) 24%, transparent);
		color: var(--state-error);
		font-size: 0.78rem;
		margin-top: 8px;
	}

	.player-error-msg {
		flex: 1;
		min-width: 0;
	}

	.player-error-btn,
	.player-error-close {
		background: transparent;
		border: 1px solid color-mix(in srgb, var(--state-error) 40%, transparent);
		color: inherit;
		border-radius: 4px;
		cursor: pointer;
		font: inherit;
		padding: 2px 8px;
		flex-shrink: 0;
	}
	.player-error-btn:hover,
	.player-error-close:hover {
		background: color-mix(in srgb, var(--state-error) 18%, transparent);
	}
	.player-error-close {
		padding: 0 8px;
		font-size: 1rem;
		line-height: 1.2;
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

	.now-playing-panel .np-artwork-wrap,
	.now-playing-panel .np-progress,
	.now-playing-panel .np-info,
	.now-playing-panel .transport {
		transition:
			max-height var(--motion-base, 240ms) ease,
			opacity var(--motion-base, 240ms) ease,
			gap var(--motion-base, 240ms) ease,
			padding var(--motion-base, 240ms) ease;
	}

	@media (prefers-reduced-motion: reduce) {
		.now-playing-panel .np-artwork-wrap,
		.now-playing-panel .np-progress,
		.now-playing-panel .np-info,
		.now-playing-panel .transport {
			transition: none;
		}
	}

	.now-playing-panel.queue-expanded .np-artwork-wrap {
		max-height: 64px;
		overflow: hidden;
	}

	.now-playing-panel.queue-expanded .np-artwork {
		object-fit: cover;
		object-position: center 30%;
		height: 64px;
	}

	.now-playing-panel.queue-expanded :global(.np-info) {
		padding-block: 6px;
	}

	.now-playing-panel.queue-expanded :global(.np-copy .np-eyebrow) {
		display: none;
	}

	.now-playing-panel.queue-expanded :global(.np-copy .np-album),
	.now-playing-panel.queue-expanded :global(.np-copy .np-source),
	.now-playing-panel.queue-expanded :global(.np-copy .np-stream-detail) {
		display: none;
	}

	.now-playing-panel.queue-expanded :global(.np-copy .np-title) {
		font-size: 0.95rem;
		line-height: 1.2;
		margin: 0;
	}

	.now-playing-panel.queue-expanded :global(.np-copy .np-artist) {
		font-size: 0.78rem;
	}

	.now-playing-panel.queue-expanded :global(.np-progress) {
		max-height: 0;
		opacity: 0;
		overflow: hidden;
		pointer-events: none;
	}

	.now-playing-panel.queue-expanded :global(.transport) {
		gap: 6px;
	}

	.now-playing-panel.queue-expanded :global(.tp-btn),
	.now-playing-panel.queue-expanded :global(.tp-play) {
		width: 30px;
		height: 30px;
	}

	.now-playing-panel.queue-expanded .np-quality {
		display: none;
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
		align-items: baseline;
		gap: 4px;
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

	.queue-count-num {
		flex: 0 0 auto;
		font-size: 0.95rem;
		font-weight: 600;
		color: var(--text-primary);
		font-variant-numeric: tabular-nums;
		line-height: 1;
	}

	.queue-count-unit {
		flex: 1 1 auto;
		min-width: 0;
		font-size: 0.72rem;
		color: var(--text-secondary);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		line-height: 1;
	}

	.queue-header-actions {
		display: flex;
		align-items: center;
		gap: 4px;
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
		font-size: 0.7rem;
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
		font-size: 0.78rem;
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
		font-size: 0.74rem;
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
		font-size: 0.72rem;
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
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 10px;
		border: 1px solid color-mix(in srgb, var(--instrument-border) 46%, transparent);
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--instrument-surface) 78%, transparent);
		cursor: pointer;
		transition:
			border-color var(--motion-fast),
			background var(--motion-fast),
			transform var(--motion-fast);
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
		opacity: 0.55;
	}

	.queue-row.drag-over {
		border-color: var(--accent-line);
		background: color-mix(in srgb, var(--accent-soft) 70%, transparent);
		box-shadow: 0 -2px 0 var(--accent-strong) inset;
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
		font-size: 0.78rem;
		line-height: 1;
		color: var(--text-tertiary);
		cursor: grab;
		opacity: 0;
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

	/* 4px dot in the bottom-right of the artwork encodes where the track came
	   from. Replaces the old .queue-origin chip that ate horizontal space in
	   the artist row. Tooltip on .queue-art-wrap names the source. */
	.queue-source-dot {
		position: absolute;
		right: -2px;
		bottom: -2px;
		width: 8px;
		height: 8px;
		border-radius: 999px;
		border: 2px solid var(--instrument-surface, #14162a);
		background: var(--text-tertiary, rgba(255, 255, 255, 0.5));
	}

	.queue-source-dot.source-automix { background: var(--accent-strong, #6366f1); }
	.queue-source-dot.source-discover { background: #a855f7; }
	.queue-source-dot.source-genre { background: #22d3ee; }
	.queue-source-dot.source-playlist { background: #f59e0b; }
	.queue-source-dot.source-library { background: #10b981; }
	.queue-source-dot.source-manual,
	.queue-source-dot.source-queued { background: var(--text-tertiary, rgba(255, 255, 255, 0.45)); }

	.queue-meta {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.queue-title {
		font-weight: 600;
		font-size: 0.88rem;
		line-height: 1.25;
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
		font-size: 0.76rem;
		line-height: 1.3;
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
	.queue-empty span,
	.queue-overflow {
		color: var(--text-secondary);
		font-size: 0.76rem;
	}

	.queue-side {
		position: relative;
		display: flex;
		align-items: center;
		justify-content: flex-end;
		min-width: 76px;
		flex-shrink: 0;
		margin-left: auto;
	}

	.queue-time {
		transition: opacity var(--motion-fast);
	}

	.queue-row:hover .queue-time,
	.queue-row:focus-within .queue-time {
		opacity: 0;
	}

	.queue-actions {
		position: absolute;
		right: 0;
		top: 50%;
		transform: translateY(-50%);
		display: flex;
		align-items: center;
		gap: 4px;
		opacity: 0;
		pointer-events: none;
		transition: opacity var(--motion-fast);
	}

	.queue-row:hover .queue-actions,
	.queue-row:focus-within .queue-actions {
		opacity: 1;
		pointer-events: auto;
	}

	.queue-action {
		padding: 5px 8px;
		border-radius: 999px;
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 56%, transparent);
		color: var(--text-primary);
		font-size: 0.72rem;
		cursor: pointer;
		transition: background var(--motion-fast), color var(--motion-fast), border-color var(--motion-fast);
	}

	.queue-action:hover {
		background: color-mix(in srgb, var(--instrument-surface-strong) 92%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 82%, transparent);
	}

	.queue-action:disabled {
		cursor: not-allowed;
		opacity: 0.45;
	}

	.queue-action:disabled:hover {
		background: color-mix(in srgb, var(--instrument-surface) 82%, transparent);
		border-color: color-mix(in srgb, var(--instrument-border) 56%, transparent);
		color: var(--text-primary);
	}

	.queue-action.icon {
		width: 28px;
		height: 28px;
		padding: 0;
		display: inline-grid;
		place-items: center;
		font-size: 0.9rem;
		line-height: 1;
	}

	.queue-action.icon.active {
		color: var(--accent-strong, #6366f1);
	}

	.queue-action.icon.remove:hover {
		background: color-mix(in srgb, var(--danger, #f87171) 22%, transparent);
		border-color: color-mix(in srgb, var(--danger, #f87171) 55%, transparent);
		color: var(--danger, #f87171);
	}

	.queue-empty {
		padding: 18px 0;
		display: flex;
		flex-direction: column;
		gap: 4px;
	}

	.queue-empty p {
		font-weight: 600;
	}

	.queue-overflow {
		padding-top: 12px;
		border-top: 1px solid var(--border-subtle);
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
			width: 56px;
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
			font-size: 1rem;
			letter-spacing: 0.03em;
		}

		.mobile-theme-btn {
			font-size: 0.78rem;
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
			font-size: 1rem;
		}

		.mobile-mini-copy {
			display: flex;
			flex-direction: column;
			min-width: 0;
			gap: 2px;
		}

		.mobile-mini-copy strong {
			font-size: 0.87rem;
			font-weight: 600;
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			color: var(--text-primary);
		}

		.mobile-mini-copy span {
			font-size: 0.76rem;
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
			font-size: 18px;
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
			font-size: 19px;
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
			font-size: 10px;
			font-weight: 600;
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
			font-size: 0.92rem;
			font-weight: 500;
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
			font-size: 1rem;
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
			border-radius: 24px 24px 0 0;
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
			font-size: 3rem;
		}

		.mobile-np-quality {
			position: absolute;
			top: 10px;
			right: 10px;
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
			font-size: 1.25rem;
			line-height: 1.1;
			letter-spacing: -0.01em;
			white-space: nowrap;
			overflow: hidden;
			text-overflow: ellipsis;
			display: block;
		}

		.mobile-np-artist {
			color: var(--text-secondary);
			font-size: 0.88rem;
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
			font-size: 20px;
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
			font-size: 0.74rem;
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
			font-size: 20px;
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
			font-size: 24px;
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
			font-size: 0.78rem;
			font-weight: 600;
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
			font-size: 0.8rem;
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
		.queue-side { min-width: auto; align-items: flex-end; }
		.queue-time { display: none; }
		.queue-actions {
			position: static;
			transform: none;
			opacity: 1;
			pointer-events: auto;
		}
	}

	/* ─── Connect screen ───────────────────── */

	.connect-backdrop {
		position: fixed;
		inset: 0;
		z-index: 9999;
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
		width: 72px;
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
		font-size: 1.2rem;
		font-weight: 800;
		letter-spacing: 0.12em;
		color: var(--text-primary);
	}

	.connect-title {
		font-size: 1.25rem;
		font-weight: 700;
		color: var(--text-primary);
	}

	.connect-copy {
		font-size: 0.88rem;
		color: var(--text-secondary);
		line-height: 1.5;
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
		font-size: 1.6rem;
		font-weight: 600;
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
		font-size: 0.82rem;
		color: #ffb0b0;
		text-align: center;
	}

	@media (max-width: 420px) {
		.pin-digit {
			width: 40px;
			height: 52px;
			font-size: 1.4rem;
		}
		.pin-pad { gap: 8px; }
	}
</style>
