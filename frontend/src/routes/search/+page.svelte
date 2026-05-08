<script lang="ts">
  import { onMount } from 'svelte'
  import { goto, beforeNavigate } from '$app/navigation'
  import type { Snapshot } from './$types'
  import { api, type TidalSearchResults, type TidalSearchAlbum, type TidalSearchArtist, type TidalSearchTrack, type AudioSearchResult, type AudioSearchParams, type Genre, type VibeTrack, type BasicTrack, type Playlist, type TidalSearchPlaylist, type SpotifyPlaylistSearchItem } from '$lib/api/client'
  import TrendingShelf from '$lib/components/charts/TrendingShelf.svelte'
  import { buildTidalTrackMenu, buildTrackMenu } from '$lib/player/track_menu'
  import { buildAlbumMenu } from '$lib/player/album_menu'
  import { buildArtistMenu } from '$lib/player/artist_menu'
  import { openContextMenu, type MenuItem } from '$lib/stores/context_menu'
  import { playTidalTrackNow, playTidalAlbum, playTidalTrackNext, addTidalTrackToQueue, startTidalSongRadio, playTrackNow, playTidalPlaylist } from '$lib/stores/player'
  import { formatTrackDuration } from '$lib/utils/format'
  import { parseQuery, filtersToChips, type ParsedQuery } from '$lib/search/query_parser'
  import { buildAudioParams as sharedBuildAudioParams } from '$lib/search/audio_params'
  import { parseIntent } from '$lib/search/intent'
  import { wheelToHorizontal } from '$lib/actions/wheel-to-horizontal'
  import { tidalSearchTrackToPlayable } from '$lib/utils/track'
  import { canPlayTrack, getPlayableLabel } from '$lib/player/playable'
  import { mergeLocalIntoTidal } from '$lib/search/merge_local'
  import PlayOverlay from '$lib/components/ui/PlayOverlay.svelte'

  const RECENT_KEY = 'noor_recent_searches'
  const RECENT_MAX = 8
  // Backend caps `limit` at 50 (see tidal_search route). Use the cap as the
  // page size so each round-trip pulls the maximum the upstream allows.
  const SEARCH_PAGE_SIZE = 50

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
  let spotifyPlaylistResults = $state<SpotifyPlaylistSearchItem[]>([])

  type FilterMode = 'all' | 'artists' | 'albums' | 'tracks' | 'library' | 'playlists'
  let filterMode = $state<FilterMode>('all')

  // Infinite-scroll state. The combined Tidal endpoint shares one offset for
  // tracks/albums/artists; playlists are paged separately because they hit a
  // different endpoint. `lastQuery` lets `loadMore` re-query the same string
  // without depending on the live `query` input (which may be mid-edit).
  let tidalOffset = $state(0)
  let tidalPlaylistOffset = $state(0)
  let spotifyPlaylistOffset = $state(0)
  let hasMoreTidal = $state(true)
  let hasMoreTidalPlaylists = $state(true)
  let hasMoreSpotifyPlaylists = $state(true)
  let loadingMore = $state(false)
  let lastQuery = $state('')

  // Trending shelf is encapsulated in <TrendingShelf /> below; shown only when
  // the query is empty (inside the existing {#if !query.trim()} branch).

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
      spotifyPlaylistResults = []
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
          spotifyPlaylistResults = []
        } else {
          audioResults = null
          // Reset paging — every fresh query starts at offset 0.
          tidalOffset = 0
          tidalPlaylistOffset = 0
          spotifyPlaylistOffset = 0
          hasMoreTidal = true
          hasMoreTidalPlaylists = true
          hasMoreSpotifyPlaylists = true
          lastQuery = q
          const cacheKey = q.toLowerCase()
          const cached = resultCache.get(cacheKey)
          if (cached) {
            // Cache holds raw TIDAL only — re-run local search every time so
            // newly-favorited tracks show up without a manual refresh.
            const [localRes, tidalPlRes, spotifyPlRes] = await Promise.allSettled([
              api.search(q, SEARCH_PAGE_SIZE),
              api.searchTidalPlaylists(q, signal, { limit: SEARCH_PAGE_SIZE, offset: 0 }),
              api.searchSpotifyPlaylists(q, SEARCH_PAGE_SIZE, signal, 0),
            ])
            const localResults = localRes.status === 'fulfilled' ? localRes.value : null
            results = localResults ? mergeLocalIntoTidal(localResults, cached) : cached
            tidalOffset = SEARCH_PAGE_SIZE
            // Cached page may already cap a category — assume more exists; the
            // next load-more attempt will discover the truth and flip the flag.
            tidalPlaylistResults = tidalPlRes.status === 'fulfilled' ? tidalPlRes.value.playlists : []
            spotifyPlaylistResults = spotifyPlRes.status === 'fulfilled' ? spotifyPlRes.value : []
            tidalPlaylistOffset = tidalPlaylistResults.length
            spotifyPlaylistOffset = spotifyPlaylistResults.length
            if (tidalPlaylistResults.length < SEARCH_PAGE_SIZE) hasMoreTidalPlaylists = false
            if (spotifyPlaylistResults.length < SEARCH_PAGE_SIZE) hasMoreSpotifyPlaylists = false
          } else {
            // Fan out all four upstream searches at once. Local DB and TIDAL
            // both feed the unified results list (library entries float to
            // top via the in_library sort); the two playlist lookups are
            // best-effort and degrade to empty arrays. TIDAL-track is the
            // only one whose failure aborts — no point rendering search with
            // zero discovery results.
            const [localRes, tracksRes, tidalPlRes, spotifyPlRes] = await Promise.allSettled([
              api.search(q, SEARCH_PAGE_SIZE),
              api.searchTidal(q, SEARCH_PAGE_SIZE, signal, 0),
              api.searchTidalPlaylists(q, signal, { limit: SEARCH_PAGE_SIZE, offset: 0 }),
              api.searchSpotifyPlaylists(q, SEARCH_PAGE_SIZE, signal, 0),
            ])

            if (tracksRes.status !== 'fulfilled') {
              throw tracksRes.reason
            }
            const tidalResults = tracksRes.value
            const localResults = localRes.status === 'fulfilled' ? localRes.value : null
            const fresh = localResults ? mergeLocalIntoTidal(localResults, tidalResults) : tidalResults
            results = fresh
            // Cache only the raw TIDAL response so subsequent hits can re-merge
            // a fresh local snapshot (favorites change without query change).
            resultCache.set(cacheKey, tidalResults)
            if (resultCache.size > 5) resultCache.delete(resultCache.keys().next().value!)
            tidalOffset = SEARCH_PAGE_SIZE
            if (
              tidalResults.tracks.length < SEARCH_PAGE_SIZE &&
              tidalResults.albums.length < SEARCH_PAGE_SIZE &&
              tidalResults.artists.length < SEARCH_PAGE_SIZE
            ) {
              hasMoreTidal = false
            }

            tidalPlaylistResults = tidalPlRes.status === 'fulfilled' ? tidalPlRes.value.playlists : []
            spotifyPlaylistResults = spotifyPlRes.status === 'fulfilled' ? spotifyPlRes.value : []
            tidalPlaylistOffset = tidalPlaylistResults.length
            spotifyPlaylistOffset = spotifyPlaylistResults.length
            if (tidalPlaylistResults.length < SEARCH_PAGE_SIZE) hasMoreTidalPlaylists = false
            if (spotifyPlaylistResults.length < SEARCH_PAGE_SIZE) hasMoreSpotifyPlaylists = false
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

  // Load the next page for whichever single-category pill is active. The
  // combined Tidal endpoint covers tracks/albums/artists with a shared offset,
  // so loading once advances all three; the Playlists pill triggers two
  // independent fetches (TIDAL + Spotify) since they live on separate endpoints.
  async function loadMore() {
    if (loadingMore) return
    if (!lastQuery.trim()) return
    if (filterMode === 'all' || filterMode === 'library') return
    if (audioResults !== null) return
    const needsTidal =
      (filterMode === 'tracks' || filterMode === 'albums' || filterMode === 'artists') &&
      hasMoreTidal &&
      results !== null
    const needsPlaylists =
      filterMode === 'playlists' && (hasMoreTidalPlaylists || hasMoreSpotifyPlaylists)
    if (!needsTidal && !needsPlaylists) return

    loadingMore = true
    try {
      if (needsTidal && results) {
        const next = await api.searchTidal(lastQuery, SEARCH_PAGE_SIZE, undefined, tidalOffset)
        tidalOffset += SEARCH_PAGE_SIZE
        // De-dupe by id — Tidal occasionally returns overlapping pages.
        const seenTracks = new Set(results.tracks.map((t) => t.tidal_id))
        const seenAlbums = new Set(results.albums.map((a) => a.tidal_id))
        const seenArtists = new Set(results.artists.map((a) => a.tidal_id))
        const newTracks = next.tracks.filter((t) => !seenTracks.has(t.tidal_id))
        const newAlbums = next.albums.filter((a) => !seenAlbums.has(a.tidal_id))
        const newArtists = next.artists.filter((a) => !seenArtists.has(a.tidal_id))
        results = {
          tracks: [...results.tracks, ...newTracks],
          albums: [...results.albums, ...newAlbums],
          artists: [...results.artists, ...newArtists],
        }
        if (
          next.tracks.length < SEARCH_PAGE_SIZE &&
          next.albums.length < SEARCH_PAGE_SIZE &&
          next.artists.length < SEARCH_PAGE_SIZE
        ) {
          hasMoreTidal = false
        }
      }
      if (needsPlaylists) {
        const tasks: Promise<unknown>[] = []
        if (hasMoreTidalPlaylists) {
          tasks.push(
            api
              .searchTidalPlaylists(lastQuery, undefined, { limit: SEARCH_PAGE_SIZE, offset: tidalPlaylistOffset })
              .then((r) => {
                const seen = new Set(tidalPlaylistResults.map((p) => p.uuid))
                const fresh = r.playlists.filter((p) => !seen.has(p.uuid))
                tidalPlaylistResults = [...tidalPlaylistResults, ...fresh]
                tidalPlaylistOffset += SEARCH_PAGE_SIZE
                if (r.playlists.length < SEARCH_PAGE_SIZE) hasMoreTidalPlaylists = false
              })
              .catch(() => { hasMoreTidalPlaylists = false }),
          )
        }
        if (hasMoreSpotifyPlaylists) {
          tasks.push(
            api
              .searchSpotifyPlaylists(lastQuery, SEARCH_PAGE_SIZE, undefined, spotifyPlaylistOffset)
              .then((items) => {
                const seen = new Set(spotifyPlaylistResults.map((p) => p.spotifyId))
                const fresh = items.filter((p) => !seen.has(p.spotifyId))
                spotifyPlaylistResults = [...spotifyPlaylistResults, ...fresh]
                spotifyPlaylistOffset += SEARCH_PAGE_SIZE
                if (items.length < SEARCH_PAGE_SIZE) hasMoreSpotifyPlaylists = false
              })
              .catch(() => { hasMoreSpotifyPlaylists = false }),
          )
        }
        await Promise.allSettled(tasks)
      }
    } finally {
      loadingMore = false
    }
  }

  // IntersectionObserver on a sentinel below the result list. Fires loadMore
  // whenever the sentinel scrolls into view — only if a single-category pill
  // is active, which is the only mode where pagination is meaningful.
  let infiniteSentinel = $state<HTMLDivElement | null>(null)
  $effect(() => {
    if (!infiniteSentinel) return
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) void loadMore()
    }, { rootMargin: '400px 0px' })
    observer.observe(infiniteSentinel)
    return () => observer.disconnect()
  })

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
    if (!query.trim())
      return {
        local: [] as Playlist[],
        tidal: [] as TidalSearchPlaylist[],
        spotify: [] as SpotifyPlaylistSearchItem[],
      }
    const q = query.trim().toLowerCase()
    const matched = localPlaylists.filter(p => p.name.toLowerCase().includes(q))
    const localNames = new Set(matched.map(p => p.name.toLowerCase()))
    const tidalOnly = tidalPlaylistResults.filter(tp => !localNames.has(tp.title.toLowerCase()))
    // Keep Spotify visible even when a TIDAL/local playlist has the same
    // title. The source chip is the useful distinction on mixed-service search.
    const spotifyOnly = spotifyPlaylistResults.filter(sp => sp.spotifyId)
    return { local: matched, tidal: tidalOnly, spotify: spotifyOnly }
  })

  const isFilteredEmpty = $derived(
    results !== null &&
    !isEmpty &&
    sortedTracks.length === 0 &&
    sortedAlbums.length === 0 &&
    sortedArtists.length === 0 &&
    !(showPlaylists && (filteredPlaylists.local.length > 0 || filteredPlaylists.tidal.length > 0 || filteredPlaylists.spotify.length > 0))
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

  function albumMenuItems(album: TidalSearchAlbum): MenuItem[] {
    return buildAlbumMenu(
      {
        local_id: album.local_id,
        tidal_id: album.tidal_id,
        title: album.title,
        artist_name: album.artist_name,
        in_library: album.in_library,
      },
      { isLocal: album.in_library && album.local_id != null }
    )
  }

  function artistMenuItems(artist: TidalSearchArtist): MenuItem[] {
    return buildArtistMenu(
      {
        local_id: artist.local_id,
        tidal_id: artist.tidal_id,
        name: artist.name,
        in_library: artist.in_library,
      },
      { isLocal: artist.in_library && artist.local_id != null }
    )
  }

  function localPlaylistMenuItems(_playlist: Playlist): MenuItem[] {
    return [
      { label: 'Open playlist', icon: '↗', onSelect: () => void goto('/playlists') },
    ]
  }

  function tidalPlaylistMenuItems(playlist: TidalSearchPlaylist): MenuItem[] {
    return [
      { label: 'Play', icon: '▶', onSelect: () => void playTidalPlaylist(playlist.uuid) },
    ]
  }

  function spotifyPlaylistMenuItems(playlist: SpotifyPlaylistSearchItem): MenuItem[] {
    return [
      { label: 'Open', icon: '↗', onSelect: () => void goto(`/spotify-playlist/${playlist.spotifyId}`) },
    ]
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

  function canPlaySearchTrack(track: TidalSearchTrack): boolean {
    return canPlayTrack(toPlayable(track))
  }

  function playableSearchLabel(track: TidalSearchTrack): string {
    return getPlayableLabel(toPlayable(track))
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
    scrollY: number
  }
  export const snapshot: Snapshot<SearchSnapshot> = {
    capture: () => ({
      query,
      filterMode,
      scrollY: typeof window !== 'undefined' ? window.scrollY : 0
    }),
    restore: (saved) => {
      filterMode = saved.filterMode
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
      <TrendingShelf limit={25} />
    </section>

    {#if recent.length === 0}
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
              <span class="track-duration">{formatTrackDuration(track.duration_ms)}</span>
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
          oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, top.kind === 'track' ? trackContextMenu(top.entry) : top.kind === 'album' ? albumMenuItems(top.entry) : artistMenuItems(top.entry)) }}
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
        <div
          class="artists-row"
          class:section-grid-artists={filterMode === 'artists'}
          use:wheelToHorizontal
        >
          {#each sortedArtists as artist (artist.tidal_id)}
            <a
              class="artist-card"
              class:in-library={artist.in_library}
              href={artist.in_library && artist.local_id != null
                ? `/artists/${artist.local_id}`
                : `/tidal/artists/${artist.tidal_id}`}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, artistMenuItems(artist)) }}
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
        <div
          class="albums-row"
          class:section-grid-albums={filterMode === 'albums'}
          use:wheelToHorizontal
        >
          {#each sortedAlbums as album (album.tidal_id)}
            <a
              class="album-card"
              class:in-library={album.in_library}
              href={album.in_library && album.local_id != null
                ? `/albums/${album.local_id}`
                : `/tidal/albums/${album.tidal_id}`}
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, albumMenuItems(album)) }}
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
                <PlayOverlay
                  position="corner"
                  size="sm"
                  label="Play {album.title}"
                  onclick={(e) => { e.preventDefault(); e.stopPropagation(); void playTidalAlbum(album.tidal_id) }}
                />
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

    {#if showPlaylists && (filteredPlaylists.local.length > 0 || filteredPlaylists.tidal.length > 0 || filteredPlaylists.spotify.length > 0)}
      <section class="results-section">
        <h3 class="section-label">Playlists</h3>
        <div
          class="albums-row"
          class:section-grid-albums={filterMode === 'playlists'}
          use:wheelToHorizontal
        >

          {#each filteredPlaylists.local as playlist (playlist.id)}
            <a
              class="album-card in-library"
              href="/playlists"
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, localPlaylistMenuItems(playlist), playlist.name) }}
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
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, tidalPlaylistMenuItems(playlist), playlist.title) }}
            >
              <div class="art-wrap">
                {#if playlist.artwork_url}
                  <div
                    class="album-art"
                    style="background-image: url('{playlist.artwork_url}')"
                  ></div>
                {:else}
                  <div class="album-art fallback" style="background: {letterColor(playlist.title)}">
                    <span>♫</span>
                  </div>
                {/if}
                <PlayOverlay
                  position="corner"
                  size="sm"
                  label="Play {playlist.title}"
                  onclick={(e) => { e.stopPropagation(); void playTidalPlaylist(playlist.uuid) }}
                />
              </div>
              <p class="album-title">{playlist.title}</p>
              <p class="album-artist">TIDAL · {playlist.number_of_tracks ?? '?'} tracks</p>
            </div>
          {/each}

          {#each filteredPlaylists.spotify as playlist (playlist.spotifyId)}
            <a
              class="album-card spotify-card"
              href="/spotify-playlist/{playlist.spotifyId}"
              oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, spotifyPlaylistMenuItems(playlist), playlist.title ?? 'Spotify playlist') }}
            >
              <div class="art-wrap">
                {#if playlist.thumbnail}
                  <div
                    class="album-art"
                    style="background-image: url('{playlist.thumbnail}')"
                  ></div>
                {:else}
                  <div class="album-art fallback" style="background: {letterColor(playlist.title ?? 'playlist')}">
                    <span>♫</span>
                  </div>
                {/if}
                <span class="source-chip" aria-label="Spotify">Spotify</span>
              </div>
              <p class="album-title">{playlist.title ?? 'Untitled playlist'}</p>
              <p class="album-artist">
                {#if playlist.owner}{playlist.owner} · {/if}{playlist.totalTracks ?? '?'} tracks
              </p>
            </a>
          {/each}

        </div>
      </section>
    {/if}

    {#if sortedTracks.length > 0}
      <section class="results-section">
        <h3 class="section-label">Tracks</h3>
        {#if filterMode === 'tracks'}
          <div class="search-track-table" role="list">
            <div class="search-track-header">
              <span class="col-num">#</span>
              <span class="col-title">Title</span>
              <span class="col-artist">Artist</span>
              <span class="col-album">Album</span>
              <span class="col-quality">Quality</span>
              <span class="col-duration">Duration</span>
              <span class="col-actions"></span>
            </div>
            {#each sortedTracks as track, idx (track.tidal_id)}
              <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
              <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
              <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
              <div
                class="search-track-row"
                class:cursor={cursor === idx}
                class:disabled={!canPlaySearchTrack(track)}
                class:in-library={track.in_library}
                data-cursor-idx={idx}
                role="button"
                tabindex={canPlaySearchTrack(track) ? 0 : -1}
                aria-disabled={!canPlaySearchTrack(track)}
                onclick={() => canPlaySearchTrack(track) && void playTidalTrackNow(toPlayable(track))}
                onmouseenter={() => { cursor = idx }}
                onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), canPlaySearchTrack(track) && void playTidalTrackNow(toPlayable(track)))}
                oncontextmenu={(e) => { e.preventDefault(); openContextMenu(e, trackContextMenu(track)) }}
              >
                <span class="col-num">
                  <span class="track-num-label">{idx + 1}</span>
                  <button
                    class="track-num-play"
                    disabled={!canPlaySearchTrack(track)}
                    onclick={(e) => { e.stopPropagation(); canPlaySearchTrack(track) && void playTidalTrackNow(toPlayable(track)) }}
                    aria-label="Play {track.title}"
                  >▶</button>
                </span>
                <span class="col-title">
                  {#if track.artwork_url}
                    <span class="row-art" style={`background-image: url('${track.artwork_url}')`}></span>
                  {:else}
                    <span class="row-art fallback" style={`background: ${letterColor(track.title)}`}>♫</span>
                  {/if}
                  <span class="row-title-text">{track.title}</span>
                  {#if track.in_library}<span class="lib-dot" aria-label="In your library"></span>{/if}
                </span>
                <span class="col-artist">
                  {#if track.artist_name && track.artist_id != null}
                    <a
                      href={`/tidal/artists/${track.artist_id}`}
                      class="subtitle-link"
                      onclick={(e) => e.stopPropagation()}
                    >{track.artist_name}</a>
                  {:else if track.artist_name}
                    {track.artist_name}
                  {:else}
                    —
                  {/if}
                </span>
                <span class="col-album">
                  {#if track.album_title && track.album_tidal_id != null}
                    <a
                      href={`/tidal/albums/${track.album_tidal_id}`}
                      class="subtitle-link"
                      onclick={(e) => e.stopPropagation()}
                    >{track.album_title}</a>
                  {:else if track.album_title}
                    {track.album_title}
                  {:else}
                    —
                  {/if}
                </span>
                <span class="col-quality">
                  {#if track.audio_quality}
                    <span class="quality-badge">{track.audio_quality.replace(/_/g, ' ')}</span>
                  {:else}
                    —
                  {/if}
                </span>
                <span class="col-duration">{formatTrackDuration(track.duration_ms)}</span>
                <span class="col-actions">
                  <button
                    class="row-btn"
                    disabled={!canPlaySearchTrack(track)}
                    onclick={(e) => { e.stopPropagation(); canPlaySearchTrack(track) && void addTidalTrackToQueue(toPlayable(track)) }}
                    title={canPlaySearchTrack(track) ? 'Add to queue' : playableSearchLabel(track)}
                    aria-label="Queue {track.title}"
                  >＋</button>
                  <button
                    class="row-btn"
                    onclick={(e) => { e.stopPropagation(); void startTidalSongRadio(track) }}
                    title="Song radio"
                    aria-label="Start radio from {track.title}"
                  >◎</button>
                  <button
                    class="row-btn"
                    onclick={(e) => { e.stopPropagation(); openContextMenu(e, trackContextMenu(track)) }}
                    title="More options"
                    aria-label="More options"
                  >⋯</button>
                </span>
              </div>
            {/each}
          </div>
        {:else}
        <ul class="tracks-list">
          {#each sortedTracks as track, idx (track.tidal_id)}
            <!-- svelte-ignore a11y_no_noninteractive_element_to_interactive_role -->
            <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
            <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
            <li
              class="track-row"
              class:cursor={cursor === idx}
              class:disabled={!canPlaySearchTrack(track)}
              data-cursor-idx={idx}
              role="button"
              tabindex={canPlaySearchTrack(track) ? 0 : -1}
              aria-disabled={!canPlaySearchTrack(track)}
              onclick={() => canPlaySearchTrack(track) && void playTidalTrackNow(toPlayable(track))}
              onmouseenter={() => { cursor = idx }}
              onkeydown={(e) => (e.key === 'Enter' || e.key === ' ') && (e.preventDefault(), canPlaySearchTrack(track) && void playTidalTrackNow(toPlayable(track)))}
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
              <span class="track-duration">{formatTrackDuration(track.duration_ms)}</span>
              <div class="row-actions">
                <button
                  class="row-btn"
                  disabled={!canPlaySearchTrack(track)}
                  onclick={(e) => { e.stopPropagation(); canPlaySearchTrack(track) && void playTidalTrackNow(toPlayable(track)) }}
                  title={playableSearchLabel(track)}
                  aria-label="Play {track.title}"
                >▶</button>
                <button
                  class="row-btn"
                  disabled={!canPlaySearchTrack(track)}
                  onclick={(e) => { e.stopPropagation(); canPlaySearchTrack(track) && void addTidalTrackToQueue(toPlayable(track)) }}
                  title={canPlaySearchTrack(track) ? 'Add to queue' : playableSearchLabel(track)}
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
        {/if}
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
              <span class="track-duration">{formatTrackDuration(track.duration_ms)}</span>
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
              <span class="track-duration">{formatTrackDuration(track.duration_ms)}</span>
            </li>
          {/each}
        </ul>
      </section>
    {/if}

    {#if filterMode !== 'all' && filterMode !== 'library' && audioResults === null}
      <div bind:this={infiniteSentinel} class="infinite-sentinel" aria-hidden="true">
        {#if loadingMore}
          <span class="infinite-spinner">Loading more…</span>
        {:else if (
          (filterMode === 'tracks' || filterMode === 'albums' || filterMode === 'artists') && !hasMoreTidal
        ) || (filterMode === 'playlists' && !hasMoreTidalPlaylists && !hasMoreSpotifyPlaylists)}
          <span class="infinite-end">— end of results —</span>
        {/if}
      </div>
    {/if}

  {/if}
</div>

<style>
  .search-page {
    width: min(100%, var(--content-width));
    margin: 0 auto;
    padding: 16px 4px 80px;
  }
  .search-header {
    max-width: var(--content-width);
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
    border-radius: var(--radius-lg);
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
    border-radius: var(--radius-md);
    padding: 4px 12px;
    font-size: var(--font-size-xs);
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
    font-size: var(--font-size-sm);
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
    border-radius: var(--radius-md);
    padding: 5px 14px;
    font-size: var(--font-size-xs);
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
    font-size: var(--font-size-sm);
    margin-top: 64px;
    text-align: center;
  }
  .search-error { color: var(--state-error); }
  .results-section { margin-bottom: 32px; max-width: var(--content-width); margin-left: auto; margin-right: auto; }
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
    border-radius: var(--radius-md);
    padding: 6px 14px;
    font-size: var(--font-size-xs);
    color: var(--text-secondary);
    cursor: pointer;
    font-family: inherit;
    transition: border-color 0.15s, color 0.15s;
  }
  .recent-chip:hover {
    border-color: var(--accent-line);
    color: var(--text-primary);
  }
  .top-result-section { margin-bottom: 28px; max-width: var(--content-width); margin-left: auto; margin-right: auto; }
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
    font-size: var(--font-size-sm);
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
  .source-chip {
    position: absolute;
    bottom: 6px;
    left: 6px;
    padding: 2px 6px;
    border-radius: 4px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--service-spotify);
    background: rgba(0, 0, 0, 0.65);
    backdrop-filter: var(--blur-base);
    -webkit-backdrop-filter: var(--blur-base);
  }
  .spotify-card { text-decoration: none; }
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
    font-size: var(--font-size-xl);
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
  .art-wrap:hover :global(.play-overlay),
  .album-card:focus-within :global(.play-overlay) {
    opacity: 1;
    transform: translateY(0);
  }
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
    font-size: var(--font-size-xs);
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
  .track-row.disabled {
    cursor: default;
    opacity: 0.62;
  }
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
    font-size: var(--font-size-md);
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
    font-size: var(--font-size-xs);
    color: var(--text-muted);
    white-space: nowrap;
  }
  .row-btn {
    background: none;
    border: none;
    color: var(--text-tertiary);
    cursor: pointer;
    font-size: var(--font-size-sm);
    padding: 4px;
    border-radius: 4px;
    opacity: 0;
    transition: opacity 0.1s, color 0.1s;
  }
  .track-row:hover .row-btn { opacity: 1; }
  .row-btn:hover { color: var(--text-primary); }
  .row-btn:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }
  .row-btn:disabled:hover { color: var(--text-tertiary); }
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

  /* Single-category grids — Trending/Library style. Override the carousel's
     horizontal-scroll layout when the matching pill is active. */
  .section-grid-albums {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 14px;
    overflow: visible;
    padding-bottom: 0;
  }
  .section-grid-albums .album-card { width: 100%; }
  .section-grid-albums .art-wrap { width: 100%; aspect-ratio: 1 / 1; }
  .section-grid-albums .album-art {
    width: 100%;
    height: 100%;
    aspect-ratio: 1 / 1;
  }

  .section-grid-artists {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(130px, 1fr));
    gap: 14px;
    overflow: visible;
    justify-items: center;
    padding-bottom: 0;
  }

  /* Library-style track table for the Tracks pill. Mirrors the visual rhythm
     of the library tracks tab: compact header row, fixed-grid columns,
     hoverable rows with inline play / queue / radio / menu actions. */
  .search-track-table {
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .search-track-header,
  .search-track-row {
    display: grid;
    grid-template-columns: 44px minmax(0, 2.2fr) minmax(0, 1.4fr) minmax(0, 1.4fr) 96px 64px 132px;
    align-items: center;
    gap: 12px;
    padding: 6px 10px;
    border-radius: 4px;
  }
  .search-track-header {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 1.4px;
    color: var(--text-muted);
    border-bottom: 1px solid var(--border-subtle);
    padding-bottom: 8px;
    margin-bottom: 4px;
  }
  .search-track-header .col-num { text-align: center; }
  .search-track-row {
    font-size: 13px;
    color: var(--text-primary);
    cursor: pointer;
    transition: background 0.12s;
  }
  .search-track-row:hover { background: var(--bg-hover); }
  .search-track-row.cursor { background: var(--bg-hover); box-shadow: inset 2px 0 0 var(--accent); }
  .search-track-row.disabled { cursor: default; opacity: 0.62; }
  .search-track-row .col-num {
    position: relative;
    text-align: center;
    color: var(--text-muted);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }
  .search-track-row .track-num-label { display: inline; }
  .search-track-row .track-num-play {
    display: none;
    background: none;
    border: none;
    color: var(--text-primary);
    font-size: var(--font-size-sm);
    cursor: pointer;
    padding: 0;
  }
  .search-track-row:hover .track-num-label { display: none; }
  .search-track-row:hover .track-num-play { display: inline; }
  .search-track-row .track-num-play:disabled { opacity: 0.4; cursor: not-allowed; }
  .search-track-row .col-title {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }
  .search-track-row .row-art {
    width: 32px;
    height: 32px;
    border-radius: 3px;
    background-size: cover;
    background-position: center;
    background-color: var(--bg-raised);
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    font-size: var(--font-size-sm);
    color: rgba(255,255,255,0.5);
  }
  .search-track-row .row-title-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .search-track-row.in-library .row-title-text { color: var(--text-primary); font-weight: 600; }
  .search-track-row .col-artist,
  .search-track-row .col-album {
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .search-track-row .col-quality .quality-badge {
    display: inline-block;
    padding: 2px 7px;
    border-radius: 99px;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    background: rgba(125, 200, 175, 0.10);
    color: var(--accent, #7dc8af);
  }
  .search-track-row .col-duration {
    color: var(--text-muted);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
    text-align: right;
  }
  .search-track-row .col-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 2px;
  }
  .search-track-row .col-actions .row-btn { opacity: 0; }
  .search-track-row:hover .col-actions .row-btn { opacity: 1; }

  .infinite-sentinel {
    min-height: 60px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--text-muted);
    font-size: var(--font-size-xs);
    padding: 16px 0;
  }
  .infinite-spinner { font-style: italic; }
  .infinite-end { letter-spacing: 0.04em; }
</style>
