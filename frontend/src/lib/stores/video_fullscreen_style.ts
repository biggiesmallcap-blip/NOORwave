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
