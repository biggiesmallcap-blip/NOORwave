<script lang="ts">
	import { fly, fade } from 'svelte/transition';
	import { quintOut } from 'svelte/easing';
	import { goto } from '$app/navigation';
	import { api, type TidalSearchTrack, type TidalSearchAlbum, type TidalSearchArtist } from '$lib/api/client';
	import { commandPaletteOpen } from '$lib/stores/command_palette';
	import { playTidalTrackNow } from '$lib/stores/player';
	import { matchCommands, parseSlashInput, type SlashCommand } from '$lib/search/commands';

	let inputEl = $state<HTMLInputElement | null>(null);
	let query = $state('');
	let loading = $state(false);
	let tracks = $state<TidalSearchTrack[]>([]);
	let albums = $state<TidalSearchAlbum[]>([]);
	let artists = $state<TidalSearchArtist[]>([]);
	let cursor = $state(0);
	let debounceTimer: ReturnType<typeof setTimeout>;

	const isSlashMode = $derived(query.startsWith('/'));
	const slashMatches = $derived(isSlashMode ? matchCommands(query) : []);

	// Total navigable items count (for cursor wrapping)
	const totalItems = $derived(
		isSlashMode ? slashMatches.length : tracks.length + albums.length + artists.length
	);

	$effect(() => {
		if ($commandPaletteOpen) {
			// focus input on next tick
			setTimeout(() => inputEl?.focus(), 10);
			query = '';
			tracks = [];
			albums = [];
			artists = [];
			cursor = 0;
		}
	});

	function close() {
		commandPaletteOpen.set(false);
	}

	function onInput() {
		clearTimeout(debounceTimer);
		cursor = 0;
		if (!query.trim() || isSlashMode) {
			tracks = [];
			albums = [];
			artists = [];
			loading = false;
			return;
		}
		loading = true;
		debounceTimer = setTimeout(async () => {
			try {
				const res = await api.searchTidal(query.trim());
				tracks = res.tracks.slice(0, 5);
				albums = res.albums.slice(0, 3);
				artists = res.artists.slice(0, 3);
			} catch { /* silent */ } finally {
				loading = false;
			}
		}, 220);
	}

	async function selectTrack(track: TidalSearchTrack) {
		close();
		await playTidalTrackNow({ ...track, artist_tidal_id: track.artist_id ?? null });
	}

	function selectAlbum(album: TidalSearchAlbum) {
		close();
		goto(album.local_id ? `/albums/${album.local_id}` : `/tidal/albums/${album.tidal_id}`);
	}

	function selectArtist(artist: TidalSearchArtist) {
		close();
		goto(artist.local_id ? `/artists/${artist.local_id}` : `/tidal/artists/${artist.tidal_id}`);
	}

	async function executeSlashCommand() {
		const { command, arg } = parseSlashInput(query);
		if (!command) return;
		close();
		await command.execute(arg);
	}

	function onKeydown(e: KeyboardEvent) {
		if (e.key === 'Escape') { e.preventDefault(); close(); return; }
		if (e.key === 'ArrowDown') { e.preventDefault(); cursor = (cursor + 1) % Math.max(totalItems, 1); return; }
		if (e.key === 'ArrowUp') { e.preventDefault(); cursor = (cursor - 1 + Math.max(totalItems, 1)) % Math.max(totalItems, 1); return; }
		if (e.key === 'Enter') {
			e.preventDefault();
			if (isSlashMode) {
				if (slashMatches.length > 0) {
					const cmd = slashMatches[cursor] ?? slashMatches[0];
					// If user typed full command + space + arg, execute immediately
					const { command: parsed } = parseSlashInput(query);
					if (parsed) { void executeSlashCommand(); }
					else {
						// Autocomplete to the command
						query = `/${cmd.prefix} `;
					}
				}
				return;
			}
			// Normal search mode: activate cursor item
			const allItems = [...artists, ...albums, ...tracks];
			const item = allItems[cursor];
			if (!item) return;
			if ('name' in item) selectArtist(item as TidalSearchArtist);
			else if (!('duration_ms' in item)) selectAlbum(item as TidalSearchAlbum);
			else void selectTrack(item as TidalSearchTrack);
		}
	}
</script>

{#if $commandPaletteOpen}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<!-- svelte-ignore a11y_no_static_element_interactions -->
	<div class="palette-backdrop" onclick={close} transition:fade={{ duration: 140 }}></div>

	<div
		class="palette-panel"
		role="dialog"
		aria-modal="true"
		aria-label="Command palette"
		transition:fly={{ y: -12, duration: 180, easing: quintOut }}
	>
		<div class="palette-input-wrap">
			<span class="palette-icon">{isSlashMode ? '/' : '⌘'}</span>
			<input
				bind:this={inputEl}
				bind:value={query}
				oninput={onInput}
				onkeydown={onKeydown}
				class="palette-input"
				placeholder={isSlashMode ? 'Type a command…' : 'Search or type / for commands'}
				autocomplete="off"
				spellcheck={false}
			/>
			{#if loading}<span class="palette-spinner">⟳</span>{/if}
		</div>

		{#if isSlashMode && slashMatches.length > 0}
			<ul class="palette-list">
				{#each slashMatches as cmd, i (cmd.prefix)}
					<li>
						<button
							class="palette-row"
							class:palette-row--active={cursor === i}
							onclick={() => { query = `/${cmd.prefix} `; inputEl?.focus(); }}
						>
							<span class="cmd-prefix">/{cmd.prefix}</span>
							{#if cmd.args}<span class="cmd-args">{cmd.args}</span>{/if}
							<span class="cmd-desc">{cmd.description}</span>
						</button>
					</li>
				{/each}
			</ul>
		{:else if !isSlashMode && (tracks.length > 0 || albums.length > 0 || artists.length > 0)}
			<ul class="palette-list">
				{#each artists as artist, i (artist.tidal_id)}
					<li>
						<button
							class="palette-row"
							class:palette-row--active={cursor === i}
							onclick={() => selectArtist(artist)}
						>
							{#if artist.artwork_url}
								<div class="row-art row-art--circle" style="background-image:url('{artist.artwork_url}')"></div>
							{:else}
								<div class="row-art row-art--circle row-art--fallback">♪</div>
							{/if}
							<span class="row-title">{artist.name}</span>
							<span class="row-kind">Artist</span>
							{#if artist.in_library}<span class="row-lib">✓</span>{/if}
						</button>
					</li>
				{/each}
				{#each albums as album, i (album.tidal_id)}
					<li>
						<button
							class="palette-row"
							class:palette-row--active={cursor === artists.length + i}
							onclick={() => selectAlbum(album)}
						>
							{#if album.artwork_url}
								<div class="row-art" style="background-image:url('{album.artwork_url}')"></div>
							{:else}
								<div class="row-art row-art--fallback">♫</div>
							{/if}
							<span class="row-title">{album.title}</span>
							<span class="row-kind">Album</span>
							{#if album.in_library}<span class="row-lib">✓</span>{/if}
						</button>
					</li>
				{/each}
				{#each tracks as track, i (track.tidal_id)}
					<li>
						<button
							class="palette-row"
							class:palette-row--active={cursor === artists.length + albums.length + i}
							onclick={() => void selectTrack(track)}
						>
							{#if track.artwork_url}
								<div class="row-art" style="background-image:url('{track.artwork_url}')"></div>
							{:else}
								<div class="row-art row-art--fallback">♫</div>
							{/if}
							<div class="row-meta">
								<span class="row-title">{track.title}</span>
								{#if track.artist_name}<span class="row-sub">{track.artist_name}</span>{/if}
							</div>
							<span class="row-kind">Track</span>
							{#if track.in_library}<span class="row-lib">✓</span>{/if}
						</button>
					</li>
				{/each}
			</ul>
		{:else if query.trim() && !loading && !isSlashMode}
			<p class="palette-empty">No results</p>
		{:else if !query.trim()}
			<p class="palette-hint">Search Tidal or type <kbd>/</kbd> for commands</p>
		{/if}
	</div>
{/if}

<style>
	.palette-backdrop {
		position: fixed;
		inset: 0;
		background: rgba(0,0,0,0.45);
		z-index: 1400;
	}
	.palette-panel {
		position: fixed;
		top: 18vh;
		left: 50%;
		transform: translateX(-50%);
		width: min(600px, calc(100vw - 32px));
		background: var(--bg-elevated);
		border: 1px solid var(--border-strong);
		border-radius: 14px;
		box-shadow: 0 32px 64px -16px rgba(0,0,0,0.7);
		z-index: 1401;
		overflow: hidden;
	}
	.palette-input-wrap {
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 14px 18px;
		border-bottom: 1px solid var(--border-subtle);
	}
	.palette-icon {
		color: var(--text-muted);
		font-size: 16px;
		flex-shrink: 0;
		width: 16px;
		text-align: center;
	}
	.palette-input {
		flex: 1;
		background: none;
		border: none;
		outline: none;
		font-size: 15px;
		color: var(--text-primary);
		font-family: inherit;
	}
	.palette-input::placeholder { color: var(--text-muted); }
	.palette-spinner {
		color: var(--text-muted);
		font-size: 14px;
		animation: spin 0.8s linear infinite;
	}
	@keyframes spin { to { transform: rotate(360deg); } }
	.palette-list {
		list-style: none;
		padding: 6px 0;
		margin: 0;
		max-height: 400px;
		overflow-y: auto;
	}
	.palette-row {
		width: 100%;
		display: flex;
		align-items: center;
		gap: 10px;
		padding: 8px 18px;
		background: none;
		border: none;
		color: var(--text-primary);
		font-family: inherit;
		font-size: 13px;
		cursor: pointer;
		text-align: left;
	}
	.palette-row:hover, .palette-row--active {
		background: var(--bg-hover);
	}
	.row-art {
		width: 32px; height: 32px;
		border-radius: 4px;
		background: var(--bg-raised);
		background-size: cover;
		background-position: center;
		flex-shrink: 0;
	}
	.row-art--circle { border-radius: 50%; }
	.row-art--fallback {
		display: grid;
		place-items: center;
		font-size: 14px;
		color: var(--text-muted);
	}
	.row-meta { display: flex; flex-direction: column; gap: 1px; min-width: 0; }
	.row-title { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; flex: 1; min-width: 0; }
	.row-sub { font-size: 11px; color: var(--text-muted); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
	.row-kind { font-size: 10px; color: var(--text-muted); margin-left: auto; flex-shrink: 0; }
	.row-lib { font-size: 10px; color: var(--accent); flex-shrink: 0; }
	.cmd-prefix { font-weight: 600; color: var(--accent); font-family: monospace; flex-shrink: 0; }
	.cmd-args { font-size: 11px; color: var(--text-muted); font-family: monospace; flex-shrink: 0; }
	.cmd-desc { color: var(--text-secondary); margin-left: auto; font-size: 12px; }
	.palette-empty, .palette-hint {
		padding: 20px 18px;
		color: var(--text-muted);
		font-size: 13px;
		margin: 0;
	}
	kbd {
		background: var(--bg-raised);
		border: 1px solid var(--border-subtle);
		border-radius: 4px;
		padding: 1px 5px;
		font-size: 11px;
		font-family: monospace;
		color: var(--text-secondary);
	}
</style>
