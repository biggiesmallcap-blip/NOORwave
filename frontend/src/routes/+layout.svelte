<script lang="ts">
	import '../app.css';
	import { onMount } from 'svelte';
	import { page } from '$app/state';
	import { connectWebSocket, wsConnected } from '$lib/api/ws';
	import StateBadge from '$lib/components/ui/StateBadge.svelte';
	import {
		currentTrack,
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
		toggleTrackFavorite
	} from '$lib/stores/player';
	import { formatDuration, getQualityClass } from '$lib/stores/library';

	let { children } = $props();
	let isScrubbing = $state(false);
	let scrubPosition = $state(0);
	let theme = $state<'dark' | 'light'>('dark');
	let displayVolume = $state(Math.round($volume * 100));
	let mobileScrollActive = $state(false);
	let mobileScrollDirection = $state<'up' | 'down' | 'idle'>('idle');
	let mobileQueueOpen = $state(false);
	let mobileFavoritePending = $state(false);
	let mobileScrollTimer: ReturnType<typeof setTimeout> | null = null;
	let lastScrollY = 0;

	const navZones = [
		{
			label: 'Atlas',
			items: [
				{ path: '/', label: 'Home', icon: '⌂' },
				{ path: '/library', label: 'Library', icon: '♫' },
				{ path: '/genres', label: 'Genre Galaxy', icon: '✦' },
				{ path: '/playlists', label: 'Playlists', icon: '☰' },
				{ path: '/discover', label: 'Discover', icon: '✦' }
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

	onMount(() => {
		connectWebSocket();
		void refreshPlaybackState();

		const storedTheme = localStorage.getItem('noor-theme');
		if (storedTheme === 'light' || storedTheme === 'dark') {
			theme = storedTheme;
		}

		applyTheme(theme);

		const handleScroll = () => {
			const nextScrollY = window.scrollY;
			if (nextScrollY > lastScrollY) mobileScrollDirection = 'down';
			else if (nextScrollY < lastScrollY) mobileScrollDirection = 'up';

			lastScrollY = nextScrollY;
			mobileScrollActive = true;

			if (mobileScrollTimer) clearTimeout(mobileScrollTimer);
			mobileScrollTimer = setTimeout(() => {
				mobileScrollActive = false;
				mobileScrollDirection = 'idle';
			}, 220);
		};

		lastScrollY = window.scrollY;
		window.addEventListener('scroll', handleScroll, { passive: true });

		return () => {
			window.removeEventListener('scroll', handleScroll);
			if (mobileScrollTimer) clearTimeout(mobileScrollTimer);
		};
	});

	function applyTheme(t: 'dark' | 'light') {
		theme = t;
		document.documentElement.setAttribute('data-theme', t);
		localStorage.setItem('noor-theme', t);
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
		mobileQueueOpen = false;
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

	let upcomingQueue = $derived.by(() => {
		const currentId = $currentTrack?.id;
		if (!currentId) return $playbackQueue;
		const currentPosition = $playbackQueue.find((item) => item.track.id === currentId)?.position ?? -1;
		return $playbackQueue.filter((item) => item.position > currentPosition);
	});

	let queueCountLabel = $derived(
		upcomingQueue.length === 1 ? '1 track queued' : `${upcomingQueue.length} tracks queued`
	);
	let playerState = $derived(
		$currentTrack ? ($isPlaying ? 'Playing' : 'Paused') : $playerReady ? 'Ready' : 'Connecting'
	);
	let mobilePlayerVisible = $derived(Boolean($currentTrack));
	let mobilePlayerExpanded = $derived(
		Boolean($currentTrack) && (mobileQueueOpen || ($isPlaying && !mobileScrollActive))
	);
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
			mobileQueueOpen = false;
		}
	});

	async function handleMobileFavoriteToggle() {
		if (!$currentTrack || !$currentTrack.tidal_id || mobileFavoritePending) return;
		mobileFavoritePending = true;
		try {
			await toggleTrackFavorite($currentTrack.id);
		} finally {
			mobileFavoritePending = false;
		}
	}
</script>

<div class="app-shell" class:mobile-player-active={mobilePlayerVisible}>
	<aside class="sidebar">
		<a href="/" class="brand" aria-label="NOOR home">
			<span class="brand-mark">N</span>
			<div class="brand-text">
				<span class="brand-name">NOOR</span>
				<span class="brand-sub">Music command center</span>
			</div>
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

	<aside class="now-playing-panel">
		<div class="np-top">
			<div class="np-artwork-wrap">
				{#key $currentTrack?.artwork_url}
					{#if $currentTrack?.artwork_url}
						<img class="np-artwork" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="np-artwork placeholder">♫</div>
					{/if}
				{/key}

				{#if $currentTrack?.best_quality}
					<span class={`quality-badge np-quality ${getQualityClass($currentTrack.best_quality)}`}>
						{formatQuality($currentTrack.best_quality)}
					</span>
				{/if}
			</div>

			<div class="np-info">
				<div class="np-copy">
					<p class="np-eyebrow">Listening Instrument</p>
					<h2 class="np-title">{$currentTrack?.title ?? 'Nothing queued'}</h2>
					<p class="np-artist">{$currentTrack?.artist_name ?? 'Choose a track to begin playback.'}</p>
					<p class="np-album">{$currentTrack?.album_title ?? 'Playback controls stay docked here.'}</p>
				</div>

				<StateBadge label={playerState} tone={$currentTrack ? 'active' : 'muted'} compact={true} />
			</div>

			<div class="np-progress">
				<div class="np-progress-track" style="--pct: {progressWidth}">
					<div class="np-progress-fill" style={`width: ${progressWidth}`}></div>
					<input
						class="np-progress-input"
						type="range"
						min="0"
						max={$currentTrack?.duration_ms ?? 0}
						step="1000"
						bind:value={scrubPosition}
						oninput={beginScrub}
						onchange={() => void commitScrub()}
						disabled={!$currentTrack?.duration_ms}
						aria-label="Seek playback"
					/>
				</div>

				<div class="np-times">
					<span>{formatDuration(scrubPosition)}</span>
					<span>{formatDuration($currentTrack?.duration_ms ?? 0)}</span>
				</div>
			</div>

			<div class="transport">
				<button
					class:active={$shuffleMode !== 'off'}
					class="tp-btn"
					title={shuffleLabels[$shuffleMode]}
					aria-label={shuffleLabels[$shuffleMode]}
					onclick={() => void cyclePlayerShuffleMode()}
				>
					{shuffleIcons[$shuffleMode]}
				</button>
				<button class="tp-btn" onclick={() => void playPreviousTrack()} aria-label="Previous">⏮</button>
				<button class="tp-play" onclick={() => void togglePlayback()} aria-label="Play or pause">
					{$isPlaying ? '⏸' : '▶'}
				</button>
				<button class="tp-btn" onclick={() => void playNextTrack()} aria-label="Next">⏭</button>
				<button
					class:active={$repeatMode !== 'off'}
					class="tp-btn"
					title={repeatLabels[$repeatMode]}
					aria-label={repeatLabels[$repeatMode]}
					onclick={() => void cyclePlayerRepeatMode()}
				>
					{repeatIcons[$repeatMode]}
				</button>
			</div>

			<div class="np-controls">
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
				<p class="player-error">{$playerError}</p>
			{/if}
		</div>

		<section class="queue-section">
			<div class="queue-header">
				<div>
					<p class="queue-eyebrow">Up next</p>
					<h3>{queueCountLabel}</h3>
				</div>
				<div class="queue-header-actions">
					<button
						class="queue-automix-btn"
						class:active={$automixEnabled}
						title={$automixEnabled ? 'Automix on' : 'Automix off'}
						aria-label={$automixEnabled ? 'Disable automix' : 'Enable automix'}
						onclick={() => void togglePlayerAutomix()}
					>🎧</button>
					<button
						class="queue-discover-btn"
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
					<span class="queue-count">{Math.min(upcomingQueue.length, 40)}</span>
				</div>
			</div>

			{#if upcomingQueue.length > 0}
				<div class="queue-list">
					{#each upcomingQueue.slice(0, 40) as item (item.id)}
						<div
							class:active={$currentTrack?.id === item.track.id}
							class="queue-row"
							role="button"
							tabindex="0"
							onclick={() => void handleQueueTrackPlay(item.track.id)}
							onkeydown={(event) => handleQueueTrackKeydown(item.track.id, event)}
						>
							{#if item.track.artwork_url}
								<img class="queue-art" src={item.track.artwork_url} alt="" />
							{:else}
								<div class="queue-art placeholder">♫</div>
							{/if}

							<div class="queue-meta">
								<p>{item.track.title}</p>
								<div class="queue-subline">
									<span>{item.track.artist_name ?? 'Unknown artist'}</span>
									<span class="queue-origin">{formatQueueSource(item.source)}</span>
								</div>
							</div>

							<div class="queue-side">
								<span class="queue-time">{formatDuration(item.track.duration_ms)}</span>
								<div class="queue-actions">
									<button class="queue-action" onclick={(event) => void handleQueueMoveNext(item.id, event)}>
										Next
									</button>
									<button class="queue-action remove" onclick={(event) => void handleQueueRemove(item.id, event)}>
										×
									</button>
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

	{#if $currentTrack}
		{#if mobileQueueOpen}
			<button
				class="mobile-player-backdrop"
				type="button"
				aria-label="Close mobile queue"
				onclick={() => {
					mobileQueueOpen = false;
				}}
			></button>

			<section class="mobile-player-sheet glass-panel" aria-label="Mobile queue">
				<div class="mobile-sheet-header">
					<div>
						<p class="queue-eyebrow">Up next</p>
						<h3>{queueCountLabel}</h3>
					</div>

					<button
						class="mobile-player-chip compact"
						type="button"
						aria-label="Close queue"
						onclick={() => {
							mobileQueueOpen = false;
						}}
					>
						Close
					</button>
				</div>

				<div class="mobile-sheet-controls">
					<button
						class="mobile-player-chip"
						class:active={$shuffleMode !== 'off'}
						type="button"
						title={shuffleLabels[$shuffleMode]}
						aria-label={shuffleLabels[$shuffleMode]}
						onclick={() => void cyclePlayerShuffleMode()}
					>
						<span>{shuffleIcons[$shuffleMode]}</span>
						<span>{$shuffleMode === 'off' ? 'Shuffle' : shuffleLabels[$shuffleMode]}</span>
					</button>

					<button
						class="mobile-player-chip"
						class:active={$automixEnabled}
						type="button"
						aria-label={$automixEnabled ? 'Disable automix' : 'Enable automix'}
						onclick={() => void togglePlayerAutomix()}
					>
						<span>🎧</span>
						<span>{$automixEnabled ? 'Automix on' : 'Automix off'}</span>
					</button>

					<button
						class="mobile-player-chip"
						class:active={$automixDiscoverNew}
						type="button"
						aria-label={$automixDiscoverNew ? 'Disable discover new' : 'Enable discover new'}
						onclick={() => void setPlayerDiscoverNew(!$automixDiscoverNew)}
					>
						<svg width="12" height="12" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg" aria-hidden="true">
							<path d="M7.5 1a6.5 6.5 0 1 0 0 13A6.5 6.5 0 0 0 7.5 1zm0 1a5.5 5.5 0 1 1 0 11A5.5 5.5 0 0 1 7.5 2zM7 4.5V7H4.5a.5.5 0 0 0 0 1H7v2.5a.5.5 0 0 0 1 0V8h2.5a.5.5 0 0 0 0-1H8V4.5a.5.5 0 0 0-1 0z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd"/>
						</svg>
						<span>Include New</span>
					</button>
				</div>

				{#if upcomingQueue.length > 0}
					<div class="queue-list mobile-sheet-list">
						{#each upcomingQueue.slice(0, 40) as item (item.id)}
							<div
								class:active={$currentTrack?.id === item.track.id}
								class="queue-row"
								role="button"
								tabindex="0"
								onclick={() => void handleQueueTrackPlay(item.track.id)}
								onkeydown={(event) => handleQueueTrackKeydown(item.track.id, event)}
							>
								{#if item.track.artwork_url}
									<img class="queue-art" src={item.track.artwork_url} alt="" />
								{:else}
									<div class="queue-art placeholder">♫</div>
								{/if}

								<div class="queue-meta">
									<p>{item.track.title}</p>
									<div class="queue-subline">
										<span>{item.track.artist_name ?? 'Unknown artist'}</span>
										<span class="queue-origin">{formatQueueSource(item.source)}</span>
									</div>
								</div>

								<div class="queue-side">
									<span class="queue-time">{formatDuration(item.track.duration_ms)}</span>
									<div class="queue-actions">
										<button class="queue-action" onclick={(event) => void handleQueueMoveNext(item.id, event)}>
											Next
										</button>
										<button class="queue-action remove" onclick={(event) => void handleQueueRemove(item.id, event)}>
											×
										</button>
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
		{/if}

		<section
			class="mobile-sticky-player glass-panel"
			class:expanded={mobilePlayerExpanded}
			class:collapsed={!mobilePlayerExpanded}
			data-scroll-direction={mobileScrollDirection}
			aria-label="Sticky mobile player"
		>
			<div class="mobile-player-progress" aria-hidden="true">
				<div class="mobile-player-progress-fill" style={`width: ${progressWidth}`}></div>
			</div>

			<div class="mobile-player-head">
				<div class="mobile-player-main">
					{#if $currentTrack.artwork_url}
						<img class="mobile-player-art" src={$currentTrack.artwork_url} alt="" />
					{:else}
						<div class="mobile-player-art placeholder">♫</div>
					{/if}

					<div class="mobile-player-copy">
						<strong>{$currentTrack.title}</strong>
						<span>{$currentTrack.artist_name ?? 'Unknown artist'}</span>
					</div>
				</div>

				<div class="mobile-player-actions">
					<button class="mobile-player-btn" type="button" aria-label="Previous" onclick={() => void playPreviousTrack()}>
						⏮
					</button>
					<button class="mobile-player-btn primary" type="button" aria-label="Play or pause" onclick={() => void togglePlayback()}>
						{$isPlaying ? '⏸' : '▶'}
					</button>
					<button class="mobile-player-btn" type="button" aria-label="Next" onclick={() => void playNextTrack()}>
						⏭
					</button>
					<button
						class="mobile-player-btn queue-toggle"
						class:active={mobileQueueOpen}
						type="button"
						aria-expanded={mobileQueueOpen}
						aria-label={mobileQueueOpen ? 'Hide queue' : 'Show queue'}
						onclick={() => {
							mobileQueueOpen = !mobileQueueOpen;
						}}
					>
						☰
						<span class="mobile-player-btn-badge">{Math.min(upcomingQueue.length, 40)}</span>
					</button>
				</div>
			</div>

			<div class="mobile-player-toolbar">
				<button
					class="mobile-player-chip"
					class:active={$currentTrack.is_favorite}
					type="button"
					disabled={mobileFavoritePending || !$currentTrack.tidal_id}
					aria-label={$currentTrack.is_favorite ? 'Unlike track' : 'Like track'}
					title={$currentTrack.is_favorite ? 'Remove from TIDAL favorites' : 'Save to TIDAL favorites'}
					onclick={() => void handleMobileFavoriteToggle()}
				>
					<span>{$currentTrack.is_favorite ? '♥' : '♡'}</span>
					<span>{$currentTrack.is_favorite ? 'Liked' : 'Like'}</span>
				</button>

				<button
					class="mobile-player-chip"
					class:active={$shuffleMode !== 'off'}
					type="button"
					title={shuffleLabels[$shuffleMode]}
					aria-label={shuffleLabels[$shuffleMode]}
					onclick={() => void cyclePlayerShuffleMode()}
				>
					<span>{shuffleIcons[$shuffleMode]}</span>
					<span>{$shuffleMode === 'off' ? 'Shuffle' : 'Shuffling'}</span>
				</button>
			</div>

			<div class="mobile-player-extra">
				<div class="mobile-player-meta">
					<StateBadge label={playerState} tone={$isPlaying ? 'active' : 'muted'} compact={true} />
					{#if $currentTrack.best_quality}
						<span class={`quality-badge ${getQualityClass($currentTrack.best_quality)}`}>
							{formatQuality($currentTrack.best_quality)}
						</span>
					{/if}
					<span class="mobile-player-queue">{queueCountLabel}</span>
					{#if $automixEnabled}
						<span class="mobile-player-flag">Automix</span>
					{/if}
				</div>

				<div class="mobile-player-scrub">
					<input
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
					<div class="mobile-player-times">
						<span>{formatDuration(scrubPosition)}</span>
						<span>{formatDuration($currentTrack.duration_ms ?? 0)}</span>
					</div>
				</div>
			</div>
		</section>
	{/if}
</div>

<style>
	.app-shell {
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

	.mobile-sticky-player {
		display: none;
	}

	.mobile-player-backdrop,
	.mobile-player-sheet {
		display: none;
	}

	.brand {
		display: flex;
		align-items: center;
		gap: 12px;
		padding: 4px 6px 18px;
	}

	.brand-mark {
		width: 34px;
		height: 34px;
		border-radius: 10px;
		background: var(--accent-soft);
		border: 1px solid var(--accent-line);
		color: var(--accent-strong);
		font-family: var(--font-display);
		font-size: 1.1rem;
		display: grid;
		place-items: center;
		flex-shrink: 0;
	}

	.brand-text {
		display: flex;
		flex-direction: column;
		min-width: 0;
	}

	.brand-name {
		font-family: var(--font-display);
		font-size: 1.08rem;
		letter-spacing: 0.03em;
	}

	.brand-sub {
		color: var(--text-secondary);
		font-size: 0.77rem;
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
	.np-album,
	.queue-meta p {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.np-title {
		font-size: 1.35rem;
		font-family: var(--font-display);
		line-height: 1.1;
		letter-spacing: -0.02em;
	}

	.np-artist {
		color: var(--text-primary);
		font-size: 0.9rem;
	}

	.np-album {
		color: var(--text-secondary);
		font-size: 0.8rem;
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
		padding: 10px 12px;
		border-radius: var(--radius-sm);
		background: color-mix(in srgb, var(--state-error) 12%, transparent);
		border: 1px solid color-mix(in srgb, var(--state-error) 24%, transparent);
		color: var(--state-error);
		font-size: 0.78rem;
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

	.queue-header {
		display: flex;
		align-items: flex-start;
		justify-content: space-between;
		gap: 12px;
		padding-bottom: 12px;
	}

	.queue-header-actions {
		display: flex;
		align-items: center;
		gap: 8px;
		flex-shrink: 0;
	}

	.queue-automix-btn {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		font-size: 0.82rem;
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.queue-automix-btn:hover {
		background: var(--bg-hover);
		border-color: var(--border-strong);
	}

	.queue-automix-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		box-shadow: 0 0 10px var(--accent), 0 0 0 1px var(--accent-line);
	}

	.queue-discover-btn {
		width: 28px;
		height: 28px;
		border-radius: 50%;
		display: grid;
		place-items: center;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		transition:
			background var(--motion-fast),
			border-color var(--motion-fast),
			color var(--motion-fast),
			box-shadow var(--motion-fast);
	}

	.queue-discover-btn:hover {
		background: var(--bg-hover);
		border-color: var(--border-strong);
		color: var(--text-primary);
	}

	.queue-discover-btn.active {
		background: var(--accent-soft);
		border-color: var(--accent-line);
		color: var(--accent-strong);
		box-shadow: 0 0 10px var(--accent), 0 0 0 1px var(--accent-line);
	}

	.queue-eyebrow {
		color: var(--text-tertiary);
		font-size: 0.72rem;
		text-transform: uppercase;
		letter-spacing: 0.08em;
		margin-bottom: 2px;
	}

	.queue-header h3 {
		font-size: 1rem;
	}

	.queue-count {
		min-width: 28px;
		height: 28px;
		padding: 0 8px;
		display: inline-grid;
		place-items: center;
		border-radius: 99px;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		color: var(--text-secondary);
		font-size: 0.75rem;
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

	.queue-row.active .queue-meta p {
		color: var(--accent-strong);
	}

	.queue-art {
		width: 42px;
		height: 42px;
		border-radius: 12px;
		object-fit: cover;
		background: var(--bg-surface);
		border: 1px solid var(--border-subtle);
		flex-shrink: 0;
	}

	.queue-meta {
		min-width: 0;
		flex: 1;
		display: flex;
		flex-direction: column;
	}

	.queue-meta p {
		font-weight: 600;
	}

	.queue-subline span,
	.queue-time,
	.queue-empty span,
	.queue-overflow {
		color: var(--text-secondary);
		font-size: 0.76rem;
	}

	.queue-subline {
		display: flex;
		align-items: center;
		gap: 6px;
		min-width: 0;
	}

	.queue-subline span:first-child {
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.queue-origin {
		padding: 2px 8px;
		border-radius: 999px;
		font-size: 0.64rem;
		font-weight: 700;
		letter-spacing: 0.07em;
		text-transform: uppercase;
		color: var(--signal-text);
		background: color-mix(in srgb, var(--instrument-surface-strong) 90%, transparent);
		border: 1px solid color-mix(in srgb, var(--instrument-border) 42%, transparent);
		flex-shrink: 0;
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
		gap: 6px;
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
	}

	.queue-action.remove {
		width: 28px;
		padding: 5px 0;
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

	@media (max-width: 1180px) {
		.app-shell {
			height: auto;
			min-height: 100dvh;
			grid-template-columns: 1fr;
			grid-template-rows: auto minmax(0, 1fr);
			overflow: visible;
			background: transparent;
		}

		.app-shell.mobile-player-active {
			padding-bottom: 206px;
		}

		.sidebar {
			border-right: none;
			border-bottom: 1px solid var(--border-subtle);
			padding-bottom: 16px;
			overflow: visible;
			background:
				linear-gradient(180deg, color-mix(in srgb, var(--instrument-surface) 76%, transparent), color-mix(in srgb, var(--instrument-surface-strong) 68%, transparent)),
				transparent;
		}

		.nav {
			flex-direction: row;
			overflow-x: auto;
			padding-bottom: 4px;
			gap: 10px;
		}

		.nav-zone {
			flex-direction: row;
			gap: 4px;
			flex-shrink: 0;
		}

		.nav-zone-label {
			display: none;
		}

		.nav-item {
			flex-shrink: 0;
		}

		.sidebar-footer {
			margin-top: 16px;
		}

		.workspace {
			padding: 22px 18px 30px;
			overflow: visible;
		}

		.now-playing-panel {
			display: none;
		}

		.mobile-player-backdrop {
			position: fixed;
			inset: 0;
			display: block;
			padding: 0;
			border: 0;
			background: rgba(3, 6, 12, 0.38);
			backdrop-filter: blur(10px);
			z-index: 27;
		}

		.mobile-player-sheet {
			position: fixed;
			left: 50%;
			bottom: 0;
			width: min(44rem, calc(100vw - 8px));
			max-height: min(64dvh, 40rem);
			display: flex;
			flex-direction: column;
			gap: 12px;
			padding: 18px 16px calc(226px + var(--safe-bottom));
			border-radius: 28px 28px 0 0;
			transform: translateX(-50%);
			z-index: 28;
			box-shadow:
				0 -18px 42px rgba(0, 0, 0, 0.28),
				inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 45%, transparent);
		}

		.mobile-sheet-header {
			display: flex;
			align-items: flex-start;
			justify-content: space-between;
			gap: 12px;
		}

		.mobile-sheet-controls {
			display: flex;
			align-items: center;
			gap: 10px;
			flex-wrap: wrap;
		}

		.mobile-sheet-list {
			padding-right: 2px;
		}

		.mobile-sticky-player {
			position: fixed;
			left: 50%;
			bottom: calc(10px + var(--safe-bottom));
			width: min(38rem, calc(100vw - 12px));
			min-height: 114px;
			display: flex;
			flex-direction: column;
			gap: 11px;
			padding: 14px 15px 15px;
			border-radius: 24px;
			transform: translateX(-50%);
			z-index: 30;
			transition:
				box-shadow var(--motion-base),
				border-color var(--motion-fast),
				padding var(--motion-base);
			box-shadow:
				0 24px 48px rgba(0, 0, 0, 0.34),
				inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 42%, transparent);
		}

		.mobile-sticky-player.expanded {
			box-shadow:
				0 28px 56px rgba(0, 0, 0, 0.42),
				inset 0 1px 0 color-mix(in srgb, var(--instrument-edge) 52%, transparent);
		}

		.mobile-sticky-player.collapsed {
			min-height: 96px;
			gap: 8px;
			padding-top: 12px;
			padding-bottom: 12px;
		}

		.mobile-player-progress {
			height: 4px;
			border-radius: 999px;
			background: color-mix(in srgb, var(--instrument-border) 34%, transparent);
			overflow: hidden;
		}

		.mobile-player-progress-fill {
			height: 100%;
			background: var(--accent);
			box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 72%, transparent);
		}

		.mobile-player-head {
			display: flex;
			align-items: center;
			justify-content: space-between;
			gap: 14px;
		}

		.mobile-player-main {
			display: flex;
			align-items: center;
			gap: 14px;
			min-width: 0;
			flex: 1;
		}

		.mobile-player-art {
			width: 62px;
			height: 62px;
			border-radius: 18px;
			object-fit: cover;
			background: var(--bg-surface);
			border: 1px solid var(--border-subtle);
			flex-shrink: 0;
			transition: width var(--motion-base), height var(--motion-base), border-radius var(--motion-base);
		}

		.mobile-player-art.placeholder {
			display: grid;
			place-items: center;
			color: var(--text-tertiary);
		}

		.mobile-player-copy {
			display: flex;
			flex-direction: column;
			min-width: 0;
			gap: 4px;
		}

		.mobile-player-copy strong,
		.mobile-player-copy span {
			overflow: hidden;
			text-overflow: ellipsis;
			white-space: nowrap;
		}

		.mobile-player-copy strong {
			font-size: 18px;
			color: var(--text-primary);
			line-height: 1.15;
		}

		.mobile-player-copy span,
		.mobile-player-queue,
		.mobile-player-times span {
			font-size: 14px;
			color: var(--text-secondary);
		}

		.mobile-player-actions {
			display: flex;
			align-items: center;
			gap: 9px;
			flex-shrink: 0;
		}

		.mobile-player-btn {
			width: 42px;
			height: 42px;
			border-radius: 50%;
			display: grid;
			place-items: center;
			background: color-mix(in srgb, var(--instrument-surface) 84%, transparent);
			border: 1px solid color-mix(in srgb, var(--instrument-border) 58%, transparent);
			color: var(--text-primary);
			font-size: 17px;
			position: relative;
		}

		.mobile-player-btn.primary {
			background: var(--accent);
			color: white;
			width: 50px;
			height: 50px;
			font-size: 20px;
			box-shadow: 0 12px 26px var(--accent-glow);
		}

		.mobile-player-btn.queue-toggle.active {
			background: color-mix(in srgb, var(--accent-soft) 76%, var(--instrument-surface));
			border-color: color-mix(in srgb, var(--accent-line) 74%, transparent);
			color: var(--accent-strong);
		}

		.mobile-player-btn-badge {
			position: absolute;
			right: -2px;
			top: -2px;
			min-width: 18px;
			height: 18px;
			padding: 0 4px;
			border-radius: 999px;
			display: inline-grid;
			place-items: center;
			background: var(--accent);
			color: white;
			font-size: 10px;
			font-weight: 700;
			line-height: 1;
			box-shadow: 0 6px 12px color-mix(in srgb, var(--accent-glow) 64%, transparent);
		}

		.mobile-player-toolbar {
			display: flex;
			align-items: center;
			gap: 10px;
			flex-wrap: wrap;
			max-height: 48px;
			overflow: hidden;
			opacity: 1;
			transition:
				max-height var(--motion-base),
				opacity var(--motion-base),
				margin-top var(--motion-base);
		}

		.mobile-player-chip {
			min-height: 36px;
			padding: 0 13px;
			border-radius: 999px;
			display: inline-flex;
			align-items: center;
			justify-content: center;
			gap: 8px;
			background: color-mix(in srgb, var(--instrument-surface) 88%, transparent);
			border: 1px solid color-mix(in srgb, var(--instrument-border) 56%, transparent);
			color: var(--text-primary);
			font-size: 12px;
			font-weight: 600;
			letter-spacing: 0.01em;
			white-space: nowrap;
			transition:
				background var(--motion-fast),
				border-color var(--motion-fast),
				box-shadow var(--motion-fast),
				transform var(--motion-fast);
		}

		.mobile-player-chip:hover {
			transform: translateY(-1px);
		}

		.mobile-player-chip.active {
			background: color-mix(in srgb, var(--accent-soft) 80%, var(--instrument-surface));
			border-color: color-mix(in srgb, var(--accent-line) 78%, transparent);
			color: var(--accent-strong);
			box-shadow: 0 0 18px color-mix(in srgb, var(--accent-glow) 36%, transparent);
		}

		.mobile-player-chip:disabled {
			opacity: 0.52;
		}

		.mobile-player-chip.compact {
			min-height: 34px;
			padding: 0 12px;
		}

		.mobile-player-chip-count {
			min-width: 22px;
			height: 22px;
			padding: 0 6px;
			border-radius: 999px;
			display: inline-grid;
			place-items: center;
			background: color-mix(in srgb, var(--instrument-surface-strong) 94%, transparent);
			border: 1px solid color-mix(in srgb, var(--instrument-border) 46%, transparent);
			color: inherit;
			font-size: 0.72rem;
		}

		.mobile-player-extra {
			display: grid;
			grid-template-rows: 1fr;
			overflow: hidden;
			opacity: 1;
			transition:
				grid-template-rows var(--motion-base),
				opacity var(--motion-base),
				margin-top var(--motion-base);
		}

		.mobile-player-extra > * {
			min-height: 0;
		}

		.mobile-player-meta,
		.mobile-player-scrub {
			display: flex;
			flex-direction: column;
			gap: 12px;
		}

		.mobile-player-meta {
			flex-direction: row;
			align-items: center;
			flex-wrap: wrap;
			gap: 8px;
			margin-bottom: 10px;
		}

		.mobile-player-flag {
			padding: 4px 10px;
			border-radius: 999px;
			border: 1px solid color-mix(in srgb, var(--instrument-border) 46%, transparent);
			background: color-mix(in srgb, var(--instrument-surface-strong) 88%, transparent);
			color: var(--signal-text);
			font-size: 0.72rem;
			font-weight: 700;
			letter-spacing: 0.06em;
			text-transform: uppercase;
		}

		.mobile-player-scrub input {
			width: 100%;
		}

		.mobile-player-times {
			display: flex;
			align-items: center;
			justify-content: space-between;
			font-variant-numeric: tabular-nums;
		}

		.mobile-sticky-player.collapsed .mobile-player-progress {
			opacity: 0.85;
		}

		.mobile-sticky-player.collapsed .mobile-player-extra {
			grid-template-rows: 0fr;
			opacity: 0;
			margin-top: -6px;
		}

		.mobile-sticky-player.collapsed .mobile-player-toolbar {
			max-height: 0;
			opacity: 0;
			margin-top: -8px;
			pointer-events: none;
		}

		.mobile-sticky-player.collapsed .mobile-player-art {
			width: 54px;
			height: 54px;
			border-radius: 16px;
		}
	}

	@media (max-width: 760px) {
		.app-shell.mobile-player-active {
			padding-bottom: 196px;
		}

		.sidebar {
			padding: 14px 12px;
		}

		.workspace {
			padding: 18px 14px 24px;
		}

		.sidebar-footer {
			padding-top: 14px;
		}

		.np-top,
		.queue-section {
			padding-left: 14px;
			padding-right: 14px;
		}

		.np-title {
			font-size: 1.16rem;
		}

		.queue-row {
			align-items: flex-start;
		}

		.queue-side {
			min-width: auto;
			align-items: flex-end;
		}

		.queue-time {
			display: none;
		}

		.queue-actions {
			position: static;
			transform: none;
			opacity: 1;
			pointer-events: auto;
		}

		.mobile-player-sheet {
			width: calc(100vw - 4px);
			padding: 16px 14px calc(216px + var(--safe-bottom));
		}

		.mobile-sticky-player.collapsed {
			min-height: 90px;
			padding-top: 12px;
			padding-bottom: 12px;
		}

		.mobile-sticky-player.collapsed .mobile-player-art {
			width: 46px;
			height: 46px;
			border-radius: 14px;
		}

		.mobile-sticky-player {
			width: calc(100vw - 8px);
			min-height: 108px;
			padding: 13px 14px 14px;
		}

		.mobile-player-head {
			gap: 10px;
		}

		.mobile-player-btn {
			width: 40px;
			height: 40px;
		}

		.mobile-player-btn.primary {
			width: 46px;
			height: 46px;
		}

		.mobile-player-chip {
			min-height: 36px;
			padding: 0 12px;
			font-size: 12px;
		}

		.mobile-player-copy strong {
			font-size: 17px;
		}

		.mobile-player-copy span,
		.mobile-player-queue,
		.mobile-player-times span {
			font-size: 13px;
		}

	}

	@media (max-width: 1180px) and (orientation: portrait) {
		.app-shell.mobile-player-active {
			padding-bottom: 214px;
		}

		.mobile-player-sheet {
			width: calc(100vw - 4px);
			padding: 18px 14px calc(236px + var(--safe-bottom));
		}

		.mobile-sticky-player {
			bottom: calc(6px + var(--safe-bottom));
			width: calc(100vw - 6px);
			min-height: 118px;
			padding: 14px 14px 14px;
			border-radius: 24px;
		}

		.mobile-sticky-player.collapsed {
			min-height: 100px;
			padding-top: 12px;
			padding-bottom: 12px;
		}

		.mobile-player-art {
			width: 64px;
			height: 64px;
			border-radius: 18px;
		}

		.mobile-sticky-player.collapsed .mobile-player-art {
			width: 56px;
			height: 56px;
			border-radius: 16px;
		}

		.mobile-player-copy strong {
			font-size: 18px;
		}

		.mobile-player-copy span,
		.mobile-player-queue,
		.mobile-player-times span {
			font-size: 14px;
		}

		.mobile-player-btn {
			width: 42px;
			height: 42px;
			font-size: 17px;
		}

		.mobile-player-btn.primary {
			width: 50px;
			height: 50px;
			font-size: 19px;
		}

		.mobile-player-chip {
			min-height: 36px;
			padding: 0 13px;
			font-size: 12px;
		}
	}

	@media (max-width: 760px) and (orientation: portrait) {
		.app-shell.mobile-player-active {
			padding-bottom: 204px;
		}

		.mobile-player-sheet {
			padding: 16px 12px calc(226px + var(--safe-bottom));
		}

		.mobile-sticky-player {
			width: calc(100vw - 4px);
			min-height: 112px;
			padding: 13px 12px 13px;
		}

		.mobile-sticky-player.collapsed {
			min-height: 96px;
		}

		.mobile-player-head {
			gap: 10px;
		}

		.mobile-player-main {
			gap: 10px;
		}

		.mobile-player-art {
			width: 60px;
			height: 60px;
			border-radius: 17px;
		}

		.mobile-sticky-player.collapsed .mobile-player-art {
			width: 52px;
			height: 52px;
			border-radius: 15px;
		}
	}
</style>
