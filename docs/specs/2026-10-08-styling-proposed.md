# Styling proposal (October 2026 design audit): adopted and implemented

Status: done. This was the draft design system from the October 2026 design
audit. It was adopted as `frontend/STYLING.md` in commit `0c8ce74d`, which is
the source of truth; this file only records how the proposal was carried out.
Read `frontend/STYLING.md` for the rules themselves.

## What was built

| Proposal area | Commits |
| --- | --- |
| Foundations: tokens, `.t-*` roles, focus ring, contrast floor, no hover lift | `0c8ce74d` |
| Shared components: CommandHeader, ScopeTabs, Segmented, FilterChip, Dropdown, ActionBar, borderless DetailHero | `0fcbc243`, `cf46f007`, `d34901a4`, `0acf0149`, `29564acf`, `6b638d37`, `48442e72` |
| One title per section, no eyebrow over it; mural source lines removed | `a6b6da16` |
| Page frames: shared workspace edge, one page-title size | `f97a2a5c`, `d1555478` |
| Boundaries: Settings as a form; tiles, empty states and route panels without boxes | `4d79301c`, `2dd35c3d` |
| Library: Songs as liked songs with a scope setting, command header, counts, permanent toolbar, calm rows | `32a34154`, `136ecf4b`, `83e2485e` |
| Album: quick view link and shared ActionBar, liner notes (work grouping, title step-down, no repeated metadata) | `d04df260`, `b0341675`, `7670e458` |
| Artist: Stage layout, counts once, tabbed discography, durations on every row | `64850461` |
| Search: top result beside five songs, cursor kept in view, library marks as rings | `4c1e8aba` |
| Playlist and Spotify heroes on ActionBar | `61365611`, `d1555478` |
| Mix page replacing Automix and DJ; crossfade in Settings > Playback | `58587704`, `f8fc885a`, `88248f41`, `05b535d8` |
| Home order with Jump back in | `6997f4ad` |
| States: ErrorState with Retry, empty searches offer TIDAL, skeleton delay | `b69bd138`, `d283288a` |
| Motion: in-app Reduce motion, motion-token lint warning | `0160d188`, `a7899d66` |
| Narrow windows: icon-rail sidebar at 680-899px | `8acb505c` |
| P2 polish: chart action, mood titles, smart playlist validation | `b8973fc8` |
| Adoption status in STYLING.md, remaining calls in FOLLOWUPS | `ff93f49a` |

## Not part of this proposal's delivery

Product calls and backend work the audit left open are tracked in
`FOLLOWUPS.md` ("design: system adoption"): Search Enter behaviour, navigation
groups and naming, Library mural strips, the A to Z index, the search
relevance floor, album label metadata, and the remaining raw animation
durations.
