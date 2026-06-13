// Coordinated colour combos applied to both UI accent vars and wallpaper shader uniforms.

export type PaletteId = 'iris' | 'sunset' | 'verdant' | 'cosmos' | 'mono'
                      | 'ember' | 'arctic' | 'sakura' | 'abyss' | 'citrus'
                      | 'slate' | 'paper' | 'moss' | 'plum' | 'acid' | 'neon'
                      | 'futuro' | 'constr' | 'obsidian' | 'carbon' | 'blackout'
                      | 'nocturne'
                      | 'daylight' | 'linen' | 'meadow' | 'blush';

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
	},
	{
		id: 'slate',
		label: 'Slate',
		sublabel: 'Charcoal / silver / sky',
		ui: {
			accent: '#6bbdf2',
			accentStrong: '#a5d9ff',
			accentSoft: 'rgba(107, 189, 242, 0.13)',
			accentLine: 'rgba(107, 189, 242, 0.28)',
			accentGlow: 'rgba(107, 189, 242, 0.20)'
		},
		shader: {
			c1: [0.039, 0.047, 0.063],
			c2: [0.85, 0.87, 0.92],
			c3: [0.42, 0.74, 0.95],
			c4: [0.20, 0.28, 0.36]
		}
	},
	{
		id: 'paper',
		label: 'Paper',
		sublabel: 'Warm white / ink / coral',
		ui: {
			accent: '#d9594d',
			accentStrong: '#ef8f82',
			accentSoft: 'rgba(217, 89, 77, 0.13)',
			accentLine: 'rgba(217, 89, 77, 0.28)',
			accentGlow: 'rgba(217, 89, 77, 0.20)'
		},
		shader: {
			c1: [0.96, 0.95, 0.93],
			c2: [0.10, 0.10, 0.12],
			c3: [0.85, 0.35, 0.30],
			c4: [0.72, 0.65, 0.56]
		}
	},
	{
		id: 'moss',
		label: 'Moss',
		sublabel: 'Forest / lichen / leaf',
		ui: {
			accent: '#8cd973',
			accentStrong: '#b6eba5',
			accentSoft: 'rgba(140, 217, 115, 0.13)',
			accentLine: 'rgba(140, 217, 115, 0.28)',
			accentGlow: 'rgba(140, 217, 115, 0.20)'
		},
		shader: {
			c1: [0.07, 0.10, 0.09],
			c2: [0.78, 0.86, 0.78],
			c3: [0.55, 0.85, 0.45],
			c4: [0.18, 0.34, 0.20]
		}
	},
	{
		id: 'plum',
		label: 'Plum',
		sublabel: 'Aubergine / lilac / orchid',
		ui: {
			accent: '#d980f2',
			accentStrong: '#edb6fb',
			accentSoft: 'rgba(217, 128, 242, 0.13)',
			accentLine: 'rgba(217, 128, 242, 0.28)',
			accentGlow: 'rgba(217, 128, 242, 0.20)'
		},
		shader: {
			c1: [0.09, 0.05, 0.12],
			c2: [0.90, 0.85, 0.95],
			c3: [0.85, 0.50, 0.95],
			c4: [0.34, 0.12, 0.42]
		}
	},
	{
		id: 'acid',
		label: 'Acid',
		sublabel: 'Violet / yellow / magenta',
		ui: {
			accent: '#f2ff33',
			accentStrong: '#fbff9a',
			accentSoft: 'rgba(242, 255, 51, 0.12)',
			accentLine: 'rgba(242, 255, 51, 0.26)',
			accentGlow: 'rgba(242, 255, 51, 0.18)'
		},
		shader: {
			c1: [0.06, 0.00, 0.10],
			c2: [0.95, 1.00, 0.20],
			c3: [0.95, 0.10, 0.65],
			c4: [0.20, 0.00, 0.34]
		}
	},
	{
		id: 'neon',
		label: 'Neon',
		sublabel: 'Near black / cyan / pink',
		ui: {
			accent: '#33ffd9',
			accentStrong: '#8dffee',
			accentSoft: 'rgba(51, 255, 217, 0.12)',
			accentLine: 'rgba(51, 255, 217, 0.28)',
			accentGlow: 'rgba(51, 255, 217, 0.20)'
		},
		shader: {
			c1: [0.02, 0.02, 0.06],
			c2: [0.20, 1.00, 0.85],
			c3: [1.00, 0.25, 0.85],
			c4: [0.06, 0.18, 0.32]
		}
	},
	{
		id: 'futuro',
		label: 'Futuro',
		sublabel: 'Cream / vermilion / black',
		ui: {
			accent: '#cc2e1a',
			accentStrong: '#eb715f',
			accentSoft: 'rgba(204, 46, 26, 0.13)',
			accentLine: 'rgba(204, 46, 26, 0.28)',
			accentGlow: 'rgba(204, 46, 26, 0.20)'
		},
		shader: {
			c1: [0.93, 0.89, 0.80],
			c2: [0.08, 0.06, 0.05],
			c3: [0.80, 0.18, 0.10],
			c4: [0.34, 0.30, 0.24]
		}
	},
	{
		id: 'constr',
		label: 'Constr',
		sublabel: 'Constructivist red / cream / black',
		ui: {
			accent: '#c71f1a',
			accentStrong: '#ed655f',
			accentSoft: 'rgba(199, 31, 26, 0.13)',
			accentLine: 'rgba(199, 31, 26, 0.28)',
			accentGlow: 'rgba(199, 31, 26, 0.20)'
		},
		shader: {
			c1: [0.94, 0.91, 0.86],
			c2: [0.78, 0.12, 0.10],
			c3: [0.05, 0.05, 0.08],
			c4: [0.32, 0.26, 0.20]
		}
	},
	{
		id: 'obsidian',
		label: 'Obsidian',
		sublabel: 'Black glass / graphite / frost',
		ui: {
			accent: '#9aa4b2',
			accentStrong: '#d7dce4',
			accentSoft: 'rgba(154, 164, 178, 0.12)',
			accentLine: 'rgba(154, 164, 178, 0.26)',
			accentGlow: 'rgba(154, 164, 178, 0.18)'
		},
		shader: {
			c1: [0.00, 0.00, 0.01],
			c2: [0.08, 0.09, 0.11],
			c3: [0.38, 0.42, 0.48],
			c4: [0.84, 0.87, 0.92]
		}
	},
	{
		id: 'carbon',
		label: 'Carbon',
		sublabel: 'Pitch black / nickel / signal amber',
		ui: {
			accent: '#f0a83a',
			accentStrong: '#ffd18a',
			accentSoft: 'rgba(240, 168, 58, 0.12)',
			accentLine: 'rgba(240, 168, 58, 0.25)',
			accentGlow: 'rgba(240, 168, 58, 0.18)'
		},
		shader: {
			c1: [0.00, 0.00, 0.00],
			c2: [0.05, 0.05, 0.06],
			c3: [0.34, 0.35, 0.36],
			c4: [0.94, 0.58, 0.18]
		}
	},
	{
		id: 'blackout',
		label: 'Blackout',
		sublabel: 'Near black / cyan / ultraviolet',
		ui: {
			accent: '#36d9ff',
			accentStrong: '#9beeff',
			accentSoft: 'rgba(54, 217, 255, 0.12)',
			accentLine: 'rgba(54, 217, 255, 0.27)',
			accentGlow: 'rgba(54, 217, 255, 0.19)'
		},
		shader: {
			c1: [0.00, 0.00, 0.02],
			c2: [0.01, 0.03, 0.07],
			c3: [0.12, 0.80, 1.00],
			c4: [0.44, 0.18, 0.92]
		}
	},
	{
		id: 'nocturne',
		label: 'Nocturne',
		sublabel: 'Ink black / wine / blue flame',
		ui: {
			accent: '#8aa8ff',
			accentStrong: '#c0ceff',
			accentSoft: 'rgba(138, 168, 255, 0.12)',
			accentLine: 'rgba(138, 168, 255, 0.27)',
			accentGlow: 'rgba(138, 168, 255, 0.19)'
		},
		shader: {
			c1: [0.01, 0.00, 0.02],
			c2: [0.08, 0.02, 0.07],
			c3: [0.44, 0.07, 0.20],
			c4: [0.18, 0.36, 0.95]
		}
	},
	// Light-mode-tuned schemes: airy wallpaper bases (high c1) and accents picked
	// to read on white surfaces. The wallpaper layer is dimmed in light mode, so a
	// light base reads as a soft tint instead of the muddy haze a near-black base
	// turns into.
	{
		id: 'daylight',
		label: 'Daylight',
		sublabel: 'Sky · cloud · gold',
		ui: {
			accent: '#2f6fed',
			accentStrong: '#5b90f5',
			accentSoft: 'rgba(47, 111, 237, 0.12)',
			accentLine: 'rgba(47, 111, 237, 0.26)',
			accentGlow: 'rgba(47, 111, 237, 0.20)'
		},
		shader: {
			c1: [0.91, 0.95, 0.99],
			c2: [0.52, 0.73, 0.97],
			c3: [0.99, 0.85, 0.52],
			c4: [0.80, 0.84, 0.97]
		}
	},
	{
		id: 'linen',
		label: 'Linen',
		sublabel: 'Warm paper · sand · terracotta',
		ui: {
			accent: '#c25a3a',
			accentStrong: '#e08a6f',
			accentSoft: 'rgba(194, 90, 58, 0.13)',
			accentLine: 'rgba(194, 90, 58, 0.28)',
			accentGlow: 'rgba(194, 90, 58, 0.20)'
		},
		shader: {
			c1: [0.97, 0.94, 0.89],
			c2: [0.87, 0.79, 0.67],
			c3: [0.86, 0.46, 0.30],
			c4: [0.76, 0.80, 0.70]
		}
	},
	{
		id: 'meadow',
		label: 'Meadow',
		sublabel: 'Pale mint · sage · leaf',
		ui: {
			accent: '#2e9e6a',
			accentStrong: '#5fc491',
			accentSoft: 'rgba(46, 158, 106, 0.13)',
			accentLine: 'rgba(46, 158, 106, 0.28)',
			accentGlow: 'rgba(46, 158, 106, 0.20)'
		},
		shader: {
			c1: [0.93, 0.97, 0.94],
			c2: [0.64, 0.84, 0.72],
			c3: [0.46, 0.80, 0.50],
			c4: [0.72, 0.89, 0.93]
		}
	},
	{
		id: 'blush',
		label: 'Blush',
		sublabel: 'Pale blush · rose · lilac',
		ui: {
			accent: '#d6457f',
			accentStrong: '#ea7ba6',
			accentSoft: 'rgba(214, 69, 127, 0.13)',
			accentLine: 'rgba(214, 69, 127, 0.28)',
			accentGlow: 'rgba(214, 69, 127, 0.20)'
		},
		shader: {
			c1: [0.99, 0.94, 0.96],
			c2: [0.97, 0.76, 0.84],
			c3: [1.00, 0.72, 0.64],
			c4: [0.83, 0.76, 0.95]
		}
	}
];

export const DEFAULT_PALETTE: PaletteId = 'futuro';

export function paletteById(id: PaletteId): Palette {
	return PALETTES.find((p) => p.id === id) ?? PALETTES[0];
}

export function rgbCss(c: Rgb): string {
	return `rgb(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)})`;
}

export function rgbaCss(c: Rgb, alpha: number): string {
	return `rgba(${Math.round(c[0] * 255)}, ${Math.round(c[1] * 255)}, ${Math.round(c[2] * 255)}, ${alpha})`;
}
