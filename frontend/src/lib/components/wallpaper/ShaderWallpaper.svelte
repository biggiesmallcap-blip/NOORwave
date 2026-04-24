<script lang="ts">
	import { onMount } from 'svelte';

	type Props = {
		shader: string;
		/** Cap device pixel ratio. Lower = cheaper. */
		maxDpr?: number;
		/** When true, the canvas receives its own pointer events. False: mouse is inferred from window. */
		interactive?: boolean;
	};

	let { shader, maxDpr = 2, interactive = true }: Props = $props();

	let host: HTMLDivElement;
	let canvas: HTMLCanvasElement;

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
		const gl = canvas.getContext('webgl', { premultipliedAlpha: false, antialias: true });
		if (!gl) return;
		gl.getExtension('OES_standard_derivatives');

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
				gl!.bufferData(
					gl!.ARRAY_BUFFER,
					new Float32Array([-1, -1, 1, -1, -1, 1, -1, 1, 1, -1, 1, 1]),
					gl!.STATIC_DRAW
				);
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

		const loop = () => {
			if (!running) return;
			raf = requestAnimationFrame(loop);

			const now = performance.now();
			if (document.hidden) {
				lastFrame = now;
				return;
			}
			// Skip frames if tab is throttled
			if (now - lastFrame < 12) return;
			lastFrame = now;

			state.mouse[0] += (state.targetMouse[0] - state.mouse[0]) * 0.12;
			state.mouse[1] += (state.targetMouse[1] - state.mouse[1]) * 0.12;
			const t = (now - state.start) / 1000;
			state.clickTime = state.clicks.length ? t - state.clicks[state.clicks.length - 1].t0 : 999;

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
				flat[i * 3 + 2] = t - c.t0;
			}
			gl!.uniform3fv(uClicks!, flat);
			gl!.uniform1i(uClickCount!, ccount);

			gl!.drawArrays(gl!.TRIANGLES, 0, 6);
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
