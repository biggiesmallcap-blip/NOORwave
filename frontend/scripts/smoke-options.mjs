export const DEFAULT_PHASES = ['shell', 'links', 'menus', 'artist-album', 'styling', 'performance'];

export const PHASE_ROUTES = {
	shell: ['/', '/library', '/search', '/settings'],
	links: ['/', '/library', '/search', '/playlists', '/videos', '/settings'],
	menus: ['/', '/library', '/search', '/automix', '/duplicates'],
	'artist-album': ['/library', '/search'],
	styling: ['/', '/library', '/search', '/settings', '/automix', '/genres'],
	performance: ['/', '/library', '/search', '/analytics', '/discoverspace', '/videos'],
};

const VALID_PHASES = new Set(['full', ...DEFAULT_PHASES]);

function readArg(args, flag, fallback = undefined) {
	const index = args.indexOf(flag);
	if (index < 0) return fallback;
	const value = args[index + 1];
	return value && !value.startsWith('--') ? value : fallback;
}

function hasFlag(args, flag) {
	return args.includes(flag);
}

function parseNumberArg(args, flag) {
	const raw = readArg(args, flag);
	if (raw === undefined) return undefined;
	const parsed = Number(raw);
	if (!Number.isInteger(parsed) || parsed < 1) {
		throw new Error(`${flag} must be a positive integer`);
	}
	return parsed;
}

function parseViewport(raw) {
	const match = /^(\d+)x(\d+)$/.exec(raw);
	if (!match) {
		throw new Error(`viewport must be WxH, got "${raw}"`);
	}
	return { width: Number(match[1]), height: Number(match[2]) };
}

export function parseSmokeOptions(args, env = process.env) {
	const phase = readArg(args, '--phase', 'full');
	if (!VALID_PHASES.has(phase)) {
		throw new Error(`phase must be one of ${[...VALID_PHASES].join(', ')}`);
	}

	return {
		phase,
		frontend: env.NOOR_FRONTEND ?? 'http://localhost:5173',
		backend: env.NOOR_BACKEND ?? 'http://localhost:3334',
		viewport: parseViewport(readArg(args, '--viewport', '1920x1080')),
		artistId: parseNumberArg(args, '--artist'),
		albumId: parseNumberArg(args, '--album'),
		trackId: parseNumberArg(args, '--track'),
		playlistId: parseNumberArg(args, '--playlist'),
		query: readArg(args, '--query', 'julio iglesias'),
		shotsSuffix: readArg(args, '--shots-suffix', ''),
		headless: !hasFlag(args, '--headed'),
		keepOpen: hasFlag(args, '--keep-open'),
		destructive: hasFlag(args, '--destructive'),
	};
}

export function routesForPhase(phase) {
	if (phase === 'full') {
		return unique(DEFAULT_PHASES.flatMap((name) => PHASE_ROUTES[name]));
	}
	if (!PHASE_ROUTES[phase]) {
		throw new Error(`unknown smoke phase "${phase}"`);
	}
	return [...PHASE_ROUTES[phase]];
}

export function buildSmokeRoutes(options) {
	const routes = routesForPhase(options.phase);
	if (options.artistId) routes.push(`/artists/${options.artistId}`);
	if (options.albumId) routes.push(`/albums/${options.albumId}`);
	if (options.playlistId) routes.push(`/spotify-playlist/${options.playlistId}`);
	return unique(routes);
}

export function redactSensitiveText(text) {
	return String(text)
		.replace(/(token=)[^&\s"'<>]+/gi, '$1[redacted]')
		.replace(/(Authorization:\s*Bearer\s+)[^\s"'<>]+/gi, '$1[redacted]')
		.replace(/(noor_api_token["']?\s*[:=]\s*["'])[^"']+/gi, '$1[redacted]');
}

function unique(items) {
	return [...new Set(items)];
}
