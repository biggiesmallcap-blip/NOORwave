<script lang="ts">
  import { onMount } from 'svelte'
  import { goto, beforeNavigate } from '$app/navigation'
  import type { Snapshot } from './$types'
  import { api, type TidalSearchResults, type TidalSearchAlbum, type TidalSearchArtist, type TidalSearchTrack, type AudioSearchResult, type AudioSearchParams, type Genre, type VibeTrack, type BasicTrack, type Playlist, type TidalSearchPlaylist, type ChartEntry, type TrendingSource } from '$lib/api/client'
  import TrackRow from '$lib/components/TrackRow.svelte'
  import TidalTrackRow from '$lib/components/TidalTrackRow.svelte'
  import { buildTidalTrackMenu, buildTrackMenu } from '$lib/player/track_menu'
  import { openContextMenu, type MenuItem } from '$lib/stores/context_menu'
  import { playTidalTrackNow, playTidalAlbum, playTidalTrackNext, addTidalTrackToQueue, startTidalSongRadio, playTrackNow, startArtistRadio, startAlbumRadio, shuffleAlbum, playTidalPlaylist } from '$lib/stores/player'
  import { formatDuration } from '$lib/stores/library'
  import { parseQuery, filtersToChips, type ParsedQuery } from '$lib/search/query_parser'
  import { buildAudioParams as sharedBuildAudioParams } from '$lib/search/audio_params'
  import { parseIntent } from '$lib/search/intent'
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal'
  import { tidalSearchTrackToPlayable } from '$lib/utils/track'

  const RECENT_KEY = 'noor_recent_searches'
  const RECENT_MAX = 8

  function loadRecent(): string[] {
    if (typeof localStorage === 'undefined') return []
    try {
      const raw = localStorage.getItem(RECENT_KEY)
      if (!raw) return []
      const parsed = JSON.parse(raw)
      return Array.isArray(parsed) ? parsed.filter((v) => typeof v === 'string').slice(0, RECENT_MAX) : []
    } catch {
      return []
    }
  }

  function pushRecent(q: string) {
    if (!q.trim() || typeof localStorage === 'undefined') return
    const next = [q, ...recent.filter((r) => r.toLowerCase() !== q.toLowerCase())].slice(0, RECENT_MAX)
    recent = next
    localStorage.setItem(RECENT_KEY, JSON.stringify(next))
  }

  function clearRecent() {
    recent = []
    if (typeof localStorage !== 'undefined') localStorage.removeItem(RECENT_KEY)
  }

  let query = $state('')
  let results = $state<TidalSearchResults | null>(null)
  let audioResults = $state<AudioSearchResult[] | null>(null)
  let loading = $state(false)
  let error = $state<string | null>(null)
  let failedArtistImages = $state(new Set<number>())
  let topArtistImageFailed = $state(false)
  $effect(() => {
    // reset image error state when search results change
    // eslint-disable-next-line @typescript-eslint/no-unused-expressions
    results
    failedArtistImages = new Set()
    topArtistImageFailed = false
  })
  let debounceTimer: ReturnType<typeof setTimeout>
  // AbortController for in-flight search requests; cancelled on each new input
  // and on route teardown so a slow query doesn't surface a phantom error after
  // the user navigates away.
  let abortController: AbortController | null = null

  // D5 — in-memory result cache (last 5 queries, keyed by normalised query string)
  const resultCache = new Map<string, TidalSearchResults>()
  let recent = $state<string[]>(loadRecent())
  let genreList = $state<Genre[]>([])

  // C3 — session-aware ranking
  let recentArtistNames = $state<Set<string>>(new Set())

  // C4 — discovery injectables
  let vibeTrack = $state<VibeTrack[] | null>(null)
  let underratedTracks = $state<BasicTrack[] | null>(null)

  let localPlaylists = $state<Playlist[]>([])
  let tidalPlaylistResults = $state<TidalSearchPlaylist[]>([])

  type FilterMode = 'all' | 'artists' | 'albums' | 'tracks' | 'library' | 'playlists'
  let filterMode = $state<FilterMode>('all')

  // Phase 5 — Trending shelf (shown when query is empty)
  const TRENDING_SOURCE_KEY = 'noor.trending.source'
  function loadTrendingSource(): TrendingSource {
    if (typeof localStorage === 'undefined') return 'lastfm'
    const v = localStorage.getItem(TRENDING_SOURCE_KEY)
    return v === 'tidal' ? 'tidal' : 'lastfm'
  }
  let trending = $state<ChartEntry[]>([])
  let trendingSource = $state<TrendingSource>(loadTrendingSource())
  let trendingLoading = $state(false)
  async function loadTrending() {
    trendingLoading = true
    try {
      const data = await api.getTrending({ source: trendingSource, limit: 25 })
      trending = data.tracks ?? []
    } catch {
      trending = []
    } finally {
      trendingLoading = false
    }
  }
  function setTrendingSource(s: TrendingSource) {
    if (s === trendingSource) return
    trendingSource = s
    if (typeof localStorage !== 'undefined') {
      try { localStorage.setItem(TRENDING_SOURCE_KEY, s) } catch { /* ignore */ }
    }
    void loadTrending()
  }

  const parsedQuery = $derived(parseQuery(query))
  const hasFilters = $derived(Object.keys(parsedQuery.filters).length > 0)

  onMount(async () => {
    try {
      const res = await api.getGenres()
      genreList = res.genres
    } catch { /* ignore */ }
    try {
      const listens = await api.getRecentListens(20)
      const names = listens.listens
        .map(e => e.artist_name)
        .filter((n): n is string => typeof n === 'string' && n.length > 0)
      recentArtistNames = new Set(names)
    } catch { /* ignore */ }
    try {
      const { playlists } = await api.getPlaylists()
      localPlaylists = playlists
    } catch { /* ignore */ }
    void loadTrending()
  })

  function buildAudioParams(pq: ParsedQuery): AudioSearchParams {
    return sharedBuildAudioParams(pq, genreList)
  }

  function removeFilter(key: string) {
    // Rebuild query string by stripping tokens that start with key: or key>/</>=/<=
    const tokens = query.trim().split(/\s+/).filter(t => t.length > 0)
    const filtered = tokens.filter(t => {
      // match key: or key>/</>=/<= prefix
      return !t.match(new RegExp(`^${key}(:|>=|<=|>|<)`))
    })
    query = filtered.join(' ')
    onInput()
  }

  async function playLibraryTrack(track: AudioSearchResult) {
    await playTrackNow(track.id)
  }

  function onInput() {
    clearTimeout(debounceTimer)
    // Cancel any prior in-flight request so its rejection doesn't fire as an error.
    abortController?.abort()
    abortController = null
    if (!query.trim()) {
      results = null
      audioResults = null
      tidalPlaylistResults = []
      loading = false
      error = null
      return
    }
    loading = true
    debounceTimer = setTimeout(async () => {
      const q = query.trim()
      const intent = parseIntent(q)
      const controller = new AbortController()
      abortController = controller
      const signal = controller.signal

      // "play <query>" → fire immediately and clear input
      if (intent.intent.type === 'play') {
        loading = false
        const r = await api.searchTidal(intent.free_text, 20, signal).catch(() => null)
        const first = r?.tracks[0]
        if (first) void playTidalTrackNow(toPlayable(first))
        query = ''
        results = null
        return
      }

      // "similar to <query>" → start radio from first result
      if (intent.intent.type === 'radio') {
        loading = false
        const r = await api.searchTidal(intent.free_text, 20, signal).catch(() => null)
        const first = r?.tracks[0]
        if (first) void startTidalSongRadio(toPlayable(first))
        query = ''
        results = null
        return
      }

      // "<title/artist> <year>" → merge year into filters and run audio search
      const effectiveParsed: ParsedQuery = intent.intent.type === 'year_filter'
        ? { free_text: intent.free_text, filters: intent.extra_filters }
        : parsedQuery
      const effectiveHasFilters = Object.keys(effectiveParsed.filters).length > 0

      try {
        if (effectiveHasFilters) {
          const res = await api.searchAudio(buildAudioParams(effectiveParsed), signal)
          audioResults = res.tracks
          results = null
          tidalPlaylistResults = []
        } else {
          audioResults = null
          const cacheKey = q.toLowerCase()
          const cached = resultCache.get(cacheKey)
          if (cached) {
            results = cached
          } else {
            const fresh = await api.searchTidal(q, 20, signal)
            results = fresh
            resultCache.set(cacheKey, fresh)
            if (resultCache.size > 5) resultCache.delete(resultCache.keys().next().value!)
          }
          try {
            const { playlists } = await api.searchTidalPlaylists(q, signal)
            tidalPlaylistResults = playlists
          } catch {
            tidalPlaylistResults = []
          }
        }
        error = null
        pushRecent(q)
      } catch (e) {
        // Swallow abort errors — the user moved on or typed more.
        if (signal.aborted || (e as Error)?.name === 'AbortError') return
        error = String(e)
      } finally {
        if (abortController === controller) abortController = null
        if (!signal.aborted) loading = false
      }
    }, 300)
  }

  function pickRecent(q: string) {
    query = q
    onInput()
    inputEl?.focus()
  }

  const isEmpty = $derived(
    results !== null &&
    results.tracks.length === 0 &&
    results.albums.length === 0 &&
    results.artists.length === 0
  )

  // Library entries float to the top of each section. Stable sort preserves
  // Tidal's order within each group so the user sees expected ranking otherwise.
  // Filter pills then narrow the visible set without re-querying.
  function applyFilter<T extends { in_library: boolean }>(items: T[], showType: boolean): T[] {
    if (!showType) return []
    if (filterMode === 'library') return items.filter((i) => i.in_library)
    return items
  }
  const sortedArtists = $derived(
    applyFilter(
      results ? [...results.artists].sort((a, b) => Number(b.in_library) - Number(a.in_library)) : [],
      filterMode === 'all' || filterMode === 'artists' || filterMode === 'library'
    )
  )
  const sortedAlbums = $derived(
    applyFilter(
      results ? [...results.albums].sort((a, b) => Number(b.in_library) - Number(a.in_library)) : [],
      filterMode === 'all' || filterMode === 'albums' || filterMode === 'library'
    )
  )
  const sortedTracks = $derived(
    applyFilter(
      results
        ? [...results.tracks].sort((a, b) => {
            const libDiff = Number(b.in_library) - Number(a.in_library)
            if (libDiff !== 0) return libDiff
            // C3: within the in_library group, boost tracks from recently played artists
            const aRecent = a.in_library && a.artist_name != null && recentArtistNames.has(a.artist_name) ? 1 : 0
            const bRecent = b.in_library && b.artist_name != null && recentArtistNames.has(b.artist_name) ? 1 : 0
            return bRecent - aRecent
          })
        : [],
      filterMode === 'all' || filterMode === 'tracks' || filterMode === 'library'
    )
  )
  const showPlaylists = $derived(filterMode === 'all' || filterMode === 'playlists')

  const filteredPlaylists = $derived.by(() => {
    if (!query.trim()) return { local: [] as Playlist[], tidal: [] as TidalSearchPlaylist[] }
    const q = query.trim().toLowerCase()
    const matched = localPlaylists.filter(p => p.name.toLowerCase().includes(q))
    const localNames = new Set(matched.map(p => p.name.toLowerCase()))
    const tidalOnly = tidalPlaylistResults.filter(tp => !localNames.has(tp.title.toLowerCase()))
    return { local: matched, tidal: tidalOnly }
  })

  const isFilteredEmpty = $derived(
    results !== null &&
    !isEmpty &&
    sortedTracks.length === 0 &&
    sortedAlbums.length === 0 &&
    sortedArtists.length === 0 &&
    !(showPlaylists && (filteredPlaylists.local.length > 0 || filteredPlaylists.tidal.length > 0))
  )

  type TopResult =
    | { kind: 'artist'; entry: TidalSearchArtist }
    | { kind: 'album'; entry: TidalSearchAlbum }
    | { kind: 'track'; entry: TidalSearchTrack }

  // Score each candidate, prefer artist > album > track on ties, exact-name match
  // wins, library entries get a +0.3 boost. The first place across all three
  // sections is the hero.
  const topResult = $derived.by<TopResult | null>(() => {
    if (!results || !query.trim()) return null
    const q = query.trim().toLowerCase()
    const score = (name: string, inLibrary: boolean, kindBias: number) => {
      const n = name.toLowerCase()
      let s = 0
      if (n === q) s += 1.0
      else if (n.startsWith(q)) s += 0.6
      else if (n.includes(q)) s += 0.3
      if (inLibrary) s += 0.3
      return s + kindBias
    }
    const candidates: { tr: TopResult; s: number }[] = []
    if (sortedArtists[0]) candidates.push({ tr: { kind: 'artist', entry: sortedArtists[0] }, s: score(sortedArtists[0].name, sortedArtists[0].in_library, 0.05) })
    if (sortedAlbums[0]) candidates.push({ tr: { kind: 'album', entry: sortedAlbums[0] }, s: score(sortedAlbums[0].title, sortedAlbums[0].in_library, 0.025) })
    if (sortedTracks[0]) candidates.push({ tr: { kind: 'track', entry: sortedTracks[0] }, s: score(sortedTracks[0].title, sortedTracks[0].in_library, 0) })
    if (candidates.length === 0) return null
    candidates.sort((a, b) => b.s - a.s)
    return candidates[0].tr
  })

  function topResultHref(top: TopResult): string {
    switch (top.kind) {
      case 'artist':
        return top.entry.in_library && top.entry.local_id != null
          ? `/artists/${top.entry.local_id}`
          : `/tidal/artists/${top.entry.tidal_id}`
      case 'album':
        return top.entry.in_library && top.entry.local_id != null
          ? `/albums/${top.entry.local_id}`
          : `/tidal/albums/${top.entry.tidal_id}`
      case 'track':
        return top.entry.artist_id != null
          ? `/tidal/artists/${top.entry.artist_id}`
          : '#'
    }
  }

  function topResultPlay(top: TopResult) {
    if (top.kind === 'track') {
      void playTidalTrackNow(toPlayable(top.entry))
    } else if (top.kind === 'album') {
      void playTidalAlbum(top.entry.tidal_id)
    } else {
      void goto(topResultHref(top))
    }
  }

  // Stable per-name color so empty-artwork avatars feel intentional, not broken.
  function letterColor(name: string): string {
    let hash = 0
    for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) | 0
    const hue = Math.abs(hash) % 360
    return `hsl(${hue}, 38%, 28%)`
  }

  function initials(name: string): string {
    const parts = name.trim().split(/\s+/).slice(0, 2)
    return parts.map((p) => p[0]?.toUpperCase() ?? '').join('') || '?'
  }

  function buildAlbumMenu(album: TidalSearchAlbum): MenuItem[] {
    const items: MenuItem[] = [
      { label: 'Play album', icon: '▶', onSelect: () => void playTidalAlbum(album.tidal_id) },
    ]
    if (album.in_library && album.local_id != null) {
      items.push({ label: 'Shuffle album', icon: '⤮', onSelect: () => void shuffleAlbum(album.local_id!) })
      items.push({ label: 'Album radio', icon: '◉', onSelect: () => void startAlbumRadio(album.local_id!) })
      items.push({ separator: true, label: '' })
      items.push({ label: 'Open in library', icon: '→', onSelect: () => void goto(`/albums/${album.local_id}`) })
    } else {
      items.push({ separator: true, label: '' })
      items.push({ label: 'Open on Tidal', icon: '→', onSelect: () => void goto(`/tidal/albums/${album.tidal_id}`) })
    }
    return items
  }

  function buildArtistMenu(artist: TidalSearchArtist): MenuItem[] {
    const href = artist.in_library && artist.local_id != null
      ? `/artists/${artist.local_id}`
      : `/tidal/artists/${artist.tidal_id}`
    const items: MenuItem[] = [
      { label: artist.in_library ? 'Open in library' : 'Open artist', icon: '→', onSelect: () => void goto(href) },
    ]
    if (artist.in_library && artist.local_id != null) {
      items.push({ separator: true, label: '' })
      items.push({ label: 'Artist radio', icon: '◉', onSelect: () => void startArtistRadio(artist.local_id!) })
    }
    return items
  }

  function trackContextMenu(track: TidalSearchTrack): MenuItem[] {
    if (track.in_library && track.local_id != null) {
      return buildTrackMenu({
        id: track.local_id,
        title: track.title,
        artist_id: null,
        artist_name: track.artist_name ?? null,
        album_id: null,
        album_title: track.album_title ?? null,
      })
    }
    return buildTidalTrackMenu(toPlayable(track))
  }

  // ─── Keyboard navigation ────────────────────────────────────────────────
  // `/` from anywhere on the page focuses the input (skips when typing
  // elsewhere). Once in the input, `Enter` plays — modifiers branch to
  // queue/play-next. Arrow keys move a row cursor through the visible tracks.
  let inputEl: HTMLInputElement | null = $state(null)
  let cursor = $state(-1)
  let inputFocused = $state(false)

  // DSP filter syntax is invisible — surface a tiny set of example chips when
  // the input is focused but empty so users discover bpm:/key:/energy: filters.
  const HINT_CHIPS: { token: string; label: string }[] = [
    { token: 'bpm:128 ', label: 'bpm:128' },
    { token: 'key:Am ', label: 'key:Am' },
    { token: 'energy:>0.7 ', label: 'energy:>0.7' },
  ]

  function insertHintChip(token: string) {
    query = `${query}${query.endsWith(' ') || query.length === 0 ? '' : ' '}${token}`
    inputEl?.focus()
    onInput()
  }

  // Inline prefix completion: if the trailing token matches a known filter
  // prefix (e.g. "bp", "ke", "en", "ge"), suggest the full filter key.
  const FILTER_PREFIXES: Record<string, string> = {
    bp: 'bpm:',
    bpm: 'bpm:',
    ke: 'key:',
    key: 'key:',
    en: 'energy:',
    energy: 'energy:',
    ge: 'genre:',
    genre: 'genre:',
    da: 'danceability:',
    danceability: 'danceability:',
  }

  const inlineHint = $derived.by<string | null>(() => {
    if (!query) return null
    if (query.endsWith(' ')) return null
    if (query.includes(':')) {
      const tail = query.slice(query.lastIndexOf(' ') + 1)
      if (tail.includes(':')) return null
    }
    const tail = query.slice(query.lastIndexOf(' ') + 1).toLowerCase()
    if (!tail) return null
    const completion = FILTER_PREFIXES[tail]
    if (!completion) return null
    return completion
  })

  function applyInlineHint() {
    if (!inlineHint) return
    const idx = query.lastIndexOf(' ')
    const head = idx === -1 ? '' : query.slice(0, idx + 1)
    query = `${head}${inlineHint}`
    inputEl?.focus()
  }

  // Reset cursor whenever the result set changes shape so we never point past the end.
  $effect(() => {
    if (cursor >= sortedTracks.length) cursor = sortedTracks.length - 1
  })

  const toPlayable = tidalSearchTrackToPlayable;

  function actOnTrack(track: TidalSearchTrack, mode: 'play' | 'queue' | 'next') {
    if (mode === 'queue') void addTidalTrackToQueue(toPlayable(track))
    else if (mode === 'next') void playTidalTrackNext(toPlayable(track))
    else void playTidalTrackNow(toPlayable(track))
  }

  function inputKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      if (query) {
        query = ''
        results = null
        cursor = -1
      } else {
        inputEl?.blur()
      }
      return
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault()
      if (sortedTracks.length === 0) return
      cursor = cursor < 0 ? 0 : Math.min(cursor + 1, sortedTracks.length - 1)
      return
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault()
      if (sortedTracks.length === 0) return
      cursor = cursor <= 0 ? -1 : cursor - 1
      return
    }
    if (e.key === 'Enter') {
      e.preventDefault()
      const mode: 'play' | 'queue' | 'next' = e.shiftKey ? 'queue' : (e.metaKey || e.ctrlKey) ? 'next' : 'play'
      const target = cursor >= 0 ? sortedTracks[cursor] : null
      if (target) {
        actOnTrack(target, mode)
      } else if (topResult) {
        if (topResult.kind === 'track') actOnTrack(topResult.entry, mode)
        else topResultPlay(topResult)
      }
      return
    }
  }

  function globalKeydown(e: KeyboardEvent) {
    if (e.key !== '/' || e.metaKey || e.ctrlKey || e.altKey) return
    const t = e.target as HTMLElement | null
    if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
    e.preventDefault()
    inputEl?.focus()
    inputEl?.select()
  }

  $effect(() => {
    window.addEventListener('keydown', globalKeydown)
    return () => window.removeEventListener('keydown', globalKeydown)
  })

  // Keep the highlighted track in view as the cursor moves.
  $effect(() => {
    if (cursor < 0) return
    const el = document.querySelector<HTMLElement>(`.track-row[data-cursor-idx="${cursor}"]`)
    el?.scrollIntoView({ block: 'nearest', behavior: 'smooth' })
  })

  // ─── Position memory (Phase 5B — SvelteKit snapshot) ─────────────────────
  // Snapshot binds state to the browser's history entry, so back AND forward
  // both land where the user left off.
  // Also abort any in-flight search on navigation so a slow response doesn't
  // surface a phantom error after we've left the page.
  beforeNavigate(() => {
    abortController?.abort()
    abortController = null
    clearTimeout(debounceTimer)
  })

  let pendingRestoreScroll: number | null = null

  // Restore scroll once results render so the layout has its final height.
  $effect(() => {
    if (results !== null && pendingRestoreScroll !== null) {
      const target = pendingRestoreScroll
      pendingRestoreScroll = null
      requestAnimationFrame(() => window.scrollTo({ top: target, behavior: 'auto' }))
    }
  })

  type SearchSnapshot = {
    query: string
    filterMode: FilterMode
    trendingSource: TrendingSource
    scrollY: number
  }
  export const snapshot: Snapshot<SearchSnapshot> = {
    capture: () => ({
      query,
      filterMode,
      trendingSource,
      scrollY: typeof window !== 'undefined' ? window.scrollY : 0
    }),
    restore: (saved) => {
      filterMode = saved.filterMode
      if (saved.trendingSource !== trendingSource) {
        trendingSource = saved.trendingSource
        void loadTrending()
      }
      if (typeof saved.query === 'string' && saved.query.trim()) {
        query = saved.query
        pendingRestoreScroll = saved.scrollY
        // Re-trigger the search; scroll restore happens once results land.
        onInput()
      } else {
        // No query — restore scroll directly on next frame.
        requestAnimationFrame(() => window.scrollTo({ top: saved.scrollY, behavior: 'auto' }))
      }
    }
  }

  // C4 — load discovery sections whenever the top result changes
  $effect(() => {
    const top = topResult
    vibeTrack = null
    underratedTracks = null
    if (!top) return
    // "Same vibe" — only when top result is a library track with a local id
    if (top.kind === 'track' && top.entry.in_library && (top.entry as TidalSearchTrack & { local_id?: number | null }).local_id != null) {
      const id = (top.entry as TidalSearchTrack & { local_id?: number | null }).local_id!
      void api.getVibeTracksForTrack(id).then(r => { vibeTrack = r.tracks }).catch(() => {})
    }
    // "Unplayed in your library" — only when top result is a library artist with a local id
    if (top.kind === 'artist' && top.entry.in_library && top.entry.local_id != null) {
      void api.getUnderratedTracksForArtist(top.entry.local_id).then(r => { underratedTracks = r.tracks }).catch(() => {})
    }
  })

</script>

<div class="search-page">
  <div class="search-header">
    <input
      bind:this={inputEl}
      class="search-input"
      type="text"
      placeholder="Search Tidal's full catalogue"
      bind:value={query}
      oninput={onInput}
      onkeydown={(e) => {
        if (e.key === 'Tab' && inlineHint && !e.shiftKey) {
          e.preventDefault()
          applyInlineHint()
          return
        }
        inputKeydown(e)
      }}
      onfocus={() => { inputFocused = true }}
      onblur={() => { inputFocused = false }}
      autofocus
    />
    {#if inputFocused && !query.trim()}
      <div class="hint-chips" aria-label="Search filter examples">
        {#each HINT_CHIPS as chip (chip.token)}
          <button
            type="button"
            class="hint-chip"
            onmousedown={(e) => e.preventDefault()}
            onclick={() => insertHintChip(chip.token)}
          >{chip.label}</button>
        {/each}
      </div>
    {/if}
    {#if inlineHint && query.trim()}
      <p class="inline-hint">
        Try <code>{inlineHint}</code> <span class="hint-tab">Tab</span>
      </p>
    {/if}
    <p class="kbd-hint">
      <kbd>/</kbd> focus &nbsp;·&nbsp;
      <kbd>↑</kbd><kbd>↓</kbd> move &nbsp;·&nbsp;
      <kbd>Enter</kbd> play &nbsp;·&nbsp;
      <kbd>Shift</kbd>+<kbd>Enter</kbd> queue &nbsp;·&nbsp;
      <kbd>Ctrl</kbd>+<kbd>Enter</kbd> next
    </p>
    {#if hasFilters}
      <div class="filter-chips">
        {#each filtersToChips(parsedQuery.filters) as chip (chip.key)}
          <button
            class="filter-chip"
            onclick={() => removeFilter(chip.key)}
            title="Remove filter"
          >{chip.display} <span class="chip-x">×</span></button>
        {/each}
      </div>
    {/if}
    {#if results && query.trim()}
      <div class="filter-pills">
        {#each [
          { id: 'all', label: 'All' },
          { id: 'artists', label: 'Artists' },
          { id: 'albums', label: 'Albums' },
          { id: 'tracks', label: 'Tracks' },
          { id: 'playlists', label: 'Playlists' },
          { id: 'library', label: 'In Library' },
        ] as pill (pill.id)}
          <button
            class="filter-pill"
            class:active={filterMode === pill.id}
            onclick={() => { filterMode = pill.id as FilterMode }}
          >{pill.label}</button>
        {/each}
      </div>
    {/if}
  </div>

  {#if !query.trim()}
    {#if recent.length > 0}
      <section class="recent-section">
        <div class="recent-head">
          <h3 class="section-label">Recent</h3>
          <button class="recent-clear" onclick={clearRecent}>Clear</button>
        </div>
        <div class="recent-chips">
          {#each recent as q (q)}
            <button class="recent-chip" onclick={() => pickRecent(q)}>{q}</button>
          {/each}
        </div>
      </section>
    {/if}

    <section class="results-section">
      <div class="trending-head">
        <h3 class="section-label">Trending</h3>
        <div class="chip-group" role="tablist" aria-label="Trending source">
          <button
            type="button"
            class="chip"
            class:active={trendingSource === 'lastfm'}
            onclick={() => setTrendingSource('lastfm')}
            role="tab"
            aria-selected={trendingSource === 'lastfm'}>Last.fm</button>
          <button
            type="button"
            class="chip"
            class:active={trendingSource === 'tidal'}
            onclick={() => setTrendingSource('tidal')}
            role="tab"
            aria-selected={trendingSource === 'tidal'}>Tidal</button>
        </div>
        {#if trendingLoading}
          <span class="trending-loading">Loading…</span>
        {/if}
      </div>
      {#if trending.length > 0}
        <div class="trending-list">
          {#each trending.slice(0, 25) as entry, i (`${i}-${entry.local_track?.id ?? entry.tidal_playable?.tidal_id ?? i}`)}
            {#if entry.local_track}
              {@const t = entry.local_track}
              <TrackRow
                track={t}
                variant="art"
                index={i}
                isCurrent={false}
                isPlaying={false}
                onRowClick={() => void playTrackNow(t.id)}
              />
            {:else if entry.tidal_playable}
              {@const tp = entry.tidal_playable}
              <TidalTrackRow
                track={tp}
                variant="art"
                index={i}
                isCurrent={false}
                isPlaying={false}
                onRowClick={() => void playTidalTrackNow(tp)}
              />
            {/if}
          {/each}
        </div>
      {:else if !trendingLoading}
        <p class="search-hint">
          {trendingSource === 'tidal'
            ? 'Tidal editorial chart unavailable. Try Last.fm.'
            : 'Last.fm chart unavailable.'}
        </p>
      {/if}
    </section>

    {#if recent.length === 0 && trending.length === 0}
      <p class="search-hint">Start typing to search Tidal's full catalogue</p>
    {/if}
  {:else if loading}
    <p class="search-hint">Searching…</p>
  {:else if error}
    <p class="search-hint search-error">{error}</p>
  {:else if isEmpty}
    <p class="search-hint">No results for "{query}"</p>
  {:else if isFilteredEmpty}
    <p class="search-hint">No {filterMode === 'library' ? 'library' : filterMode} matches for "{query}"</p>
  {:else if audioResults !== null}
    <section class="results-section">
      <h3 class="section-label">Library matches</h3>
      {#if audioResults.length === 0}
        <p class="no-audio-results">No library tracks match these filters.</p>
      {:else}
        <ul class="tracks-list">
          {#each audioResults as track (track.id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              role="button"
              tabindex="0"
              onclick={() => void playLibraryTrack(track)}
              onkeydown={(e) => e.key === 'Enter' && void playLibraryTrack(track)}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTrackMenu({ id: track.id, title: track.title, artist_name: track.artist_name, album_title: track.album_title, is_favorite: track.is_favorite })) }}
            >
              {#if track.artwork_url}
                <div class="track-art" style={`background-image: url('${track.artwork_url}')`}></div>
              {:else}
                <div class="track-art fallback" style={`background: ${letterColor(track.title)}`}>
                  <span>♫</span>
                </div>
              {/if}
              <div class="track-meta">
                <p class="track-title">{track.title}</p>
                <p class="track-subtitle">
                  {#if track.artist_name}{track.artist_name}{/if}
                  {#if track.artist_name && track.album_title} — {/if}
                  {#if track.album_title}{track.album_title}{/if}
                </p>
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
              <div class="row-actions">
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void playLibraryTrack(track) }}
                  title="Play now"
                  aria-label="Play {track.title}"
                >▶</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); openContextMenu(e, buildTrackMenu({ id: track.id, title: track.title, artist_name: track.artist_name, album_title: track.album_title, is_favorite: track.is_favorite })) }}
                  title="More options"
                  aria-label="More options"
                >⋯</button>
              </div>
            </li>
          {/each}
        </ul>
      {/if}
    </section>

  {:else if results}

    {#if topResult}
      {@const top = topResult}
      {@const artistBg = top.kind === 'artist'
        ? (top.entry.artwork_url ?? sortedAlbums.find(a => a.artist_name?.toLowerCase() === top.entry.name?.toLowerCase())?.artwork_url ?? null)
        : null}
      <section class="top-result-section">
        <h3 class="section-label">Top Result</h3>
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="top-result-card"
          class:in-library={top.entry.in_library}
          class:artist-hero={top.kind === 'artist'}
          role="button"
          tabindex="0"
          style={top.kind === 'artist' && artistBg && !topArtistImageFailed ? `background-image: url('${artistBg}'); background-size: cover; background-position: center top;` : ''}
          onclick={() => void goto(topResultHref(top))}
          onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), void goto(topResultHref(top)))}
          oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, top.kind === 'track' ? trackContextMenu(top.entry) : top.kind === 'album' ? buildAlbumMenu(top.entry) : buildArtistMenu(top.entry)) }}
        >
          {#if top.kind === 'artist'}
            {#if artistBg && !topArtistImageFailed}
              <img
                class="top-art top-art--circle"
                src={artistBg}
                alt={top.entry.name}
                onerror={() => { topArtistImageFailed = true }}
              />
            {:else}
              <div class="top-art top-art--circle fallback" style={`background: ${letterColor(top.entry.name)}`}>
                <span>{initials(top.entry.name)}</span>
              </div>
            {/if}
          {:else if top.entry.artwork_url}
            <div class="top-art" style={`background-image: url('${top.entry.artwork_url}')`}></div>
          {:else}
            <div class="top-art fallback" style={`background: ${letterColor(top.entry.title)}`}>
              <span>♫</span>
            </div>
          {/if}
          <div class="top-meta">
            <span class="top-kind">{top.kind === 'artist' ? 'Artist' : top.kind === 'album' ? 'Album' : 'Track'}{#if top.entry.in_library} · In your library{/if}</span>
            <h2 class="top-title" class:display-face={top.kind !== 'track'}>
              {top.kind === 'artist' ? top.entry.name : top.entry.title}
            </h2>
            {#if top.kind === 'album' && top.entry.artist_name}
              <p class="top-sub">{top.entry.artist_name}</p>
            {:else if top.kind === 'track' && top.entry.artist_name}
              <p class="top-sub">{top.entry.artist_name}</p>
            {/if}
            <button
              class="top-play-btn"
              onclick={(e) => { e.stopPropagation(); topResultPlay(top) }}
            >▶ {top.kind === 'artist' ? 'Open' : 'Play'}</button>
          </div>
        </div>
      </section>
    {/if}

    {#if sortedArtists.length > 0}
      <section class="results-section">
        <h3 class="section-label">Artists</h3>
        <div class="artists-row" use:wheelToHorizontal>
          {#each sortedArtists as artist (artist.tidal_id)}
            <a
              class="artist-card"
              class:in-library={artist.in_library}
              href={artist.in_library && artist.local_id != null
                ? `/artists/${artist.local_id}`
                : `/tidal/artists/${artist.tidal_id}`}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildArtistMenu(artist)) }}
            >
              <div class="avatar-wrap">
                {#if artist.artwork_url && !failedArtistImages.has(artist.tidal_id)}
                  <img
                    class="artist-avatar"
                    src={artist.artwork_url}
                    alt={artist.name}
                    onerror={() => { failedArtistImages = new Set([...failedArtistImages, artist.tidal_id]) }}
                  />
                {:else}
                  <div class="artist-avatar fallback" style={`background: ${letterColor(artist.name)}`}>
                    <span>{initials(artist.name)}</span>
                  </div>
                {/if}
                {#if artist.in_library}
                  <span class="lib-badge" aria-label="In your library"></span>
                {/if}
              </div>
              <span class="artist-name">{artist.name}</span>
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if sortedAlbums.length > 0}
      <section class="results-section">
        <h3 class="section-label">Albums</h3>
        <div class="albums-row" use:wheelToHorizontal>
          {#each sortedAlbums as album (album.tidal_id)}
            <a
              class="album-card"
              class:in-library={album.in_library}
              href={album.in_library && album.local_id != null
                ? `/albums/${album.local_id}`
                : `/tidal/albums/${album.tidal_id}`}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildAlbumMenu(album)) }}
            >
              <div class="art-wrap">
                {#if album.artwork_url}
                  <div class="album-art" style={`background-image: url('${album.artwork_url}')`}></div>
                {:else}
                  <div class="album-art fallback" style={`background: ${letterColor(album.title)}`}>
                    <span>♫</span>
                  </div>
                {/if}
                {#if album.in_library}
                  <span class="lib-badge" aria-label="In your library"></span>
                {/if}
                <button
                  class="art-play-overlay"
                  onclick={(e) => { e.preventDefault(); e.stopPropagation(); void playTidalAlbum(album.tidal_id) }}
                  aria-label="Play {album.title}"
                >▶</button>
              </div>
              <p class="album-title">{album.title}</p>
              {#if album.artist_name}
                <p class="album-artist">{album.artist_name}</p>
              {/if}
            </a>
          {/each}
        </div>
      </section>
    {/if}

    {#if showPlaylists && (filteredPlaylists.local.length > 0 || filteredPlaylists.tidal.length > 0)}
      <section class="results-section">
        <h3 class="section-label">Playlists</h3>
        <div class="albums-row" use:wheelToHorizontal>

          {#each filteredPlaylists.local as playlist (playlist.id)}
            <a
              class="album-card in-library"
              href="/playlists"
            >
              <div class="art-wrap">
                <div class="album-art fallback" style="background: {letterColor(playlist.name)}">
                  <span>♫</span>
                </div>
                <span class="lib-badge" aria-label="In your library"></span>
              </div>
              <p class="album-title">{playlist.name}</p>
              <p class="album-artist">{playlist.is_smart ? 'Smart playlist' : 'Playlist'} · {playlist.track_count} tracks</p>
            </a>
          {/each}

          {#each filteredPlaylists.tidal as playlist (playlist.uuid)}
            <div
              class="album-card"
              role="button"
              tabindex="0"
              onclick={() => void playTidalPlaylist(playlist.uuid)}
              onkeydown={(e) => e.key === 'Enter' && void playTidalPlaylist(playlist.uuid)}
            >
              <div class="art-wrap">
                {#if playlist.square_image}
                  <div
                    class="album-art"
                    style="background-image: url('https://resources.tidal.com/images/{playlist.square_image.replaceAll('-', '/')}/320x320.jpg')"
                  ></div>
                {:else}
                  <div class="album-art fallback" style="background: {letterColor(playlist.title)}">
                    <span>♫</span>
                  </div>
                {/if}
                <button
                  class="art-play-overlay"
                  onclick={(e) => { e.stopPropagation(); void playTidalPlaylist(playlist.uuid) }}
                  aria-label="Play {playlist.title}"
                >▶</button>
              </div>
              <p class="album-title">{playlist.title}</p>
              <p class="album-artist">TIDAL · {playlist.number_of_tracks ?? '?'} tracks</p>
            </div>
          {/each}

        </div>
      </section>
    {/if}

    {#if sortedTracks.length > 0}
      <section class="results-section">
        <h3 class="section-label">Tracks</h3>
        <ul class="tracks-list">
          {#each sortedTracks as track, idx (track.tidal_id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              class:cursor={cursor === idx}
              data-cursor-idx={idx}
              role="button"
              tabindex="0"
              onclick={() => void playTidalTrackNow(toPlayable(track))}
              onmouseenter={() => { cursor = idx }}
              onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), void playTidalTrackNow(toPlayable(track)))}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, trackContextMenu(track)) }}
            >
              {#if track.artwork_url}
                <div class="track-art" style={`background-image: url('${track.artwork_url}')`}></div>
              {:else}
                <div class="track-art fallback" style={`background: ${letterColor(track.title)}`}>
                  <span>♫</span>
                </div>
              {/if}
              <div class="track-meta">
                <p class="track-title">
                  {track.title}
                  {#if track.in_library}<span class="lib-dot" aria-label="In your library"></span>{/if}
                </p>
                <p class="track-subtitle">
                  {#if track.artist_name}
                    {#if track.artist_id != null}
                      <a
                        href={`/tidal/artists/${track.artist_id}`}
                        class="subtitle-link"
                        onclick={(e) => e.stopPropagation()}
                      >{track.artist_name}</a>
                    {:else}
                      <span>{track.artist_name}</span>
                    {/if}
                  {/if}
                  {#if track.artist_name && track.album_title} — {/if}
                  {#if track.album_title}
                    {#if track.album_tidal_id != null}
                      <a
                        href={`/tidal/albums/${track.album_tidal_id}`}
                        class="subtitle-link"
                        onclick={(e) => e.stopPropagation()}
                      >{track.album_title}</a>
                    {:else}
                      <span>{track.album_title}</span>
                    {/if}
                  {/if}
                </p>
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
              <div class="row-actions">
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void playTidalTrackNow(toPlayable(track)) }}
                  title="Play now"
                  aria-label="Play {track.title}"
                >▶</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void addTidalTrackToQueue(toPlayable(track)) }}
                  title="Add to queue"
                  aria-label="Queue {track.title}"
                >＋</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); void startTidalSongRadio(track) }}
                  title="Song radio — mix of related tracks from your library and Tidal"
                  aria-label="Start radio from {track.title}"
                >◎</button>
                <button
                  class="row-btn"
                  onclick={(e) => { e.stopPropagation(); openContextMenu(e, trackContextMenu(track)) }}
                  title="More options"
                  aria-label="More options"
                >⋯</button>
              </div>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if vibeTrack && vibeTrack.length > 0}
      <section class="results-section discovery-section">
        <h3 class="section-label">Same vibe</h3>
        <ul class="tracks-list">
          {#each vibeTrack as track (track.id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              role="button"
              tabindex="0"
              onclick={() => void playTrackNow(track.id)}
              onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.id)}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTrackMenu({ id: track.id, title: track.title, artist_name: track.artist_name, album_title: track.album_title })) }}
            >
              {#if track.artwork_url}
                <div class="track-art" style="background-image:url('{track.artwork_url}')"></div>
              {:else}
                <div class="track-art fallback" style="background:{letterColor(track.title)}"><span>♫</span></div>
              {/if}
              <div class="track-meta">
                <p class="track-title">{track.title}</p>
                <p class="track-subtitle">{track.artist_name ?? ''}{track.bpm ? ` · ${Math.round(track.bpm)} bpm` : ''}{track.camelot_key ? ` · ${track.camelot_key}` : ''}</p>
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if underratedTracks && underratedTracks.length > 0}
      <section class="results-section discovery-section">
        <h3 class="section-label">Unplayed in your library</h3>
        <ul class="tracks-list">
          {#each underratedTracks as track (track.id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              role="button"
              tabindex="0"
              onclick={() => void playTrackNow(track.id)}
              onkeydown={(e) => e.key === 'Enter' && void playTrackNow(track.id)}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, buildTrackMenu({ id: track.id, title: track.title, artist_name: track.artist_name, album_title: track.album_title })) }}
            >
              {#if track.artwork_url}
                <div class="track-art" style="background-image:url('{track.artwork_url}')"></div>
              {:else}
                <div class="track-art fallback" style="background:{letterColor(track.title)}"><span>♫</span></div>
              {/if}
              <div class="track-meta">
                <p class="track-title">{track.title}</p>
                <p class="track-subtitle">{track.artist_name ?? ''}{track.album_title ? ` · ${track.album_title}` : ''}</p>
              </div>
              <span class="track-duration">{formatDuration(track.duration_ms)}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

  {/if}
</div>

<style>
  .search-page {
    width: min(100%, 1280px);
    margin: 0 auto;
    padding: 16px 4px 80px;
  }
  .search-header {
    max-width: 1200px;
    margin: 0 auto 40px;
    padding: 0 4px;
  }
  .search-input {
    display: block;
    width: 100%;
    max-width: 640px;
    margin: 0 auto;
    background: var(--bg-raised);
    border: 1px solid var(--border-strong);
    border-radius: 24px;
    padding: 12px 22px;
    font-size: 15px;
    color: var(--text-primary);
    outline: none;
    transition: border-color 0.15s, background 0.15s;
  }
  .search-input::placeholder { color: var(--text-tertiary); }

  .hint-chips {
    margin: 10px auto 0;
    max-width: 640px;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .hint-chip {
    background: var(--bg-surface, rgba(255, 255, 255, 0.06));
    border: 1px solid var(--border-subtle);
    color: var(--text-secondary);
    border-radius: 999px;
    padding: 4px 12px;
    font-size: 11.5px;
    font-family: var(--font-mono, ui-monospace, monospace);
    cursor: pointer;
    transition: background 0.12s, color 0.12s, border-color 0.12s;
  }
  .hint-chip:hover {
    background: var(--accent-soft);
    color: var(--accent-strong);
    border-color: var(--accent-line);
  }
  .inline-hint {
    margin: 8px auto 0;
    max-width: 640px;
    font-size: 11px;
    color: var(--text-tertiary);
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .inline-hint code {
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 11px;
    color: var(--accent-strong);
  }
  .hint-tab {
    background: var(--bg-surface, rgba(255, 255, 255, 0.06));
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    padding: 0 5px;
    font-size: 10px;
    color: var(--text-secondary);
  }

  .kbd-hint {
    margin: 10px auto 0;
    max-width: 640px;
    font-size: 11px;
    color: var(--text-tertiary);
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 2px;
  }
  .filter-chips {
    margin: 10px 0 0;
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .filter-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    background: var(--bg-elevated);
    border: 1px solid var(--accent-line);
    color: var(--text-secondary);
    border-radius: 14px;
    padding: 4px 12px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
    transition: background 0.15s, border-color 0.15s, color 0.15s;
  }
  .filter-chip:hover {
    background: var(--bg-hover);
    border-color: var(--accent);
    color: var(--text-primary);
  }
  .chip-x {
    font-size: 14px;
    line-height: 1;
    color: var(--text-tertiary);
    margin-left: 2px;
  }
  .filter-chip:hover .chip-x { color: var(--text-primary); }
  .no-audio-results {
    color: var(--text-muted);
    font-size: 13px;
    margin: 0;
  }
  .filter-pills {
    margin: 14px 0 0;
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .filter-pill {
    background: transparent;
    border: 1px solid var(--border-subtle);
    color: var(--text-secondary);
    border-radius: 14px;
    padding: 5px 14px;
    font-size: 12px;
    cursor: pointer;
    font-family: inherit;
    transition: background 0.15s, border-color 0.15s, color 0.15s;
  }
  .filter-pill:hover {
    border-color: var(--accent-line);
    color: var(--text-primary);
  }
  .filter-pill.active {
    background: var(--accent);
    border-color: var(--accent);
    color: #fff;
    font-weight: 600;
  }
  .kbd-hint kbd {
    display: inline-block;
    padding: 1px 5px;
    background: var(--bg-raised);
    border: 1px solid var(--border-subtle);
    border-radius: 4px;
    font-family: var(--font-mono, ui-monospace, monospace);
    font-size: 10px;
    color: var(--text-secondary);
    line-height: 1.4;
    margin: 0 1px;
  }
  .search-input:focus {
    border-color: var(--accent);
    background: var(--bg-elevated);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }
  .search-hint {
    color: var(--text-muted);
    font-size: 14px;
    margin-top: 64px;
    text-align: center;
  }
  .search-error { color: var(--state-error); }
  .results-section { margin-bottom: 32px; max-width: 1200px; margin-left: auto; margin-right: auto; }
  .recent-section { margin-top: 36px; max-width: 720px; margin-left: auto; margin-right: auto; }
  .recent-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 12px;
  }
  .recent-clear {
    background: none;
    border: none;
    color: var(--text-tertiary);
    font-size: 11px;
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 4px;
  }
  .recent-clear:hover { color: var(--text-secondary); background: var(--bg-hover); }
  .recent-chips {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .recent-chip {
    background: var(--bg-raised);
    border: 1px solid var(--border-subtle);
    border-radius: 14px;
    padding: 6px 14px;
    font-size: 12px;
    color: var(--text-secondary);
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.15s, color 0.15s;
  }
  .recent-chip:hover {
    border-color: var(--accent-line);
    color: var(--text-primary);
  }
  .top-result-section { margin-bottom: 28px; max-width: 1200px; margin-left: auto; margin-right: auto; }
  .top-result-card {
    display: grid;
    grid-template-columns: 168px 1fr;
    gap: 24px;
    padding: 20px;
    border-radius: 12px;
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    text-decoration: none;
    transition: transform 0.18s ease, border-color 0.18s, background 0.18s;
    align-items: center;
  }
  .top-result-card:hover {
    transform: translateY(-2px);
    border-color: var(--accent-line);
    background: var(--bg-raised);
  }
  .top-result-card.in-library { border-color: var(--accent-line); }
  .top-art {
    width: 168px;
    height: 168px;
    border-radius: 8px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-raised);
    box-shadow: 0 12px 28px -12px rgba(0,0,0,0.6);
    object-fit: cover;
  }
  .top-art.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .top-art--circle { border-radius: 50%; }
  .top-art.fallback span {
    font-size: 56px;
    color: rgba(255,255,255,0.55);
    font-weight: 600;
  }
  .top-art--circle.fallback span { font-size: 40px; }
  .top-meta {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }
  .top-kind {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 1.5px;
    color: var(--text-tertiary);
    font-weight: 600;
  }
  .top-title {
    font-family: var(--font-body, inherit);
    font-size: clamp(24px, 2.8vw, 36px);
    font-weight: 750;
    line-height: 1.1;
    letter-spacing: 0;
    margin: 0;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .top-title.display-face {
    font-family: var(--font-display, serif);
    font-size: clamp(28px, 3.4vw, 42px);
    font-weight: 600;
    line-height: 1.05;
    letter-spacing: -0.02em;
  }
  .top-sub {
    font-size: 14px;
    color: var(--text-secondary);
    margin: 0;
  }
  .top-play-btn {
    align-self: flex-start;
    margin-top: 10px;
    background: var(--accent);
    color: #fff;
    border: none;
    border-radius: 22px;
    padding: 9px 22px;
    font-size: 13px;
    font-weight: 600;
    cursor: pointer;
    transition: opacity 0.15s, transform 0.15s;
  }
  .top-play-btn:hover { opacity: 0.9; transform: scale(1.03); }
  .section-label {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 1.5px;
    color: var(--accent);
    margin-bottom: 14px;
  }
  .trending-head {
    display: flex;
    align-items: center;
    gap: 12px;
    margin-bottom: 14px;
  }
  .trending-head .section-label { margin-bottom: 0; }
  .trending-loading {
    font-size: 0.78rem;
    color: var(--text-muted);
    font-style: italic;
  }
  .chip-group {
    display: inline-flex;
    gap: 4px;
    padding: 2px;
    background: rgba(255, 255, 255, 0.04);
    border-radius: 999px;
  }
  .chip {
    background: transparent;
    border: none;
    color: var(--text-muted, #888);
    font: inherit;
    font-size: 0.75rem;
    font-weight: 500;
    padding: 4px 10px;
    border-radius: 999px;
    cursor: pointer;
    transition: background 0.15s ease, color 0.15s ease;
  }
  .chip:hover { color: var(--text, #fff); }
  .chip.active {
    background: rgba(255, 255, 255, 0.12);
    color: var(--text, #fff);
  }
  .trending-list {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  /* Artists */
  .artists-row {
    display: flex;
    gap: 20px;
    overflow-x: auto;
    padding-bottom: 8px;
    scrollbar-width: none;
  }
  .artists-row::-webkit-scrollbar { display: none; }
  .artist-card {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
    text-decoration: none;
    flex-shrink: 0;
    width: 84px;
    transition: transform 0.18s ease;
  }
  .artist-card:hover { transform: translateY(-3px); }
  .artist-card:hover .artist-avatar {
    opacity: 0.85;
  }
  .avatar-wrap, .art-wrap {
    position: relative;
    line-height: 0;
  }
  .lib-badge {
    position: absolute;
    bottom: 3px;
    right: 3px;
    width: 10px;
    height: 10px;
    border-radius: 50%;
    background: var(--accent);
    border: 2px solid var(--bg-base);
  }
  .lib-dot {
    display: inline-block;
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--accent);
    margin-left: 6px;
    vertical-align: middle;
    flex-shrink: 0;
  }
  .artist-card.in-library .artist-name {
    color: var(--text-primary);
    font-weight: 600;
  }
  .album-card.in-library .album-title {
    color: var(--text-primary);
    font-weight: 600;
  }
  .artist-avatar {
    width: 72px;
    height: 72px;
    border-radius: 50%;
    background: var(--bg-raised);
    object-fit: cover;
    display: block;
    transition: opacity 0.15s;
  }
  .artist-avatar.fallback {
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .artist-avatar.fallback span {
    font-family: var(--font-body, inherit);
    font-size: 22px;
    font-weight: 600;
    color: rgba(255, 255, 255, 0.78);
    letter-spacing: 0.02em;
  }
  .artist-name {
    font-size: 11px;
    color: var(--text-secondary);
    text-align: center;
    width: 84px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  /* Albums */
  .albums-row {
    display: flex;
    gap: 16px;
    overflow-x: auto;
    padding-bottom: 8px;
    scrollbar-width: none;
  }
  .albums-row::-webkit-scrollbar { display: none; }
  .album-card {
    text-decoration: none;
    flex-shrink: 0;
    width: 128px;
    transition: transform 0.18s ease;
  }
  .album-card:hover { transform: translateY(-3px); }
  .art-play-overlay {
    position: absolute;
    bottom: 8px;
    right: 8px;
    width: 36px;
    height: 36px;
    border-radius: 50%;
    background: var(--accent);
    color: #fff;
    font-size: 14px;
    border: none;
    cursor: pointer;
    opacity: 0;
    transform: translateY(4px);
    transition: opacity 0.15s, transform 0.15s;
    box-shadow: 0 6px 16px -4px rgba(0,0,0,0.5);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0;
  }
  .art-wrap:hover .art-play-overlay {
    opacity: 1;
    transform: translateY(0);
  }
  .art-play-overlay:hover { transform: scale(1.08); opacity: 1; }
  .album-art {
    width: 128px;
    height: 128px;
    border-radius: 6px;
    background: var(--bg-raised);
    background-size: cover;
    background-position: center;
    margin-bottom: 7px;
    transition: opacity 0.15s;
    display: flex;
    align-items: center;
    justify-content: center;
  }
  .album-art.fallback span {
    font-size: 38px;
    color: rgba(255, 255, 255, 0.5);
  }
  .album-card:hover .album-art { opacity: 0.85; }
  .album-title {
    font-size: 12px;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0;
  }
  .album-artist {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 2px 0 0;
  }
  /* Tracks */
  .tracks-list {
    list-style: none;
    padding: 0;
    margin: 0;
  }
  .track-row {
    display: grid;
    grid-template-columns: 38px 1fr auto auto;
    align-items: center;
    gap: 12px;
    padding: 8px 8px;
    border-radius: 6px;
    cursor: pointer;
    transition: background 0.12s;
  }
  .row-actions {
    display: flex;
    align-items: center;
    gap: 2px;
  }
  .track-row:hover { background: var(--bg-hover); }
  .track-row.cursor {
    background: var(--bg-hover);
    box-shadow: inset 2px 0 0 var(--accent);
  }
  .track-art {
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
  .track-art.fallback span {
    font-size: 16px;
    color: rgba(255, 255, 255, 0.5);
  }
  .track-title {
    font-size: 13px;
    color: var(--text-primary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 0;
  }
  .track-subtitle {
    font-size: 11px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    margin: 2px 0 0;
  }
  .subtitle-link { color: inherit; text-decoration: none; }
  .subtitle-link:hover { color: var(--text-primary); text-decoration: underline; }
  .track-duration {
    font-size: 12px;
    color: var(--text-muted);
    white-space: nowrap;
  }
  .row-btn {
    background: none;
    border: none;
    color: var(--text-tertiary);
    cursor: pointer;
    font-size: 14px;
    padding: 4px;
    border-radius: 4px;
    opacity: 0;
    transition: opacity 0.1s, color 0.1s;
  }
  .track-row:hover .row-btn { opacity: 1; }
  .row-btn:hover { color: var(--text-primary); }
  .discovery-section { opacity: 0.9; }
  .discovery-section .section-label { color: var(--text-muted); }
  .top-result-card.artist-hero {
    position: relative;
    overflow: hidden;
    min-height: 200px;
  }
  .top-result-card.artist-hero::after {
    content: '';
    position: absolute;
    inset: 0;
    background: linear-gradient(to right, rgba(0,0,0,0.88) 0%, rgba(0,0,0,0.5) 55%, rgba(0,0,0,0.1) 100%);
    pointer-events: none;
  }
  .top-result-card.artist-hero .top-art,
  .top-result-card.artist-hero .top-meta {
    position: relative;
    z-index: 1;
  }
  .top-result-card.artist-hero .top-art--circle {
    width: 100px;
    height: 100px;
  }
</style>
