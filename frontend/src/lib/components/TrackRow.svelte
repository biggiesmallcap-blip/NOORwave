<script lang="ts">
	import { formatDuration } from '$lib/stores/library';
	import { openContextMenu, openMenuAtElement } from '$lib/stores/context_menu';
	import {
		buildTrackMenu,
		type BuildTrackMenuOptions,
		type MenuTrack
	} from '$lib/player/track_menu';
	import {
		addTrackToQueue,
		startSongRadio,
		toggleTrackFavorite
	} from '$lib/stores/player';

	export type TrackRowVariant = 'numbered' | 'indexed' | 'art' | 'compact';

	interface TrackInput extends MenuTrack {
		duration_ms: number | null;
		play_count?: number;
		track_number?: number | null;
		artwork_url?: string | null;
	}

	let {
		track,
		variant,
		isCurrent,
		isPlaying,
		index = 0,
		showAlbum = true,
		showArtist = true,
		showPlayCount = false,
		onRowClick,
		menuOptions,
		selected = false,
		onSelect
	}: {
		track: TrackInput;
		variant: TrackRowVariant;
		isCurrent: boolean;
		isPlaying: boolean;
		index?: number;
		showAlbum?: boolean;
		showArtist?: boolean;
		showPlayCount?: boolean;
		onRowClick: () => void;
		menuOptions?: BuildTrackMenuOptions;
		selected?: boolean;
		onSelect?: (e: MouseEvent | KeyboardEvent) => void;
	} = $props();

	function handleKeyDown(e: KeyboardEvent) {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			onRowClick();
		}
	}

	function handleContextMenu(e: MouseEvent) {
		e.preventDefault();
		openContextMenu(e, buildTrackMenu(track, menuOptions), track.title);
	}

	function handleMoreClick(e: MouseEvent) {
		e.stopPropagation();
		openMenuAtElement(
			e.currentTarget as HTMLElement,
			buildTrackMenu(track, menuOptions),
			track.title
		);
	}

	function handleAddToQueue(e: MouseEvent) {
		e.stopPropagation();
		void addTrackToQueue(track.id);
	}

	function handleSongRadio(e: MouseEvent) {
		e.stopPropagation();
		void startSongRadio(track.id);
	}

	async function handleHeart(e: MouseEvent) {
		e.stopPropagation();
		try {
			await toggleTrackFavorite(track.id, track.is_favorite ?? false);
		} catch {
			// player store already surfaces the error; nothing else to do
		}
	}
</script>

{#if variant === 'numbered'}
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row numbered"
		class:active={isCurrent}
		class:selected
		role="button"
		tabindex="0"
		onclick={(e) => (onSelect ? onSelect(e) : onRowClick())}
		ondblclick={() => onRowClick()}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		<span class="cell-index">{index + 1}</span>
		<div class="cell-art">
			{#if track.artwork_url}
				<img class="art" src={track.artwork_url} alt="" />
			{:else}
				<div class="art placeholder">♫</div>
			{/if}
		</div>
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			{#if showArtist && track.artist_name}
				<span class="sub">{track.artist_name}</span>
			{/if}
		</div>
		{#if showPlayCount}
			<span class="cell-plays">{(track.play_count ?? 0).toLocaleString()}</span>
		{/if}
		<div class="cell-actions">
			<button
				class="row-btn"
				aria-label="Add to queue"
				title="Add to queue"
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
				class:on={track.is_favorite}
				aria-label={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
				title={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
				onclick={handleHeart}
			>{track.is_favorite ? '♥' : '♡'}</button>
		</div>
		<span class="cell-duration">{formatDuration(track.duration_ms)}</span>
	</li>
{:else if variant === 'indexed'}
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row indexed"
		class:active={isCurrent}
		class:selected
		role="button"
		tabindex="0"
		onclick={(e) => (onSelect ? onSelect(e) : onRowClick())}
		ondblclick={() => onRowClick()}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		<span class="cell-num">
			{#if isCurrent}
				<span class="play-indicator" aria-hidden="true">
					{#if isPlaying}
						<span class="eq"><span></span><span></span><span></span></span>
					{:else}
						▶
					{/if}
				</span>
			{:else}
				<span class="num">{track.track_number ?? index + 1}</span>
				<span class="play-hover" aria-hidden="true">▶</span>
			{/if}
		</span>
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			{#if showArtist && track.artist_name}
				{#if track.artist_id != null}
					<a
						class="sub link"
						href="/artists/{track.artist_id}"
						onclick={(e) => e.stopPropagation()}
					>{track.artist_name}</a>
				{:else}
					<span class="sub">{track.artist_name}</span>
				{/if}
			{/if}
		</div>
		{#if showPlayCount}
			<span class="cell-plays">{(track.play_count ?? 0).toLocaleString()}</span>
		{/if}
		<div class="cell-actions">
			<button
				class="row-btn"
				aria-label="Add to queue"
				title="Add to queue"
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
				class:on={track.is_favorite}
				aria-label={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
				title={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
				onclick={handleHeart}
			>{track.is_favorite ? '♥' : '♡'}</button>
		</div>
		<span class="cell-duration">{formatDuration(track.duration_ms)}</span>
	</li>
{:else if variant === 'art'}
	<!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
	<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
	<li
		class="track-row art"
		class:active={isCurrent}
		class:selected
		role="button"
		tabindex="0"
		onclick={(e) => (onSelect ? onSelect(e) : onRowClick())}
		ondblclick={() => onRowClick()}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		{#if track.artwork_url}
			<div class="cell-art-thumb" style="background-image:url('{track.artwork_url}')"></div>
		{:else}
			<div class="cell-art-thumb placeholder"><span>♫</span></div>
		{/if}
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			<p class="sub">
				{#if showArtist && track.artist_name}{track.artist_name}{/if}
				{#if showArtist && track.artist_name && showAlbum && track.album_title} — {/if}
				{#if showAlbum && track.album_title}{track.album_title}{/if}
			</p>
		</div>
		<span class="cell-duration">{formatDuration(track.duration_ms)}</span>
		<div class="cell-actions">
			<button
				class="row-btn"
				aria-label="Add to queue"
				title="Add to queue"
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
				class:on={track.is_favorite}
				aria-label={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
				title={track.is_favorite ? 'Remove from favourites' : 'Add to favourites'}
				onclick={handleHeart}
			>{track.is_favorite ? '♥' : '♡'}</button>
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
		class:selected
		role="button"
		tabindex="0"
		onclick={(e) => (onSelect ? onSelect(e) : onRowClick())}
		ondblclick={() => onRowClick()}
		onkeydown={handleKeyDown}
		oncontextmenu={handleContextMenu}
	>
		<div class="cell-meta">
			<p class="title">{track.title}</p>
			{#if showArtist && track.artist_name}
				<span class="sub">{track.artist_name}</span>
			{/if}
		</div>
		<span class="cell-duration">{formatDuration(track.duration_ms)}</span>
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
	.track-row.selected { background: var(--accent-soft); }

	/* ── numbered (artist popular list) ─────────────────────────── */
	.track-row.numbered {
		grid-template-columns: 32px 42px 1fr 90px auto 60px;
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

	/* ── indexed (album track list) ──────────────────────────────── */
	.track-row.indexed {
		grid-template-columns: 40px 1fr 80px auto 64px;
	}

	.track-row.indexed .cell-num {
		position: relative;
		display: grid;
		place-items: center;
		width: 40px;
		color: var(--text-secondary);
		font-variant-numeric: tabular-nums;
		font-size: 0.9rem;
	}

	.track-row.indexed .cell-num .play-hover {
		position: absolute;
		inset: 0;
		display: grid;
		place-items: center;
		opacity: 0;
		color: var(--text-primary);
		font-size: 0.8rem;
	}

	.track-row.indexed:hover .cell-num .num { opacity: 0; }
	.track-row.indexed:hover .cell-num .play-hover { opacity: 1; }

	.play-indicator {
		color: var(--accent-strong);
		display: grid;
		place-items: center;
	}

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

	/* ── art (library / search local rows) ───────────────────────── */
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
		font-size: 16px;
		color: rgba(255, 255, 255, 0.5);
	}

	/* ── compact (analytics / home / automix) ────────────────────── */
	.track-row.compact {
		grid-template-columns: 1fr auto;
		gap: 8px;
		padding: 6px 10px;
	}

	/* ── shared cells ────────────────────────────────────────────── */
	.cell-meta {
		min-width: 0;
		display: flex;
		flex-direction: column;
		gap: 2px;
	}

	.title {
		margin: 0;
		font-weight: 600;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
	}

	.sub {
		color: var(--text-secondary);
		font-size: 0.82rem;
		text-decoration: none;
		white-space: nowrap;
		overflow: hidden;
		text-overflow: ellipsis;
		max-width: 100%;
	}

	.sub.link:hover {
		color: var(--text-primary);
		text-decoration: underline;
	}

	.cell-plays {
		color: var(--text-tertiary);
		font-size: 0.82rem;
		text-align: right;
		font-variant-numeric: tabular-nums;
	}

	.cell-duration {
		color: var(--text-secondary);
		font-size: 0.82rem;
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
		font-size: 1rem;
		transition: background var(--motion-fast), color var(--motion-fast);
	}

	.row-btn:hover {
		background: var(--bg-hover);
		color: var(--text-primary);
	}

	.row-btn.heart.on { color: var(--accent); }
</style>
