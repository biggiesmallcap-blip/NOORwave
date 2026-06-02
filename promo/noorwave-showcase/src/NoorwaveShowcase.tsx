import './styles.css';
import {
	AbsoluteFill,
	Easing,
	Img,
	Sequence,
	interpolate,
	spring,
	staticFile,
	useCurrentFrame,
	useVideoConfig,
} from 'remotion';
import {SCENE_FRAMES} from './Root';
import {ShaderBackdrop} from './ShaderBackdrop';

type ShellMode = 'home' | 'onboarding' | 'search' | 'functions';

const mixes = [
	{title: 'Daily Discovery', subtitle: 'Based on your library', hue: 186},
	{title: 'Late Night Flow', subtitle: 'Deep cuts and smooth edges', hue: 255},
	{title: 'Focus Current', subtitle: 'Minimal, clean, steady', hue: 142},
	{title: 'Video Mix', subtitle: 'Clips queued from TIDAL', hue: 24},
];

const searchResults = [
	{kind: 'Artist', title: 'Kiasmos', meta: 'In library', color: '#72dbc8'},
	{kind: 'Album', title: 'Burial - Untrue', meta: 'Lossless', color: '#b0b3ff'},
	{kind: 'Track', title: 'Nils Frahm - Says', meta: 'Play now', color: '#f4c75f'},
	{kind: 'Playlist', title: 'Nocturne Drive', meta: 'Spotify', color: '#78e08f'},
];

const featureCards = [
	{
		label: 'DiscoverSpace',
		value: 'Explore music as a map',
		detail: 'Jump through recommendations, routes, and search nodes',
		visual: 'discover',
	},
	{
		label: 'Genre Galaxy',
		value: 'See the taxonomy',
		detail: 'Mapped genre families with search highlights and lineage',
		visual: 'genre',
	},
	{
		label: 'Remote Control',
		value: 'Phone control on LAN',
		detail: 'Browse, search, queue, and play from the same backend',
		visual: 'remote',
	},
	{
		label: 'Bit-perfect Playback',
		value: 'HiRes Lossless',
		detail: 'Native rate follow, queue intelligence, and quality badges',
		visual: 'playback',
	},
] as const;

const navItems = ['Home', 'Library', 'Search', 'Videos', 'Genre Galaxy', 'Discover', 'Playlists', 'Automix'];

const clamp = (value: number, min = 0, max = 1) => Math.min(Math.max(value, min), max);

const eased = (value: number) => Easing.bezier(0.22, 1, 0.36, 1)(clamp(value));

const fadeOpacity = (frame: number, duration = SCENE_FRAMES) => {
	const inOpacity = interpolate(frame, [0, 18], [0, 1], {
		extrapolateLeft: 'clamp',
		extrapolateRight: 'clamp',
	});
	const outOpacity = interpolate(frame, [duration - 22, duration], [1, 0], {
		extrapolateLeft: 'clamp',
		extrapolateRight: 'clamp',
	});
	return Math.min(inOpacity, outOpacity);
};

const useEnter = (delay = 0, distance = 24) => {
	const frame = useCurrentFrame();
	const {fps} = useVideoConfig();
	const value = spring({
		frame: frame - delay,
		fps,
		config: {
			damping: 24,
			stiffness: 110,
			mass: 0.9,
		},
	});
	return {
		opacity: interpolate(frame, [delay, delay + 14], [0, 1], {
			extrapolateLeft: 'clamp',
			extrapolateRight: 'clamp',
		}),
		transform: `translate3d(0, ${interpolate(value, [0, 1], [distance, 0])}px, 0)`,
	};
};

const LogoMark = ({compact = false}: {compact?: boolean}) => (
	<Img
		className={compact ? 'logo-asset compact' : 'logo-asset'}
		src={staticFile('assets/noor-logo-centered-transparent.svg')}
	/>
);

const IconMark = () => (
	<Img className="icon-asset" src={staticFile('assets/noor-icon-transparent.svg')} />
);

const WaveBackdrop = () => {
	return (
		<div className="wave-backdrop">
			<ShaderBackdrop />
			<div className="wave-grid" />
			<div className="shader-vignette" />
		</div>
	);
};

const Caption = ({eyebrow, title, body}: {eyebrow: string; title: string; body: string}) => {
	const enter = useEnter(8, 18);
	return (
		<div className="caption" style={enter}>
			<p>{eyebrow}</p>
			<h1>{title}</h1>
			<span>{body}</span>
		</div>
	);
};

const Sidebar = ({active}: {active: string}) => (
	<aside className="sidebar-replica">
		<LogoMark compact />
		<nav>
			{navItems.map((item) => (
				<div className={item === active ? 'nav-item active' : 'nav-item'} key={item}>
					<span>{item.slice(0, 1)}</span>
					{item}
				</div>
			))}
		</nav>
		<div className="server-chip">
			<i />
			Server connected
		</div>
	</aside>
);

const AppShell = ({mode, children}: {mode: ShellMode; children: React.ReactNode}) => {
	const frame = useCurrentFrame();
	const zoom = interpolate(frame, [0, SCENE_FRAMES], [0.985, 1.015], {
		extrapolateLeft: 'clamp',
		extrapolateRight: 'clamp',
	});
	const active = mode === 'search' ? 'Search' : mode === 'functions' ? 'Discover' : 'Home';
	return (
		<div className={`app-device mode-${mode}`} style={{transform: `scale(${zoom})`}}>
			<div className="device-topbar">
				<span />
				<span />
				<span />
				<strong>NOORwave</strong>
				<kbd>Ctrl K</kbd>
			</div>
			<div className="app-body">
				<Sidebar active={active} />
				<main className="app-content">{children}</main>
				<NowPlayingPanel />
			</div>
		</div>
	);
};

const AlbumArt = ({large = false}: {large?: boolean}) => {
	const frame = useCurrentFrame();
	const rotation = interpolate(frame, [0, SCENE_FRAMES], [-4, 4], {
		extrapolateLeft: 'clamp',
		extrapolateRight: 'clamp',
	});
	return (
		<div className={large ? 'album-art large' : 'album-art'} style={{transform: `rotate(${rotation}deg)`}}>
			<div className="art-orbit orbit-one" />
			<div className="art-orbit orbit-two" />
			<div className="art-hole" />
		</div>
	);
};

const NowPlayingPanel = () => {
	const frame = useCurrentFrame();
	const progress = interpolate(frame % SCENE_FRAMES, [0, SCENE_FRAMES], [18, 78], {
		extrapolateLeft: 'clamp',
		extrapolateRight: 'clamp',
	});
	return (
		<aside className="now-playing-panel">
			<AlbumArt />
			<div className="track-copy">
				<p>Now playing</p>
				<h3>Midnight Signal</h3>
				<span>NOORwave demo library</span>
			</div>
			<div className="quality-row">
				<em>HiRes Lossless</em>
				<em>24 / 96</em>
			</div>
			<div className="progress">
				<i style={{width: `${progress}%`}} />
			</div>
			<div className="transport">
				<button>Prev</button>
				<button className="play">Play</button>
				<button>Next</button>
			</div>
			<div className="queue-list">
				<strong>3 tracks queued</strong>
				<span>Automix - genre bridge</span>
				<span>Song radio - deep lane</span>
			</div>
		</aside>
	);
};

const IntroScene = () => {
	const frame = useCurrentFrame();
	const {fps} = useVideoConfig();
	const logoScale = spring({
		frame,
		fps,
		config: {
			damping: 18,
			stiffness: 90,
		},
	});
	const panelEnter = useEnter(18, 30);

	return (
		<AbsoluteFill className="scene" style={{opacity: fadeOpacity(frame)}}>
			<WaveBackdrop />
			<div className="intro-layout">
				<div className="intro-brand" style={{transform: `scale(${0.88 + logoScale * 0.12})`}}>
					<LogoMark />
					<p>Pure sound. Perfect flow.</p>
					<div className="intro-pills">
						<span>Desktop music command center</span>
						<span>Playback, search, queue, video</span>
					</div>
				</div>
				<div className="hero-now-playing" style={panelEnter}>
					<AlbumArt large />
					<div>
						<p>Currently playing</p>
						<h2>Midnight Signal</h2>
						<span>HiRes Lossless - 24 / 96 - exclusive output</span>
					</div>
					<div className="hero-meter">
						{Array.from({length: 18}, (_, index) => (
							<i
								key={index}
								style={{
									height: `${32 + Math.sin(frame * 0.18 + index * 0.7) * 22}px`,
								}}
							/>
						))}
					</div>
				</div>
			</div>
		</AbsoluteFill>
	);
};

const OnboardingScene = () => {
	const frame = useCurrentFrame();
	const cardEnter = useEnter(6, 26);
	const activeStep = Math.min(4, Math.floor(frame / 30));
	const steps = ['Welcome', 'TIDAL', 'Last.fm', 'Audio', 'Done'];

	return (
		<AbsoluteFill className="scene" style={{opacity: fadeOpacity(frame)}}>
			<WaveBackdrop />
			<Caption
				eyebrow="First run"
				title="Set up in seconds"
				body="Connect your sources, choose output quality, then land on Home with the same wallpaper intact."
			/>
				<div className="onboarding-card" style={cardEnter}>
					<LogoMark compact />
				<div className="step-dots">
					{steps.map((step, index) => (
						<span className={index <= activeStep ? 'active' : ''} key={step} />
					))}
				</div>
				<h2>{activeStep < 3 ? steps[activeStep] : activeStep === 3 ? 'How should we play it?' : "You're all set"}</h2>
				<p>
					{activeStep === 0 && 'Pure sound. Perfect flow.'}
					{activeStep === 1 && 'Connect TIDAL to sync your library in the background.'}
					{activeStep === 2 && 'Add Last.fm for listening history and richer discovery.'}
					{activeStep === 3 && 'Pick Bit-perfect or Standard playback.'}
					{activeStep === 4 && 'Open NOORwave and start listening.'}
				</p>
				<div className="onboarding-actions">
					<button>{activeStep === 4 ? 'Open NOORwave' : 'Continue'}</button>
					<button className="ghost">Set up later</button>
				</div>
			</div>
		</AbsoluteFill>
	);
};

const HomeScene = () => {
	const frame = useCurrentFrame();
	return (
		<AbsoluteFill className="scene" style={{opacity: fadeOpacity(frame)}}>
			<WaveBackdrop />
			<Caption
				eyebrow="Home"
				title="A listening dashboard"
				body="Music Mixes, Personal Radio, moods, videos, and news all sit above the persistent player."
			/>
			<AppShell mode="home">
				<section className="home-stack">
					<HeaderBlock title="Music Mixes" eyebrow="TIDAL" loading={frame < 28} />
					<div className="mix-row">
						{mixes.map((mix, index) => (
							<MixCard key={mix.title} item={mix} delay={index * 6} />
						))}
					</div>
					<HeaderBlock title="Personal Radio" eyebrow="TIDAL" loading={false} />
					<div className="radio-row">
						{['Artist Radio', 'Discovery Radio', 'Focus Radio'].map((name, index) => (
							<div className="radio-card" key={name} style={{transform: `translateY(${Math.sin((frame + index * 12) * 0.04) * 4}px)`}}>
								<span />
								<div>
									<strong>{name}</strong>
									<small>Ready to play</small>
								</div>
							</div>
						))}
					</div>
				</section>
			</AppShell>
		</AbsoluteFill>
	);
};

const HeaderBlock = ({eyebrow, title, loading}: {eyebrow: string; title: string; loading: boolean}) => (
	<div className="section-header-replica">
		<div>
			<p>{eyebrow}</p>
			<h2>{title}</h2>
		</div>
		{loading && <span>Loading...</span>}
	</div>
);

const MixCard = ({item, delay}: {item: (typeof mixes)[number]; delay: number}) => {
	const frame = useCurrentFrame();
	const enter = eased((frame - delay) / 22);
	return (
		<div
			className="mix-card-replica"
			style={{
				opacity: enter,
				transform: `translateY(${(1 - enter) * 18}px)`,
			}}
		>
			<div className="mix-art" style={{background: `linear-gradient(135deg, hsl(${item.hue} 72% 52%), hsl(${item.hue + 50} 74% 24%))`}}>
				<span>Play</span>
			</div>
			<strong>{item.title}</strong>
			<small>{item.subtitle}</small>
		</div>
	);
};

const SearchScene = () => {
	const frame = useCurrentFrame();
	const queryProgress = clamp((frame - 28) / 55);
	const typed = 'burial radio'.slice(0, Math.floor(queryProgress * 'burial radio'.length));
	const slashProgress = clamp((frame - 94) / 36);
	const slash = '/radio burial'.slice(0, Math.floor(slashProgress * '/radio burial'.length));
	const query = frame < 94 ? typed : slash;

	return (
		<AbsoluteFill className="scene" style={{opacity: fadeOpacity(frame)}}>
			<WaveBackdrop />
			<Caption
				eyebrow="Ctrl + K"
				title="Search or run a command"
				body="Find local and TIDAL results, play now, queue next, start radio, or jump across the app."
			/>
			<AppShell mode="search">
				<div className="search-focus">
					<div className="palette-panel-replica">
						<div className="palette-input">
							<span>{query.startsWith('/') ? '/' : 'K'}</span>
							<strong>{query || 'Search or type / for commands'}</strong>
							<kbd>Esc</kbd>
						</div>
						<ul>
							{query.startsWith('/') ? (
								<>
									<CommandRow active prefix="/radio" text="Start radio from current or query" />
									<CommandRow prefix="/play" text="Play first search result" />
									<CommandRow prefix="/queue" text="Add first result to queue" />
									<CommandRow prefix="/jump" text="Navigate to a page" />
								</>
							) : (
								searchResults.map((item, index) => (
									<li className={index === 0 ? 'active' : ''} key={item.title}>
										<i style={{background: item.color}} />
										<div>
											<strong>{item.title}</strong>
											<span>{item.meta}</span>
										</div>
										<em>{item.kind}</em>
									</li>
								))
							)}
						</ul>
					</div>
				</div>
			</AppShell>
		</AbsoluteFill>
	);
};

const CommandRow = ({prefix, text, active = false}: {prefix: string; text: string; active?: boolean}) => (
	<li className={active ? 'active command-row' : 'command-row'}>
		<i />
		<div>
			<strong>{prefix}</strong>
			<span>{text}</span>
		</div>
		<em>Command</em>
	</li>
);

const FeatureVisual = ({type}: {type: (typeof featureCards)[number]['visual']}) => {
	if (type === 'remote') {
		return (
			<div className="feature-visual visual-remote" aria-hidden="true">
				<div className="phone-frame">
					<span />
					<strong>Remote</strong>
					<i />
					<em>Search</em>
					<b>Play</b>
				</div>
				<div className="remote-signal">
					<span />
					<span />
					<span />
				</div>
			</div>
		);
	}
	if (type === 'genre') {
		return (
			<div className="feature-visual visual-genre" aria-hidden="true">
				<span className="genre-node main" />
				<span className="genre-node node-a" />
				<span className="genre-node node-b" />
				<span className="genre-node node-c" />
				<span className="genre-line line-a" />
				<span className="genre-line line-b" />
				<span className="genre-line line-c" />
			</div>
		);
	}
	if (type === 'discover') {
		return (
			<div className="feature-visual visual-discover" aria-hidden="true">
				{Array.from({length: 14}, (_, index) => (
					<span key={index} style={{transform: `rotate(${index * 25.7}deg) translateX(${28 + (index % 4) * 11}px)`}} />
				))}
				<i />
			</div>
		);
	}
	return (
		<div className="feature-visual visual-playback" aria-hidden="true">
			{Array.from({length: 9}, (_, index) => (
				<span key={index} style={{height: `${18 + Math.sin(index * 0.9) * 12 + index * 2}px`}} />
			))}
		</div>
	);
};

const FunctionsScene = () => {
	const frame = useCurrentFrame();
	return (
		<AbsoluteFill className="scene" style={{opacity: fadeOpacity(frame)}}>
			<WaveBackdrop />
			<Caption
				eyebrow="Functions"
				title="Discovery, genres, remote"
				body="DiscoverSpace, Genre Galaxy, and LAN remote control are first-class surfaces beside playback."
			/>
			<AppShell mode="functions">
				<div className="features-grid">
					{featureCards.map((feature, index) => {
						const enter = eased((frame - index * 10) / 24);
						return (
							<div
								className="feature-card"
								key={feature.label}
								style={{
									opacity: enter,
									transform: `translateY(${(1 - enter) * 22}px)`,
								}}
							>
								<p>{feature.label}</p>
								<h3>{feature.value}</h3>
								<span>{feature.detail}</span>
								<FeatureVisual type={feature.visual} />
								<i />
							</div>
						);
					})}
				</div>
				<div className="final-cta">
					<IconMark />
					<strong>NOORwave</strong>
					<span>Pure sound. Perfect flow.</span>
				</div>
			</AppShell>
		</AbsoluteFill>
	);
};

export const NoorwaveShowcase = () => (
	<AbsoluteFill className="video-root">
		<Sequence from={0} durationInFrames={SCENE_FRAMES}>
			<IntroScene />
		</Sequence>
		<Sequence from={SCENE_FRAMES} durationInFrames={SCENE_FRAMES}>
			<OnboardingScene />
		</Sequence>
		<Sequence from={SCENE_FRAMES * 2} durationInFrames={SCENE_FRAMES}>
			<HomeScene />
		</Sequence>
		<Sequence from={SCENE_FRAMES * 3} durationInFrames={SCENE_FRAMES}>
			<SearchScene />
		</Sequence>
		<Sequence from={SCENE_FRAMES * 4} durationInFrames={SCENE_FRAMES}>
			<FunctionsScene />
		</Sequence>
	</AbsoluteFill>
);
