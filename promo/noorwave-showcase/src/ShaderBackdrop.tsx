import {useLayoutEffect, useRef} from 'react';
import {useCurrentFrame, useVideoConfig} from 'remotion';

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
`;

const SHADER_STANDING_WAVE = `void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  vec2 s1=vec2(-0.22,0.0)+0.04*vec2(sin(u_time*0.13),cos(u_time*0.11));
  vec2 s2=vec2(0.22,0.0)+0.04*vec2(cos(u_time*0.09),sin(u_time*0.15));
  float r1=length(p-s1);
  float r2=length(p-s2);
  float fa=sin(r1*60.0-u_time*2.4);
  float fb=sin(r2*60.0-u_time*2.4);
  float ridge=pow(max(0.0,abs((fa+fb)*0.5)-0.45),1.5)*2.5;
  float v=ridge*exp(-(r1+r2)*0.55)*1.1;
  v+=exp(-r1*r1/0.001)*0.4+exp(-r2*r2/0.001)*0.4;
  vec3 col=vec3(v);
  col+=vec3(0.018*(0.5+0.5*sin(p.x+u_time*0.1)));
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}`;

const IRIS_SHADER_COLORS = {
	c1: [0.08, 0.42, 0.78] as const,
	c2: [0.76, 0.22, 0.95] as const,
	c3: [1.0, 0.62, 0.32] as const,
	c4: [0.1, 0.95, 0.78] as const,
};

type ShaderState = {
	gl: WebGLRenderingContext;
	program: WebGLProgram;
	buffer: WebGLBuffer;
	uRes: WebGLUniformLocation | null;
	uTime: WebGLUniformLocation | null;
	uMouse: WebGLUniformLocation | null;
	uMouseDown: WebGLUniformLocation | null;
	uClickTime: WebGLUniformLocation | null;
	uClickPos: WebGLUniformLocation | null;
	uClicks: WebGLUniformLocation | null;
	uClickCount: WebGLUniformLocation | null;
	uColor1: WebGLUniformLocation | null;
	uColor2: WebGLUniformLocation | null;
	uColor3: WebGLUniformLocation | null;
	uColor4: WebGLUniformLocation | null;
};

const compile = (gl: WebGLRenderingContext, type: number, src: string) => {
	const shader = gl.createShader(type);
	if (!shader) throw new Error('Failed to create shader');
	gl.shaderSource(shader, src);
	gl.compileShader(shader);
	if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
		throw new Error(gl.getShaderInfoLog(shader) ?? 'Shader compile failed');
	}
	return shader;
};

const initShader = (canvas: HTMLCanvasElement): ShaderState | null => {
	const gl = canvas.getContext('webgl', {
		alpha: false,
		antialias: false,
		depth: false,
		stencil: false,
		premultipliedAlpha: false,
		preserveDrawingBuffer: true,
		powerPreference: 'high-performance',
	});
	if (!gl) return null;
	gl.getExtension('OES_standard_derivatives');

	const vs = compile(gl, gl.VERTEX_SHADER, VERT);
	const fs = compile(gl, gl.FRAGMENT_SHADER, `${FRAG_PREAMBLE}\n${SHADER_STANDING_WAVE}`);
	const program = gl.createProgram();
	if (!program) throw new Error('Failed to create shader program');
	gl.attachShader(program, vs);
	gl.attachShader(program, fs);
	gl.linkProgram(program);
	if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
		throw new Error(gl.getProgramInfoLog(program) ?? 'Shader link failed');
	}
	gl.useProgram(program);

	const buffer = gl.createBuffer();
	if (!buffer) throw new Error('Failed to create shader buffer');
	gl.bindBuffer(gl.ARRAY_BUFFER, buffer);
	gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
	const aPos = gl.getAttribLocation(program, 'a_pos');
	gl.enableVertexAttribArray(aPos);
	gl.vertexAttribPointer(aPos, 2, gl.FLOAT, false, 0, 0);

	return {
		gl,
		program,
		buffer,
		uRes: gl.getUniformLocation(program, 'u_resolution'),
		uTime: gl.getUniformLocation(program, 'u_time'),
		uMouse: gl.getUniformLocation(program, 'u_mouse'),
		uMouseDown: gl.getUniformLocation(program, 'u_mouseDown'),
		uClickTime: gl.getUniformLocation(program, 'u_clickTime'),
		uClickPos: gl.getUniformLocation(program, 'u_clickPos'),
		uClicks: gl.getUniformLocation(program, 'u_clicks'),
		uClickCount: gl.getUniformLocation(program, 'u_clickCount'),
		uColor1: gl.getUniformLocation(program, 'u_color1'),
		uColor2: gl.getUniformLocation(program, 'u_color2'),
		uColor3: gl.getUniformLocation(program, 'u_color3'),
		uColor4: gl.getUniformLocation(program, 'u_color4'),
	};
};

export const ShaderBackdrop = () => {
	const canvasRef = useRef<HTMLCanvasElement | null>(null);
	const shaderRef = useRef<ShaderState | null>(null);
	const frame = useCurrentFrame();
	const {fps, width, height} = useVideoConfig();

	useLayoutEffect(() => {
		const canvas = canvasRef.current;
		if (!canvas) return;
		if (canvas.width !== width || canvas.height !== height) {
			canvas.width = width;
			canvas.height = height;
		}
		if (!shaderRef.current) {
			shaderRef.current = initShader(canvas);
		}
		const shader = shaderRef.current;
		if (!shader) return;

		const t = frame / fps;
		const {gl} = shader;
		gl.viewport(0, 0, width, height);
		gl.useProgram(shader.program);
		gl.uniform2f(shader.uRes, width, height);
		gl.uniform1f(shader.uTime, t);
		gl.uniform2f(shader.uMouse, 0.5, 0.5);
		gl.uniform1f(shader.uMouseDown, 0);
		gl.uniform1f(shader.uClickTime, 999);
		gl.uniform2f(shader.uClickPos, 0.5, 0.5);
		gl.uniform3fv(shader.uClicks, new Float32Array(8 * 3));
		gl.uniform1i(shader.uClickCount, 0);
		gl.uniform3f(shader.uColor1, ...IRIS_SHADER_COLORS.c1);
		gl.uniform3f(shader.uColor2, ...IRIS_SHADER_COLORS.c2);
		gl.uniform3f(shader.uColor3, ...IRIS_SHADER_COLORS.c3);
		gl.uniform3f(shader.uColor4, ...IRIS_SHADER_COLORS.c4);
		gl.drawArrays(gl.TRIANGLES, 0, 3);
	}, [fps, frame, height, width]);

	return <canvas className="standing-wave-canvas" ref={canvasRef} />;
};
