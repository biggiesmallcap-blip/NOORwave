import { createPersistedStore, oneOf } from './persisted';

/** How a video goes fullscreen in the desktop app.
 *  - grow: the window switches (one native step), then the video grows into it.
 *  - dim: the page dims around the video first, hiding the relayout.
 *  - classic: the browser Fullscreen API through Tauri's own window switch. */
export type VideoFullscreenStyle = 'grow' | 'dim' | 'classic';

export const VIDEO_FULLSCREEN_STYLES: { value: VideoFullscreenStyle; label: string }[] = [
	{ value: 'grow', label: 'Grow' },
	{ value: 'dim', label: 'Dim, then grow' },
	{ value: 'classic', label: 'Classic' },
];

export const videoFullscreenStyle = createPersistedStore<VideoFullscreenStyle>('noor-video-fullscreen', 'grow', {
	parse: oneOf(VIDEO_FULLSCREEN_STYLES.map((s) => s.value)),
});

/** Milliseconds; whole numbers clamped into range, anything else ignored. */
function msOptions(min: number, max: number) {
	return {
		parse: (raw: string) => {
			const value = Math.round(Number(raw));
			return Number.isFinite(value) ? Math.min(max, Math.max(min, value)) : undefined;
		},
		serialize: String,
	};
}

/** How long the video takes to grow into fullscreen and back. */
export const VIDEO_FULLSCREEN_GROW_MIN = 100;
export const VIDEO_FULLSCREEN_GROW_MAX = 600;
export const videoFullscreenGrowMs = createPersistedStore(
	'noor-video-fullscreen-grow-ms',
	240,
	msOptions(VIDEO_FULLSCREEN_GROW_MIN, VIDEO_FULLSCREEN_GROW_MAX),
);

/** How long "Dim, then grow" takes to fade the page to black. */
export const VIDEO_FULLSCREEN_DIM_MIN = 30;
export const VIDEO_FULLSCREEN_DIM_MAX = 300;
export const videoFullscreenDimMs = createPersistedStore(
	'noor-video-fullscreen-dim-ms',
	90,
	msOptions(VIDEO_FULLSCREEN_DIM_MIN, VIDEO_FULLSCREEN_DIM_MAX),
);
