import {Composition} from 'remotion';
import {NoorwaveShowcase} from './NoorwaveShowcase';

export const FPS = 30;
export const SCENE_SECONDS = 5;
export const SCENE_FRAMES = FPS * SCENE_SECONDS;
export const SCENE_COUNT = 5;
export const VIDEO_FRAMES = SCENE_FRAMES * SCENE_COUNT;

export const RemotionRoot = () => {
	return (
		<Composition
			id="NoorwaveShowcase"
			component={NoorwaveShowcase}
			durationInFrames={VIDEO_FRAMES}
			fps={FPS}
			width={1920}
			height={1080}
		/>
	);
};
