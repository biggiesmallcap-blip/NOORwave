<script lang="ts">
	import { formatTrackDuration } from '$lib/utils/format';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import { buildTidalTrackMenu } from '$lib/player/track_menu';
	import { buildArtistMenu } from '$lib/player/artist_menu';
	import { buildAlbumMenu } from '$lib/player/album_menu';
	import {
		addTidalTrackToQueue,
		startTidalSongRadio,
		playTidalTrackNow,
		toggleTidalTrackFavorite
	} from '$lib/stores/player';
	import type { TidalPlayable } from '$lib/api/client';
	import { canPlayTrack, getPlayableLabel } from '$lib/player/playable';
	import ArtworkImage from '$lib/components/ui/ArtworkImage.svelte';

	export type TidalTrackRowVariant = 'numbered' | 'indexed' | 'art' | 'compact';

	interface TidalTrackInput extends TidalPlayable {
		track_number?: number | null;
	}

	let {
		track,
		variant,
		isCurrent = false,
		isPlaying = false,
		index = 0,
		showAlbum = true,
		showArtist = true,
		onRowClick
	}: {
		track: TidalTrackInput;
		variant: TidalTrackRowVariant;
		isCurrent?: boolean;
		isPlaying?: boolean;
		index?: number;
		showAlbum?: boolean;
		showArtist?: boolean;
		onRowClick?: () => void;
	} = $props();

	const playable = $derived(canPlayTrack(track));
	const playableLabel = $derived(getPlayableLabel(track));

	function defaultClick() {
		if (!playable) return;
		void playTidalTrackNow(track);
	}

	function handleRowClick() {
		if (onRowClick) onRowClick();
		else defaultClick();
	}

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			handleRowClick();
		}
	}

	function handleContextMenu(e: MouseEvent) {
		e.preventDefault();
		openContextMenu(e, buildTidalTrackMenu(track), track.title);
	}

	function openArtistContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		if (track.artist_tidal_id == null || !track.artist_name) return;
		openContextMenu(
			e,
			buildArtistMenu({ tidal_id: track.artist_tidal_id, name: track.artist_name, in_library: false }, { isLocal: false }),
			track.artist_name
		);
	}

	function openAlbumContextMenu(e: MouseEvent) {
		e.preventDefault();
		e.stopPropagation();
		if (track.album_tidal_id == null || !track.album_title) return;
		openContextMenu(
			e,
			buildAlbumMenu({
				tidal_id: track.album_tidal_id,
				title: track.album_title,
				artist_name: track.artist_name ?? null,
				in_library: false,
			}, { isLocal: false }),
			track.album_title
		);
	}

	function handleMoreClick(e: MouseEvent) {
		e.stopPropagation();
		openMenuAtElement(
			e.currentTarget as HTMLElement,
			buildTidalTrackMenu(track),
			track.title
		);
	}

	function handlePlay(e: MouseEvent) {
		e.stopPropagation();
		if (!playable) return;
		void playTidalTrackNow(track);
	}

	function handleAddToQueue(e: MouseEvent) {
		e.stopPropagation();
		if (!playable) return;
		void addTidalTrackToQueue(track);
	}

	function handleSongRadio(e: MouseEvent) {
		e.stopPropagation();
		void startTidalSongRadio(track);
	}

	// External TIDAL tracks have no local row until imported, so the heart carries
	// its own optimistic override on top of the prop's initial value. Import
	// happens on demand inside toggleTidalTrackFavorite. `override` stays null
	// until the user acts, so the row still tracks the prop if it changes.
	let favoriteOverride = $state<boolean | null>(null);
	const isFavorite = $derived(favoriteOverride ?? (track.is_favorite ?? false));
	let favoritePending = $state(false);
	// Once imported, remember the minted local id so a second toggle updates the
	// existing row instead of re-importing.
	let mintedLocalId = $state<number | null>(null);
	async function handleHeart(e: MouseEvent) {
		e.stopPropagation();
		if (favoritePending) return;
		favoritePending = true;
		const previous = isFavorite;
		favoriteOverride = !previous; // optimistic
		const seed = mintedLocalId != null ? { ...track, track_id: mintedLocalId } : track;
		const result = await toggleTidalTrackFavorite(seed, previous);
		if (result) {
			favoriteOverride = result.is_favorite;
			mintedLocalId = result.local_id;
		} else {
			favoriteOverride = previous; // rollback
		}
		favoritePending = false;
	}
</script>

{#if variant === 'numbered'}
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row numbered"
		class:active={isCurrent}
		class:disabled={!playable}
		role="button"
		tabindex={playable ? 0 : -1}
		aria-disabled={!playable}
		onclick={handleRowClick}
		ondblclick={() => playable && void playTidalTrackNow(track)}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		<span class="cell-index">{index + 1}</span>
		<div class="cell-art">
			{#if track.artwork_url}
				<ArtworkImage
					className="art"
					src={track.artwork_url}
					alt={track.title}
					size={320}
					fallbackText={track.title.slice(0, 2).toUpperCase()}
				/>
			{:else}
				<div class="art placeholder">♫</div>
			{/if}
		</div>
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			<p class="sub">
				{#if showArtist && track.artist_name}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span class="sub-link" oncontextmenu={openArtistContextMenu}>{track.artist_name}</span>
				{/if}
				{#if showArtist && track.artist_name && showAlbum && track.album_title} - {/if}
				{#if showAlbum && track.album_title}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span class="sub-link" oncontextmenu={openAlbumContextMenu}>{track.album_title}</span>
				{/if}
			</p>
		</div>
		<span class="cell-duration">{formatTrackDuration(track.duration_ms)}</span>
		<div class="cell-actions">
			<button
				class="row-btn"
				aria-label="Play {track.title}"
				title={playableLabel}
				disabled={!playable}
				onclick={handlePlay}
			>▶</button>
			<button
				class="row-btn"
				aria-label="Add to queue"
				title={playable ? 'Add to queue' : playableLabel}
				disabled={!playable}
				onclick={handleAddToQueue}
			>＋</button>
			<button
				class="row-btn"
				aria-label="Start song radio"
				title="Start song radio"
				onclick={handleSongRadio}
			>◎</button>
			<button
				class="row-btn"
				aria-label="More actions"
				title="More actions"
				onclick={handleMoreClick}
			>⋯</button>
			<button
				class="row-btn heart"
				class:on={isFavorite}
				aria-label={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
				title={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
				disabled={favoritePending}
				onclick={handleHeart}
			>{isFavorite ? '♥' : '♡'}</button>
		</div>
	</li>
{:else if variant === 'indexed'}
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row indexed"
		class:active={isCurrent}
		class:disabled={!playable}
		role="button"
		tabindex={playable ? 0 : -1}
		aria-disabled={!playable}
		onclick={handleRowClick}
		ondblclick={() => playable && void playTidalTrackNow(track)}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		<span class="cell-num">
			{#if isCurrent && isPlaying}
				<span class="eq" aria-hidden="true"><span></span><span></span><span></span></span>
			{:else}
				<span class="num">{track.track_number ?? index + 1}</span>
				<span class="play-hover" aria-hidden="true">▶</span>
			{/if}
		</span>
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			{#if showArtist && track.artist_name}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<span class="sub" oncontextmenu={openArtistContextMenu}>{track.artist_name}</span>
			{/if}
		</div>
		<span class="cell-duration">{formatTrackDuration(track.duration_ms)}</span>
		<div class="cell-actions">
			<button
				class="row-btn"
				aria-label="Play {track.title}"
				title={playableLabel}
				disabled={!playable}
				onclick={handlePlay}
			>▶</button>
			<button
				class="row-btn"
				aria-label="Add to queue"
				title={playable ? 'Add to queue' : playableLabel}
				disabled={!playable}
				onclick={handleAddToQueue}
			>＋</button>
			<button
				class="row-btn"
				aria-label="Start song radio"
				title="Start song radio"
				onclick={handleSongRadio}
			>◎</button>
			<button
				class="row-btn"
				aria-label="More actions"
				title="More actions"
				onclick={handleMoreClick}
			>⋯</button>
			<button
				class="row-btn heart"
				class:on={isFavorite}
				aria-label={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
				title={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
				disabled={favoritePending}
				onclick={handleHeart}
			>{isFavorite ? '♥' : '♡'}</button>
		</div>
	</li>
{:else if variant === 'art'}
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row art"
		class:active={isCurrent}
		class:disabled={!playable}
		role="button"
		tabindex={playable ? 0 : -1}
		aria-disabled={!playable}
		onclick={handleRowClick}
		ondblclick={() => playable && void playTidalTrackNow(track)}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		{#if track.artwork_url}
			<ArtworkImage
				className="cell-art-thumb"
				src={track.artwork_url}
				alt={track.title}
				size={320}
				fallbackText={track.title.slice(0, 2).toUpperCase()}
			/>
		{:else}
			<div class="cell-art-thumb placeholder"><span>♫</span></div>
		{/if}
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			<p class="sub">
				{#if showArtist && track.artist_name}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span class="sub-link" oncontextmenu={openArtistContextMenu}>{track.artist_name}</span>
				{/if}
				{#if showArtist && track.artist_name && showAlbum && track.album_title} - {/if}
				{#if showAlbum && track.album_title}
					<!-- svelte-ignore a11y_no_static_element_interactions -->
					<span class="sub-link" oncontextmenu={openAlbumContextMenu}>{track.album_title}</span>
				{/if}
			</p>
		</div>
		<span class="cell-duration">{formatTrackDuration(track.duration_ms)}</span>
		<div class="cell-actions">
			<button
				class="row-btn"
				aria-label="Play {track.title}"
				title={playableLabel}
				disabled={!playable}
				onclick={handlePlay}
			>▶</button>
			<button
				class="row-btn"
				aria-label="Add to queue"
				title={playable ? 'Add to queue' : playableLabel}
				disabled={!playable}
				onclick={handleAddToQueue}
			>＋</button>
			<button
				class="row-btn"
				aria-label="Start song radio"
				title="Start song radio"
				onclick={handleSongRadio}
			>◎</button>
			<button
				class="row-btn"
				aria-label="More actions"
				title="More actions"
				onclick={handleMoreClick}
			>⋯</button>
			<button
				class="row-btn heart"
				class:on={isFavorite}
				aria-label={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
				title={isFavorite ? 'Remove from favourites' : 'Add to favourites'}
				disabled={favoritePending}
				onclick={handleHeart}
			>{isFavorite ? '♥' : '♡'}</button>
		</div>
	</li>
{:else}
	<!-- compact -->
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row compact"
		class:active={isCurrent}
		class:disabled={!playable}
		role="button"
		tabindex={playable ? 0 : -1}
		aria-disabled={!playable}
		onclick={handleRowClick}
		ondblclick={() => playable && void playTidalTrackNow(track)}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			{#if showArtist && track.artist_name}
				<!-- svelte-ignore a11y_no_static_element_interactions -->
				<span class="sub" oncontextmenu={openArtistContextMenu}>{track.artist_name}</span>
			{/if}
		</div>
		<span class="cell-duration">{formatTrackDuration(track.duration_ms)}</span>
	</li>
{/if}

<style>
	.track-row {
		display: grid;
		align-items: center;
		gap: 14px;
		padding: 10px 16px;
		border-radius: 6px;
		cursor: pointer;
		transition: background var(--motion-fast);
	}

	.track-row:hover { background: var(--bg-hover); }
	.track-row.active { background: var(--accent-soft); }
	.track-row.active .title { color: var(--accent-strong); }
	.track-row.disabled {
		cursor: default;
		opacity: 0.62;
	}

	.track-row.numbered {
		grid-template-columns: 32px 42px 1fr 60px auto;
		gap: 14px;
		padding: 8px 12px;
	}

	.track-row.numbered .cell-index {
		color: var(--text-secondary);
		text-align: center;
		font-variant-numeric: tabular-nums;
	}

	.track-row.numbered .cell-art {
		width: 42px;
		height: 42px;
		border-radius: 4px;
		overflow: hidden;
		background: var(--bg-surface);
	}

	.track-row.numbered .art {
		width: 100%;
		height: 100%;
		object-fit: cover;
		display: block;
	}
	.track-row.numbered .art.placeholder {
		display: grid;
		place-items: center;
		color: var(--text-tertiary);
	}

	.track-row.indexed {
		grid-template-columns: 40px 1fr 64px auto;
	}

	.track-row.indexed .cell-num {
		position: relative;
		display: grid;
		place-items: center;
		width: 40px;
		color: var(--text-secondary);
		font-variant-numeric: tabular-nums;
		font-size: var(--font-size-sm);
	}

	.track-row.indexed .cell-num .play-hover {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		opacity: 0;
		color: var(--text-primary);
		font-size: var(--font-size-sm);
	}

	.track-row.indexed:hover .cell-num .num { opacity: 0; }
	.track-row.indexed:hover .cell-num .play-hover { opacity: 1; }

	.eq {
		display: inline-flex;
		align-items: flex-end;
		gap: 2px;
		height: 14px;
	}
	.eq span {
		width: 3px;
		background: var(--accent-strong);
		animation: eq-bounce 0.9s infinite ease-in-out;
		border-radius: 2px;
	}
	.eq span:nth-child(1) { animation-delay: -0.3s; }
	.eq span:nth-child(2) { animation-delay: -0.15s; }
	.eq span:nth-child(3) { animation-delay: 0s; }
	@keyframes eq-bounce {
		0%, 100% { height: 4px; }
		50% { height: 14px; }
	}

	.track-row.art {
		grid-template-columns: 38px 1fr auto auto;
		gap: 12px;
		padding: 8px 8px;
	}

	.cell-art-thumb {
		width: 36px;
		height: 36px;
		border-radius: 4px;
		background: var(--bg-raised);
		background-size: cover;
		background-position: center;
		flex-shrink: 0;
		display: flex;
		align-items: center;
		justify-content: center;
	}
	.cell-art-thumb.placeholder span {
		font-size: var(--font-size-md);
		color: rgba(255, 255, 255, 0.5);
	}

	.track-row.compact {
		grid-template-columns: 1fr auto;
		gap: 8px;
		padding: 6px 10px;
	}

	.cell-meta {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.title {
		margin: 0;
		font-weight: var(--font-weight-semibold);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.sub {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100%;
		margin: 0;
	}

	.sub-link {
		text-decoration: none;
	}

	.sub-link:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.cell-duration {
		color: var(--text-secondary);
		font-size: var(--font-size-sm);
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	.cell-actions {
		display: flex;
		align-items: center;
		gap: 4px;
		opacity: 0;
		transition: opacity var(--motion-fast);
	}

	.track-row:hover .cell-actions,
	.track-row:focus-within .cell-actions,
	.track-row.active .cell-actions { opacity: 1; }

	.row-btn {
		all: unset;
		width: 30px;
		height: 30px;
		display: grid;
		place-items: center;
		border-radius: 999px;
		cursor: pointer;
		color: var(--text-secondary);
		font-size: var(--font-size-md);
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.row-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}
	.row-btn:disabled {
		cursor: not-allowed;
		opacity: 0.45;
	}
	.row-btn:disabled:hover {
		background: transparent;
		color: var(--text-secondary);
	}
	.row-btn.heart.on { color: var(--accent); }
</style>
