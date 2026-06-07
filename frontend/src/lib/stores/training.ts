import { writable } from 'svelte/store';

export interface TrainingState {
	isRunning: boolean;
	stage: string;
	progress: number;
	message: string;
	current_track_id: number | null;
	current_track_title: string | null;
	tracks_done: number;
	tracks_total: number;
	lastCompletedAt: string | null;
}

export const training = writable<TrainingState>({
	isRunning: false,
	stage: '',
	progress: 0,
	message: '',
	current_track_id: null,
	current_track_title: null,
	tracks_done: 0,
	tracks_total: 0,
	lastCompletedAt: null
});

export function handleTrainingProgress(data: { stage: string; progress: number; message: string; current_track_id: number | null; current_track_title: string | null; tracks_done: number; tracks_total: number }) {
	training.update((state) => ({
		...state,
		isRunning: true,
		stage: data.stage,
		progress: data.progress,
		message: data.message,
		current_track_id: data.current_track_id,
		current_track_title: data.current_track_title,
		tracks_done: data.tracks_done,
		tracks_total: data.tracks_total
	}));
}

export function handleTrainingComplete() {
	training.update((state) => ({
		...state,
		isRunning: false,
		stage: 'complete',
		progress: 1.0,
		message: 'Training complete',
		lastCompletedAt: new Date().toISOString()
	}));
}
