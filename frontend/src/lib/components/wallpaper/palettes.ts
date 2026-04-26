// Coordinated colour combos applied to both UI accent vars and wallpaper shader uniforms.

export type PaletteId = 'iris' | 'sunset' | 'verdant' | 'cosmos' | 'mono'
                      | 'ember' | 'arctic' | 'sakura' | 'abyss' | 'citrus';

export type Rgb = [number, number, number];

export interface Palette {
	id: PaletteId;
	label: string;
	sublabel: string;

	// CSS custom properties written onto <html> when this palette is active.
	ui: {
		accent: string;
		accentStrong: string;
		accentSoft: string;
		accentLine: string;
		accentGlow: string;
	};

	// Four colour slots fed to shaders as uniforms u_color1..u_color4.
	// Aurora uses all four. Nebula uses 1-3. Grid uses 1-2. Chrome and Topo use
	// only c1 as a subtle final-pixel tint.
	shader: {
		c1: Rgb;
		c2: Rgb;
		c3: Rgb;
		c4: Rgb;
	};
}

export const PALETTES: Palette[] = [
	{
		id: 'iris',
		label: 'Iris',
		sublabel: 'Indigo · magenta · cyan — the original',
		ui: {
			accent: '#7c80ff',
			accentStrong: '#b0b3ff',
			accentSoft: 'rgba(124, 128, 255, 0.14)',
			accentLine: 'rgba(124, 128, 255, 0.28)',
			accentGlow: 'rgba(124, 128, 255, 0.22)'
		},
		shader: {
			c1: [0.08, 0.42, 0.78],
			c2: [0.76, 0.22, 0.95],
			c3: [1.0, 0.62, 0.32],
			c4: [0.1, 0.95, 0.78]
		}
	},
	{
		id: 'sunset',
		label: 'Sunset',
		sublabel: 'Coral · amber · violet',
		ui: {
			accent: '#ff7a4d',
			accentStrong: '#ffb27a',
			accentSoft: 'rgba(255, 122, 77, 0.14)',
			accentLine: 'rgba(255, 122, 77, 0.30)',
			accentGlow: 'rgba(255, 122, 77, 0.24)'
		},
		shader: {
			c1: [0.22, 0.06, 0.36],
			c2: [1.0, 0.48, 0.32],
			c3: [1.0, 0.82, 0.32],
			c4: [0.62, 0.28, 0.92]
		}
	},
	{
		id: 'verdant',
		label: 'Verdant',
		sublabel: 'Mint · teal · jade',
		ui: {
			accent: '#5dd7a8',
			accentStrong: '#8ee6c1',
			accentSoft: 'rgba(93, 215, 168, 0.14)',
			accentLine: 'rgba(93, 215, 168, 0.30)',
			accentGlow: 'rgba(93, 215, 168, 0.24)'
		},
		shader: {
			c1: [0.04, 0.22, 0.20],
			c2: [0.18, 0.62, 0.52],
			c3: [0.62, 0.94, 0.42],
			c4: [0.22, 0.96, 0.78]
		}
	},
	{
		id: 'cosmos',
		label: 'Cosmos',
		sublabel: 'Deep purple · magenta · cyan',
		ui: {
			accent: '#d65cff',
			accentStrong: '#e89bff',
			accentSoft: 'rgba(214, 92, 255, 0.14)',
			accentLine: 'rgba(214, 92, 255, 0.30)',
			accentGlow: 'rgba(214, 92, 255, 0.24)'
		},
		shader: {
			c1: [0.25, 0.08, 0.55],
			c2: [0.9, 0.3, 0.55],
			c3: [1.0, 0.7, 0.32],
			c4: [0.1, 0.6, 0.9]
		}
	},
	{
		id: 'mono',
		label: 'Mono',
		sublabel: 'Ink · steel · silver',
		ui: {
			accent: '#c0c4cc',
			accentStrong: '#e8eaee',
			accentSoft: 'rgba(192, 196, 204, 0.12)',
			accentLine: 'rgba(192, 196, 204, 0.26)',
			accentGlow: 'rgba(192, 196, 204, 0.20)'
		},
		shader: {
			c1: [0.1, 0.11, 0.14],
			c2: [0.45, 0.48, 0.55],
			c3: [0.78, 0.8, 0.85],
			c4: [0.94, 0.96, 1.0]
		}
	},
	{
		id: 'ember',
		label: 'Ember',
		sublabel: 'Crimson · amber · rose',
		ui: {
			accent: '#ff4d6a',
			accentStrong: '#ff8a9a',
			accentSoft: 'rgba(255, 77, 106, 0.14)',
			accentLine: 'rgba(255, 77, 106, 0.28)',
			accentGlow: 'rgba(255, 77, 106, 0.24)'
		},
		shader: {
			c1: [0.50, 0.03, 0.10],
			c2: [1.0, 0.28, 0.10],
			c3: [1.0, 0.72, 0.10],
			c4: [0.78, 0.08, 0.32]
		}
	},
	{
		id: 'arctic',
		label: 'Arctic',
		sublabel: 'Ice · cyan · steel',
		ui: {
			accent: '#5de8fa',
			accentStrong: '#a2f0fb',
			accentSoft: 'rgba(93, 232, 250, 0.13)',
			accentLine: 'rgba(93, 232, 250, 0.28)',
			accentGlow: 'rgba(93, 232, 250, 0.20)'
		},
		shader: {
			c1: [0.04, 0.14, 0.34],
			c2: [0.22, 0.68, 0.92],
			c3: [0.68, 0.94, 1.0],
			c4: [0.14, 0.46, 0.76]
		}
	},
	{
		id: 'sakura',
		label: 'Sakura',
		sublabel: 'Plum · blossom · blush',
		ui: {
			accent: '#ff80b0',
			accentStrong: '#ffb3cc',
			accentSoft: 'rgba(255, 128, 176, 0.14)',
			accentLine: 'rgba(255, 128, 176, 0.28)',
			accentGlow: 'rgba(255, 128, 176, 0.22)'
		},
		shader: {
			c1: [0.25, 0.06, 0.18],
			c2: [0.92, 0.48, 0.68],
			c3: [1.0, 0.80, 0.88],
			c4: [0.58, 0.18, 0.48]
		}
	},
	{
		id: 'abyss',
		label: 'Abyss',
		sublabel: 'Midnight · teal · cerulean',
		ui: {
			accent: '#3ec9dc',
			accentStrong: '#76dde9',
			accentSoft: 'rgba(62, 201, 220, 0.13)',
			accentLine: 'rgba(62, 201, 220, 0.27)',
			accentGlow: 'rgba(62, 201, 220, 0.20)'
		},
		shader: {
			c1: [0.02, 0.06, 0.20],
			c2: [0.06, 0.28, 0.52],
			c3: [0.18, 0.62, 0.76],
			c4: [0.02, 0.40, 0.56]
		}
	},
	{
		id: 'citrus',
		label: 'Citrus',
		sublabel: 'Lime · lemon · tangerine',
		ui: {
			accent: '#e8d800',
			accentStrong: '#f5ea50',
			accentSoft: 'rgba(232, 216, 0, 0.12)',
			accentLine: 'rgba(232, 216, 0, 0.26)',
			accentGlow: 'rgba(232, 216, 0, 0.18)'
		},
		shader: {
			c1: [0.10, 0.24, 0.02],
			c2: [0.44, 0.82, 0.06],
			c3: [0.98, 0.88, 0.10],
			c4: [1.0, 0.52, 0.06]
		}
	}
];

export const DEFAULT_PALETTE: PaletteId = 'iris';

export function paletteById(id: PaletteId): Palette {
	return PALETTES.find((p) => p.id === id) ?? PALETTES[0];
}
