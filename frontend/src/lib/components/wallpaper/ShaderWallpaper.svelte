<script lang="ts">
	import { onMount } from 'svelte';
	import { palette } from '$lib/stores/palette';
	import { DEFAULT_PALETTE, paletteById, type PaletteId } from '$lib/components/wallpaper/palettes';
	import { currentTrackFeatures, isPlaying, position } from '$lib/stores/player';
	import {
		wallpaperReactive,
		wallpaperReactivity,
		wallpaperBeatSmoothing,
		wallpaperReduceMotionActive,
		wallpaperColorSource,
		wallpaperIdle
	} from '$lib/stores/wallpaper';
	import { artPalette, type ArtPalette } from '$lib/stores/artPalette';
	import { audioSpectrum, NUM_BANDS } from '$lib/stores/audioSpectrum';

	type Props = {
		shader: string;
		/** Cap device pixel ratio. Lower = cheaper. */
		maxDpr?: number;
		/** Cap draw rate. Lower = cheaper for always-on background wallpapers. */
		targetFps?: number;
		/** When true, the canvas receives its own pointer events. False: mouse is inferred from window. */
		interactive?: boolean;
		/** Per-shader beat-gain so 100% reactivity reads consistently across shaders. */
		reactGain?: number;
	};

	let { shader, maxDpr = 2, targetFps = 45, interactive = true, reactGain = 1 }: Props = $props();

	let host: HTMLDivElement;
	let canvas: HTMLCanvasElement;
	let frameIntervalMs = 1000 / 45;

	$effect(() => {
		frameIntervalMs = Math.max(1000 / targetFps, 1000 / 60);
	});

	const VERT = `attribute vec2 a_pos; void main(){ gl_Position = vec4(a_pos, 0.0, 1.0); }`;
	const FRAG_PREAMBLE = `#extension GL_OES_standard_derivatives : enable
precision highp float;
uniform vec2 u_resolution;
uniform float u_time;
uniform vec2 u_mouse;
uniform float u_mouseDown;
uniform float u_clickTime;
uniform vec2 u_clickPos;
uniform vec3 u_clicks[8];
uniform int u_clickCount;
uniform vec3 u_color1;
uniform vec3 u_color2;
uniform vec3 u_color3;
uniform vec3 u_color4;
uniform float u_beat;
uniform float u_energy;
uniform float u_playing;
uniform float u_reactivity;
uniform float u_pulse;
uniform vec3 u_bands;
uniform float u_level;
uniform float u_spectrum[24];
uniform float u_audio;
uniform float u_peaks[24];
uniform float u_flux;
// Sample the spectrum (or its falling peak caps) at a 0..1 frequency position
// with linear interpolation. WebGL1 forbids dynamic array indexing, so both
// walk the bands with constant indices and tent weights.
float bandAt(float t){
  float x = clamp(t, 0.0, 1.0) * 23.0;
  float v = 0.0;
  for (int i = 0; i < 24; i++) { v += u_spectrum[i] * max(0.0, 1.0 - abs(x - float(i))); }
  return v;
}
float peakAt(float t){
  float x = clamp(t, 0.0, 1.0) * 23.0;
  float v = 0.0;
  for (int i = 0; i < 24; i++) { v += u_peaks[i] * max(0.0, 1.0 - abs(x - float(i))); }
  return v;
}
`;

	function compile(gl: WebGLRenderingContext, type: number, src: string) {
		const s = gl.createShader(type)!;
		gl.shaderSource(s, src);
		gl.compileShader(s);
		if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) {
			const err = gl.getShaderInfoLog(s);
			console.error('shader compile error:', err);
			throw new Error(err ?? 'compile failed');
		}
		return s;
	}

	onMount(() => {
		const gl = canvas.getContext('webgl', {
			alpha: false,
			antialias: false,
			depth: false,
			stencil: false,
			premultipliedAlpha: false,
			preserveDrawingBuffer: false,
			powerPreference: 'low-power'
		});
		if (!gl) return;
		gl.getExtension('OES_standard_derivatives');

		const onContextLost = (e: Event) => {
			e.preventDefault();
			console.warn('[ShaderWallpaper] WebGL context LOST');
		};
		const onContextRestored = () => {
			gl!.getExtension('OES_standard_derivatives');
			prog = null;
			buf = null;
			setupProgram(shader);
		};
		canvas.addEventListener('webglcontextlost', onContextLost);
		canvas.addEventListener('webglcontextrestored', onContextRestored);

		// Set by anything that changes what the frame would look like. While the
		// wallpaper is resting (see the loop) we skip the raster until this flips.
		let needsPaint = true;

		let currentPalette: PaletteId = DEFAULT_PALETTE;
		const unsubPalette = palette.subscribe((v) => {
			currentPalette = v;
			needsPaint = true;
		});

		// Beat-reactive uniforms. The playing track's tempo/energy drive the shaders;
		// position is re-anchored on each store emission (it ticks every ~250ms) and
		// interpolated against the wall clock per-frame so the beat phase stays smooth.
		let trackBpm = 0;
		let trackEnergy = 0;
		let trackBeatStrength = 0;
		let playing = false;
		let posBaseMs = 0;
		let posBaseAt = performance.now();
		// User-facing reactivity controls (Settings > Appearance). `reactive` is the
		// master on/off; `reactivity` is a 0..1 strength (percentage / 100).
		let reactive = true;
		let reactivity = 1;
		let smoothing = 0.4; // 0 = snappy, 1 = floaty (u_pulse shape)
		let reduceMotion = false;
		let colorSource: 'palette' | 'art' = 'palette';
		let idle: 'drift' | 'frozen' | 'demo' = 'drift';
		let artColors: ArtPalette | null = null;
		let liveSpectrum: number[] | null = null;
		const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
		const unsubFeatures = currentTrackFeatures.subscribe((f) => {
			trackBpm = f?.bpm ?? 0;
			trackEnergy = clamp01(f?.energy ?? 0);
			trackBeatStrength = clamp01(f?.beat_strength ?? 0);
		});
		const unsubPlaying = isPlaying.subscribe((v) => {
			playing = v;
			needsPaint = true;
		});
		const unsubPosition = position.subscribe((v) => {
			posBaseMs = v;
			posBaseAt = performance.now();
		});
		const unsubReactive = wallpaperReactive.subscribe((v) => {
			reactive = v;
			needsPaint = true;
		});
		const unsubReactivity = wallpaperReactivity.subscribe((v) => {
			reactivity = Math.max(0, v) / 100;
			needsPaint = true;
		});
		const unsubSmoothing = wallpaperBeatSmoothing.subscribe((v) => {
			smoothing = clamp01(v / 100);
			needsPaint = true;
		});
		const unsubReduceMotion = wallpaperReduceMotionActive.subscribe((v) => {
			reduceMotion = v;
			needsPaint = true;
		});
		const unsubColorSource = wallpaperColorSource.subscribe((v) => {
			colorSource = v;
			needsPaint = true;
		});
		const unsubIdle = wallpaperIdle.subscribe((v) => {
			idle = v;
			needsPaint = true;
		});
		const unsubArt = artPalette.subscribe((v) => {
			artColors = v;
			needsPaint = true;
		});
		const unsubSpectrum = audioSpectrum.subscribe((v) => {
			liveSpectrum = v;
		});

		let prog: WebGLProgram | null = null;
		let buf: WebGLBuffer | null = null;
		let uRes: WebGLUniformLocation | null = null;
		let uTime: WebGLUniformLocation | null = null;
		let uMouse: WebGLUniformLocation | null = null;
		let uMouseDown: WebGLUniformLocation | null = null;
		let uClickTime: WebGLUniformLocation | null = null;
		let uClickPos: WebGLUniformLocation | null = null;
		let uClicks: WebGLUniformLocation | null = null;
		let uClickCount: WebGLUniformLocation | null = null;
		let uColor1: WebGLUniformLocation | null = null;
		let uColor2: WebGLUniformLocation | null = null;
		let uColor3: WebGLUniformLocation | null = null;
		let uColor4: WebGLUniformLocation | null = null;
		let uBeat: WebGLUniformLocation | null = null;
		let uEnergy: WebGLUniformLocation | null = null;
		let uPlaying: WebGLUniformLocation | null = null;
		let uReactivity: WebGLUniformLocation | null = null;
		let uPulse: WebGLUniformLocation | null = null;
		let uBands: WebGLUniformLocation | null = null;
		let uLevel: WebGLUniformLocation | null = null;
		let uSpectrum: WebGLUniformLocation | null = null;
		let uAudio: WebGLUniformLocation | null = null;
		let uPeaks: WebGLUniformLocation | null = null;
		let uFlux: WebGLUniformLocation | null = null;

		function setupProgram(fragSrc: string) {
			if (prog) gl!.deleteProgram(prog);
			const vs = compile(gl!, gl!.VERTEX_SHADER, VERT);
			const fs = compile(gl!, gl!.FRAGMENT_SHADER, FRAG_PREAMBLE + '\n' + fragSrc);
			prog = gl!.createProgram()!;
			gl!.attachShader(prog, vs);
			gl!.attachShader(prog, fs);
			gl!.linkProgram(prog);
			if (!gl!.getProgramParameter(prog, gl!.LINK_STATUS)) {
				console.error('link error:', gl!.getProgramInfoLog(prog));
				return;
			}
			gl!.useProgram(prog);

			if (!buf) {
				buf = gl!.createBuffer();
				gl!.bindBuffer(gl!.ARRAY_BUFFER, buf);
				gl!.bufferData(gl!.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl!.STATIC_DRAW);
			} else {
				gl!.bindBuffer(gl!.ARRAY_BUFFER, buf);
			}
			const aPos = gl!.getAttribLocation(prog, 'a_pos');
			gl!.enableVertexAttribArray(aPos);
			gl!.vertexAttribPointer(aPos, 2, gl!.FLOAT, false, 0, 0);

			uRes = gl!.getUniformLocation(prog, 'u_resolution');
			uTime = gl!.getUniformLocation(prog, 'u_time');
			uMouse = gl!.getUniformLocation(prog, 'u_mouse');
			uMouseDown = gl!.getUniformLocation(prog, 'u_mouseDown');
			uClickTime = gl!.getUniformLocation(prog, 'u_clickTime');
			uClickPos = gl!.getUniformLocation(prog, 'u_clickPos');
			uClicks = gl!.getUniformLocation(prog, 'u_clicks');
			uClickCount = gl!.getUniformLocation(prog, 'u_clickCount');
			uColor1 = gl!.getUniformLocation(prog, 'u_color1');
			uColor2 = gl!.getUniformLocation(prog, 'u_color2');
			uColor3 = gl!.getUniformLocation(prog, 'u_color3');
			uColor4 = gl!.getUniformLocation(prog, 'u_color4');
			uBeat = gl!.getUniformLocation(prog, 'u_beat');
			uEnergy = gl!.getUniformLocation(prog, 'u_energy');
			uPlaying = gl!.getUniformLocation(prog, 'u_playing');
			uReactivity = gl!.getUniformLocation(prog, 'u_reactivity');
			uPulse = gl!.getUniformLocation(prog, 'u_pulse');
			uBands = gl!.getUniformLocation(prog, 'u_bands');
			uLevel = gl!.getUniformLocation(prog, 'u_level');
			uSpectrum = gl!.getUniformLocation(prog, 'u_spectrum');
			uAudio = gl!.getUniformLocation(prog, 'u_audio');
			uPeaks = gl!.getUniformLocation(prog, 'u_peaks');
			uFlux = gl!.getUniformLocation(prog, 'u_flux');
			needsPaint = true;
		}

		setupProgram(shader);

		const state = {
			mouse: [0.5, 0.5] as [number, number],
			targetMouse: [0.5, 0.5] as [number, number],
			mouseDown: 0,
			clickTime: 999,
			clickPos: [0.5, 0.5] as [number, number],
			clicks: [] as { x: number; y: number; t0: number }[],
			start: performance.now()
		};

		const resize = () => {
			const rect = host.getBoundingClientRect();
			const dpr = Math.min(window.devicePixelRatio || 1, maxDpr);
			const w = Math.max(1, Math.floor(rect.width * dpr));
			const h = Math.max(1, Math.floor(rect.height * dpr));
			if (canvas.width !== w || canvas.height !== h) {
				canvas.width = w;
				canvas.height = h;
				needsPaint = true;
			}
		};
		resize();
		const ro = new ResizeObserver(resize);
		ro.observe(host);

		function pointerToUv(clientX: number, clientY: number) {
			const r = host.getBoundingClientRect();
			const x = (clientX - r.left) / r.width;
			const y = 1 - (clientY - r.top) / r.height;
			return [x, y] as [number, number];
		}

		const onMove = (e: PointerEvent | MouseEvent) => {
			state.targetMouse = pointerToUv(e.clientX, e.clientY);
		};
		const onDown = (e: PointerEvent | MouseEvent) => {
			const [x, y] = pointerToUv(e.clientX, e.clientY);
			state.mouseDown = 1;
			state.clickTime = 0;
			state.clickPos = [x, y];
			state.clicks.push({ x, y, t0: (performance.now() - state.start) / 1000 });
			if (state.clicks.length > 8) state.clicks.shift();
		};
		const onUp = () => {
			state.mouseDown = 0;
		};

		if (interactive) {
			host.addEventListener('pointermove', onMove);
			host.addEventListener('pointerdown', onDown);
			host.addEventListener('pointerup', onUp);
			host.addEventListener('pointerleave', onUp);
		} else {
			window.addEventListener('pointermove', onMove);
		}

		let raf = 0;
		let running = true;
		let lastFrame = performance.now();
		// Resting: while music (or demo) drives the shaders, render at the user's
		// fps. Once nothing has driven them for IDLE_GRACE_MS (long enough for the
		// VU envelopes to visibly decay to silence), Drift halves that fps (still
		// cheaper than active, but the fps slider now visibly changes idle motion
		// instead of pinning to a flat rate) and Frozen stops painting entirely
		// until an input that changes the picture (pointer, click ripple, palette,
		// shader, resize) invalidates the frame. A full-window raster at 60fps
		// while paused was the app's single biggest idle GPU cost; this is the
		// lever that removes it.
		const IDLE_FPS_DIVISOR = 2;
		const IDLE_GRACE_MS = 5000;
		const MOUSE_SETTLE_EPS = 0.0002;
		const CLICK_LIVE_S = 5;
		let lastDrivingAt = performance.now();
		// Holds u_time still while Idle = Frozen and nothing is driving the shaders.
		let frozenT: number | null = null;
		// Attack/decay-smoothed band levels (bass, mid, treble, overall). Fast rise,
		// slow fall, like a VU meter, so the visuals swell and settle instead of
		// flashing on every beat. Persist across frames.
		let sBass = 0;
		let sMid = 0;
		let sTreble = 0;
		let sLevel = 0;
		let lastBandNow = performance.now();
		// Per-frame-smoothed copy of the real FFT spectrum (or a synthesized one
		// in demo/idle), uploaded to u_spectrum. Length must match NUM_BANDS.
		const specSmooth = new Float32Array(NUM_BANDS);
		// Falling peak caps (u_peaks): each cap sits on its bar, holds a beat,
		// then drops - the classic analyzer read of "how loud was that just now".
		const specPeaks = new Float32Array(NUM_BANDS);
		const peakHold = new Float64Array(NUM_BANDS);
		// Spectral flux (u_flux): how much the spectrum ROSE this frame, i.e.
		// onsets. Shaders use it for motion kicks, never brightness.
		const prevTarget = new Float32Array(NUM_BANDS);
		let fluxEnv = 0;
		// Slow auto-gain: running max of the raw bands, so quietly-mastered
		// tracks still fill the analyzer instead of hugging the floor.
		let agcEnv = 0.7;
		// Move `cur` toward `tgt` with a time-constant that differs on the way up
		// (attack) vs down (decay), framerate-independent via the exp of dt.
		const smoothBand = (cur: number, tgt: number, atk: number, dec: number, dt: number) => {
			const rate = tgt > cur ? atk : dec;
			return cur + (tgt - cur) * (1 - Math.exp(-rate * dt));
		};

		const loop = () => {
			if (!running) return;
			raf = requestAnimationFrame(loop);

			const now = performance.now();
			if (document.hidden) {
				lastFrame = now;
				return;
			}

			// ── music / reactivity state ────────────────────────────────────────
			// musicOn: a real track is driving. demoOn: synthesize a beat so the
			// reactive shaders show life while nothing plays (Idle = Demo).
			// Computed before the frame gates because the resting state decides
			// how often (and whether) this frame gets painted at all.
			const musicOn = playing && reactive && reactivity > 0;
			const demoOn = reactive && !musicOn && idle === 'demo';
			const driving = musicOn || demoOn;
			if (driving) lastDrivingAt = now;
			const resting = !driving && now - lastDrivingAt >= IDLE_GRACE_MS;

			const interval =
				resting && idle === 'drift' ? frameIntervalMs * IDLE_FPS_DIVISOR : frameIntervalMs;
			if (now - lastFrame < interval) return;
			if (resting && idle === 'frozen') {
				// Frozen and settled: the frame cannot change, so skip the raster.
				// lastFrame is deliberately not stamped, so the next invalidation
				// paints on the very next tick instead of waiting out the interval.
				const mouseSettled =
					Math.abs(state.targetMouse[0] - state.mouse[0]) < MOUSE_SETTLE_EPS &&
					Math.abs(state.targetMouse[1] - state.mouse[1]) < MOUSE_SETTLE_EPS;
				const lastClick = state.clicks.length ? state.clicks[state.clicks.length - 1] : null;
				const clicksLive =
					lastClick !== null && (now - state.start) / 1000 - lastClick.t0 < CLICK_LIVE_S;
				if (!needsPaint && mouseSettled && !clicksLive) return;
			}
			// Phase-preserving stamp: `lastFrame = now` quantizes against the 60Hz
			// rAF grid and bleeds a 30fps target down to ~20fps. Advance by the
			// interval instead, snapping to now if we fell more than a frame behind.
			lastFrame += interval;
			if (now - lastFrame > interval) lastFrame = now;
			needsPaint = false;

			state.mouse[0] += (state.targetMouse[0] - state.mouse[0]) * 0.12;
			state.mouse[1] += (state.targetMouse[1] - state.mouse[1]) * 0.12;

			const rawT = (now - state.start) / 1000;
			let beatPhase = 0;
			let energy = 0;
			if (musicOn) {
				// Interpolate position off the last store emission (it ticks ~4 Hz).
				const estPosMs = posBaseMs + (now - posBaseAt);
				const tempo = trackBpm > 30 && trackBpm < 300 ? trackBpm : 100;
				beatPhase = ((estPosMs / 1000) * (tempo / 60)) % 1;
				energy = trackEnergy;
			} else if (demoOn) {
				beatPhase = (rawT * (100 / 60)) % 1; // gentle synthetic 100 BPM
				energy = 0.5;
			}

			// Reactivity amount: user strength × per-shader gain, capped when
			// reduce-motion is active. Demo keeps a floor so it always animates.
			let react = 0;
			if (musicOn) react = reactivity * reactGain;
			else if (demoOn) react = Math.max(reactivity, 0.5) * reactGain;
			if (reduceMotion) react = Math.min(react, 0.25);

			// ── Smoothed band model (bass / mid / treble) ───────────────────────
			// Synthesize per-band excitation from the beat clock + energy, then
			// attack/decay smooth it: fast rise, slow fall, like a VU meter. The beat
			// drives a bass SWELL, not a flash; mid rides energy + a slow LFO; treble
			// rides an offbeat (8th-note) LFO. Beat smoothing maps to how fast the
			// bass falls (snappy = quick, floaty = long tail). Runs every frame so it
			// decays smoothly to zero when the music stops.
			const bandDt = Math.min(0.05, Math.max(0, (now - lastBandNow) / 1000));
			lastBandNow = now;
			const strength = 0.45 + trackBeatStrength * 0.55;
			const beatBump = Math.pow(1 - beatPhase, 1.6);
			const eighth = (beatPhase * 2) % 1;
			let bassT = 0;
			let midT = 0;
			let trebT = 0;
			if (driving) {
				bassT = (0.35 + energy * 0.65) * strength * beatBump;
				midT = energy * (0.35 + 0.65 * (0.5 + 0.5 * Math.sin(now * 0.0028)));
				trebT = energy * (0.25 + 0.75 * Math.pow(1 - eighth, 3.0)) * 0.7;
			}
			const bassDecay = 5.5 - smoothing * 3.0; // snappy ~5.5/s, floaty ~2.5/s
			// Attack is deliberately unhurried (a swell, not a snap) so beats never
			// read as a flash; decay is slower still for a musical tail.
			sBass = smoothBand(sBass, bassT, 10, bassDecay, bandDt);
			sMid = smoothBand(sMid, midT, 9, 3.5, bandDt);
			sTreble = smoothBand(sTreble, trebT, 22, 9.0, bandDt);
			sLevel = smoothBand(sLevel, sBass * 0.55 + sMid * 0.3 + sTreble * 0.25, 16, 3.0, bandDt);

			// ── Real FFT spectrum ───────────────────────────────────────────────
			// When the backend is streaming live bands (audio actually playing) use
			// them; in demo, synthesize a spectrum from the coarse bands so the DJ
			// shader still previews; idle decays flat. Values are the pure signal
			// (shaders apply u_reactivity themselves).
			const liveAudio = !!(liveSpectrum && liveSpectrum.length >= NUM_BANDS && driving);
			// Auto-gain: track a slow running max of the raw bands and lift quiet
			// masters toward full range. Boost-only (max 2x), slow attack so a
			// chorus still pegs the bars for a moment before the gain settles.
			if (liveAudio) {
				let frameMax = 0;
				for (let k = 0; k < NUM_BANDS; k++) frameMax = Math.max(frameMax, liveSpectrum![k] || 0);
				if (frameMax > 0.02) agcEnv = smoothBand(agcEnv, frameMax, 1.2, 0.12, bandDt);
			}
			const agcGain = liveAudio ? Math.min(2, Math.max(1, 0.92 / Math.max(agcEnv, 0.3))) : 1;
			let fluxSum = 0;
			for (let k = 0; k < NUM_BANDS; k++) {
				let target = 0;
				if (liveAudio) {
					target = Math.min(1, (liveSpectrum![k] || 0) * agcGain);
				} else if (demoOn) {
					const f = k / (NUM_BANDS - 1);
					const bassW = Math.max(0, 1 - f * 2.2);
					const midW = Math.exp(-Math.pow((f - 0.45) * 3.2, 2));
					const trebW = Math.max(0, (f - 0.55) / 0.45);
					target = sBass * bassW + sMid * midW + sTreble * trebW;
				}
				fluxSum += Math.max(0, target - prevTarget[k]);
				prevTarget[k] = target;
				const rising = target > specSmooth[k];
				specSmooth[k] += (target - specSmooth[k]) * (1 - Math.exp(-(rising ? 22 : 7) * bandDt));
				if (specSmooth[k] >= specPeaks[k]) {
					specPeaks[k] = specSmooth[k];
					peakHold[k] = now + 420;
				} else if (now > peakHold[k]) {
					specPeaks[k] = Math.max(specSmooth[k], specPeaks[k] - 0.35 * bandDt);
				}
			}
			// Onset envelope: fast attack, musical decay. Normalized by dt so the
			// value doesn't depend on the render frame rate.
			const fluxRate = fluxSum / NUM_BANDS / Math.max(bandDt, 0.008);
			fluxEnv = smoothBand(fluxEnv, Math.min(1, fluxRate * 0.55), 35, 5.5, bandDt);
			if (liveAudio) {
				// Derive the 3 coarse bands the other reactive shaders read from the
				// real spectrum, so every reactive wallpaper follows the audio.
				let lo = 0;
				let md = 0;
				let hi = 0;
				for (let k = 0; k < 6; k++) lo += specSmooth[k];
				for (let k = 6; k < 15; k++) md += specSmooth[k];
				for (let k = 15; k < NUM_BANDS; k++) hi += specSmooth[k];
				sBass = lo / 6;
				sMid = md / 9;
				sTreble = hi / (NUM_BANDS - 15);
				sLevel = sBass * 0.6 + sMid * 0.3 + sTreble * 0.25;
			}

			// Freeze base motion when Idle = Frozen and nothing is driving. Clicks and
			// mouse keep real time so interaction still works in the settings preview.
			if (!driving && idle === 'frozen') {
				if (frozenT === null) frozenT = rawT;
			} else {
				frozenT = null;
			}
			const t = frozenT !== null ? frozenT : rawT;
			state.clickTime = state.clicks.length ? rawT - state.clicks[state.clicks.length - 1].t0 : 999;

			gl!.viewport(0, 0, canvas.width, canvas.height);
			gl!.uniform2f(uRes!, canvas.width, canvas.height);
			gl!.uniform1f(uTime!, t);
			gl!.uniform2f(uMouse!, state.mouse[0], state.mouse[1]);
			gl!.uniform1f(uMouseDown!, state.mouseDown);
			gl!.uniform1f(uClickTime!, state.clickTime);
			gl!.uniform2f(uClickPos!, state.clickPos[0], state.clickPos[1]);

			const flat = new Float32Array(8 * 3);
			const ccount = Math.min(state.clicks.length, 8);
			for (let i = 0; i < ccount; i++) {
				const c = state.clicks[state.clicks.length - ccount + i];
				flat[i * 3] = c.x;
				flat[i * 3 + 1] = c.y;
				flat[i * 3 + 2] = rawT - c.t0;
			}
			gl!.uniform3fv(uClicks!, flat);
			gl!.uniform1i(uClickCount!, ccount);

			// Colours: fixed palette, or extracted from cover art when the user picked
			// "Album art" and extraction succeeded (else it falls back to the palette).
			if (colorSource === 'art' && artColors) {
				const a = artColors;
				gl!.uniform3f(uColor1!, a[0][0], a[0][1], a[0][2]);
				gl!.uniform3f(uColor2!, a[1][0], a[1][1], a[1][2]);
				gl!.uniform3f(uColor3!, a[2][0], a[2][1], a[2][2]);
				gl!.uniform3f(uColor4!, a[3][0], a[3][1], a[3][2]);
			} else {
				const pal = paletteById(currentPalette).shader;
				gl!.uniform3f(uColor1!, pal.c1[0], pal.c1[1], pal.c1[2]);
				gl!.uniform3f(uColor2!, pal.c2[0], pal.c2[1], pal.c2[2]);
				gl!.uniform3f(uColor3!, pal.c3[0], pal.c3[1], pal.c3[2]);
				gl!.uniform3f(uColor4!, pal.c4[0], pal.c4[1], pal.c4[2]);
			}

			gl!.uniform1f(uBeat!, beatPhase);
			gl!.uniform1f(uEnergy!, energy);
			gl!.uniform1f(uPlaying!, driving ? 1 : 0);
			gl!.uniform1f(uReactivity!, react);
			gl!.uniform1f(uPulse!, sBass); // smoothed bass envelope: no strobe
			gl!.uniform3f(uBands!, sBass, sMid, sTreble);
			gl!.uniform1f(uLevel!, sLevel);
			gl!.uniform1fv(uSpectrum!, specSmooth);
			gl!.uniform1f(uAudio!, liveAudio ? 1 : 0);
			gl!.uniform1fv(uPeaks!, specPeaks);
			gl!.uniform1f(uFlux!, fluxEnv);

			gl!.drawArrays(gl!.TRIANGLES, 0, 3);
		};
		loop();

		const onVisibility = () => {
			// Reset timing so a tab that was hidden doesn't resume with a huge delta,
			// and repaint once in case the frame went stale while hidden.
			lastFrame = performance.now();
			needsPaint = true;
		};
		document.addEventListener('visibilitychange', onVisibility);

		// Recompile when the shader prop changes.
		$effect.root(() => {
			$effect(() => {
				if (prog && shader) {
					try {
						setupProgram(shader);
					} catch {
						/* compile errors already logged */
					}
				}
			});
		});

		return () => {
			running = false;
			cancelAnimationFrame(raf);
			ro.disconnect();
			unsubPalette();
			unsubFeatures();
			unsubPlaying();
			unsubPosition();
			unsubReactive();
			unsubReactivity();
			unsubSmoothing();
			unsubReduceMotion();
			unsubColorSource();
			unsubIdle();
			unsubArt();
			unsubSpectrum();
			canvas.removeEventListener('webglcontextlost', onContextLost);
			canvas.removeEventListener('webglcontextrestored', onContextRestored);
			document.removeEventListener('visibilitychange', onVisibility);
			if (interactive) {
				host.removeEventListener('pointermove', onMove);
				host.removeEventListener('pointerdown', onDown);
				host.removeEventListener('pointerup', onUp);
				host.removeEventListener('pointerleave', onUp);
			} else {
				window.removeEventListener('pointermove', onMove);
			}
			if (prog) gl!.deleteProgram(prog);
			if (buf) gl!.deleteBuffer(buf);
		};
	});
</script>

<div bind:this={host} class="shader-host" class:interactive>
	<canvas bind:this={canvas} class="shader-canvas"></canvas>
</div>

<style>
	.shader-host {
		position: absolute;
		inset: 0;
		overflow: hidden;
		background: #000;
	}
	.shader-host:not(.interactive) {
		pointer-events: none;
	}
	.shader-canvas {
		position: absolute;
		inset: 0;
		width: 100%;
		height: 100%;
		display: block;
	}
</style>
