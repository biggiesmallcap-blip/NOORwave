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

		let currentPalette: PaletteId = DEFAULT_PALETTE;
		const unsubPalette = palette.subscribe((v) => {
			currentPalette = v;
		});

		// Beat-reactive uniforms. The playing track's tempo/energy drive the shaders;
		// position is re-anchored on each store emission (it ticks every ~250ms) and
		// interpolated against the wall clock per-frame so the beat phase stays smooth.
		let trackBpm = 0;
		let trackEnergy = 0;
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
		const clamp01 = (v: number) => Math.max(0, Math.min(1, v));
		const unsubFeatures = currentTrackFeatures.subscribe((f) => {
			trackBpm = f?.bpm ?? 0;
			trackEnergy = clamp01(f?.energy ?? 0);
		});
		const unsubPlaying = isPlaying.subscribe((v) => {
			playing = v;
		});
		const unsubPosition = position.subscribe((v) => {
			posBaseMs = v;
			posBaseAt = performance.now();
		});
		const unsubReactive = wallpaperReactive.subscribe((v) => {
			reactive = v;
		});
		const unsubReactivity = wallpaperReactivity.subscribe((v) => {
			reactivity = Math.max(0, v) / 100;
		});
		const unsubSmoothing = wallpaperBeatSmoothing.subscribe((v) => {
			smoothing = clamp01(v / 100);
		});
		const unsubReduceMotion = wallpaperReduceMotionActive.subscribe((v) => {
			reduceMotion = v;
		});
		const unsubColorSource = wallpaperColorSource.subscribe((v) => {
			colorSource = v;
		});
		const unsubIdle = wallpaperIdle.subscribe((v) => {
			idle = v;
		});
		const unsubArt = artPalette.subscribe((v) => {
			artColors = v;
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
		// Holds u_time still while Idle = Frozen and nothing is driving the shaders.
		let frozenT: number | null = null;

		const loop = () => {
			if (!running) return;
			raf = requestAnimationFrame(loop);

			const now = performance.now();
			if (document.hidden) {
				lastFrame = now;
				return;
			}
			if (now - lastFrame < frameIntervalMs) return;
			lastFrame = now;

			state.mouse[0] += (state.targetMouse[0] - state.mouse[0]) * 0.12;
			state.mouse[1] += (state.targetMouse[1] - state.mouse[1]) * 0.12;
			// ── music / reactivity state ────────────────────────────────────────
			// musicOn: a real track is driving. demoOn: synthesize a beat so the
			// reactive shaders show life while nothing plays (Idle = Demo).
			const musicOn = playing && reactive && reactivity > 0;
			const demoOn = reactive && !musicOn && idle === 'demo';
			const driving = musicOn || demoOn;

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

			// Beat envelope shape (snappy → floaty), both peaking on the beat onset.
			const snappy = Math.pow(1 - beatPhase, 3);
			const floaty = 0.5 + 0.5 * Math.cos(Math.PI * Math.min(beatPhase, 1));
			const pulse = driving ? snappy * (1 - smoothing) + floaty * smoothing : 0;

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
			gl!.uniform1f(uPulse!, pulse);

			gl!.drawArrays(gl!.TRIANGLES, 0, 3);
		};
		loop();

		const onVisibility = () => {
			// Reset timing so a tab that was hidden doesn't resume with a huge delta.
			lastFrame = performance.now();
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
