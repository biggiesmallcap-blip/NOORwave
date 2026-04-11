import { writable } from 'svelte/store';

export interface TrainingState {
	isRunning: boolean;
	stage: string;
	progress: number;
	message: string;
	lastCompletedAt: string | null;
}

export const training = writable<TrainingState>({
	isRunning: false,
	stage: '',
	progress: 0,
	message: '',
	lastCompletedAt: null
});

export function handleTrainingProgress(data: { stage: string; progress: number; message: string }) {
	training.update((state) => ({
		...state,
		isRunning: true,
		stage: data.stage,
		progress: data.progress,
		message: data.message
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

export function handleTrainingFailed(errorMessage: string) {
	training.update((state) => ({
		...state,
		isRunning: false,
		stage: 'failed',
		progress: 0,
		message: errorMessage
	}));
}

export function resetTrainingState() {
	training.set({
		isRunning: false,
		stage: '',
		progress: 0,
		message: '',
		lastCompletedAt: null
	});
}
