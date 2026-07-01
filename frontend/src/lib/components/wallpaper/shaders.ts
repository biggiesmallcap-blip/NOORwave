// Five interactive fragment shaders ported from the Claude Design bundle.
// Each string is injected after FRAG_PREAMBLE in ShaderWallpaper.svelte.

export const SHADER_AURORA = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<5;i++){ v += a*noise(p); p *= 2.03; a *= 0.5; }
  return v;
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = uv;
  p.x *= u_resolution.x / u_resolution.y;
  vec2 m = u_mouse;
  m.x *= u_resolution.x / u_resolution.y;

  float wave = 0.0;
  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = c.xy; cp.x *= u_resolution.x / u_resolution.y;
    float d = distance(p, cp);
    float age = c.z;
    float ring = exp(-pow((d - age*0.7)*6.0, 2.0)) * exp(-age*0.9);
    wave += ring;
  }

  vec2 toM = (m - p);
  float pull = 0.25 / (0.25 + dot(toM, toM));
  vec2 flow = vec2(
    fbm(p*1.4 + vec2(u_time*0.08, 0.0)),
    fbm(p*1.4 + vec2(0.0, -u_time*0.06))
  ) - 0.5;
  flow += toM * pull * 0.35;
  flow += wave * normalize(toM + 1e-5) * 0.4;

  vec2 q = p + flow * 0.6;
  float n = fbm(q * 2.2 + u_time * 0.12);
  float n2 = fbm(q * 4.5 - u_time * 0.18);
  float band = sin(n * 6.2831 + n2 * 3.0 + u_time * 0.4);
  band = pow(0.5 + 0.5*band, 2.2);

  float t1 = smoothstep(0.0, 0.6, n);
  vec3 col = mix(u_color1, u_color2, t1);
  col = mix(col, u_color3, smoothstep(0.55, 1.0, n*n2*1.8));
  col = mix(col, u_color4, smoothstep(0.0, 0.2, pull*1.2) * 0.5);

  col *= band * 1.3 + 0.15;
  col += wave * vec3(1.0, 0.9, 1.0) * 1.5;

  vec3 bg = vec3(0.02, 0.01, 0.05);
  col = bg + col;

  float g = exp(-distance(p, m) * 5.0);
  col += g * vec3(0.4, 0.3, 0.6) * (0.3 + u_mouseDown * 0.8);

  col += (hash(gl_FragCoord.xy + u_time) - 0.5) * 0.025;
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_CHROME = /* glsl */ `
#define PI 3.14159265

float sdCircle(vec2 p, float r){ return length(p) - r; }
float smin(float a, float b, float k){
  float h = clamp(0.5 + 0.5*(b-a)/k, 0.0, 1.0);
  return mix(b, a, h) - k*h*(1.0-h);
}

vec3 env(vec2 n){
  float y = n.y;
  vec3 sky = mix(vec3(0.78, 0.85, 1.05), vec3(0.25, 0.18, 0.38), smoothstep(-0.2, 0.9, y));
  vec3 warm = vec3(1.0, 0.55, 0.25) * smoothstep(0.1, -0.3, y);
  vec3 col = sky + warm;
  col += smoothstep(0.92, 1.0, sin(n.y*14.0 + n.x*3.0)) * 0.6;
  col += smoothstep(0.86, 1.0, sin(n.x*22.0)) * 0.3;
  return col;
}

void main(){
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = (u_mouse * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;

  float d = 1e9;

  float r0 = 0.18 + u_mouseDown * 0.05 + sin(u_time*2.0)*0.01;
  d = smin(d, sdCircle(p - m, r0), 0.12);

  for(int i=0;i<4;i++){
    float fi = float(i);
    vec2 wp = vec2(
      sin(u_time*0.3 + fi*1.7) * 0.7,
      cos(u_time*0.27 + fi*2.1) * 0.35
    );
    float rr = 0.11 + 0.04*sin(u_time + fi);
    d = smin(d, sdCircle(p - wp, rr), 0.18);
  }

  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = (c.xy * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;
    float age = c.z;
    vec2 target = mix(cp, m, clamp(age*0.15, 0.0, 0.85));
    float orbit = age*1.2 + float(i);
    target += vec2(cos(orbit), sin(orbit)) * 0.08 * exp(-age*0.15);
    float rr = 0.09 * exp(-age*0.12);
    if(rr > 0.005) d = smin(d, sdCircle(p - target, rr), 0.1);
  }

  vec3 col;
  if (d < 0.0){
    vec2 g = vec2(dFdx(d), dFdy(d));
    vec3 n = normalize(vec3(g * 40.0, 1.0));
    vec3 viewDir = vec3(0.0, 0.0, 1.0);
    vec3 r = reflect(-viewDir, n);
    col = env(r.xy);
    float fres = pow(1.0 - n.z, 3.0);
    col += fres * vec3(1.0, 0.95, 0.9) * 0.7;
    col *= mix(vec3(0.85, 0.88, 1.0), vec3(1.0), smoothstep(0.0, -0.15, d));
  } else {
    col = mix(vec3(0.07, 0.05, 0.10), vec3(0.02, 0.02, 0.04), length(p)*0.7);
    col += exp(-distance(p, m)*2.5) * vec3(0.35, 0.22, 0.5) * 0.4;
    float edge = smoothstep(0.02, 0.0, d);
    col += edge * vec3(0.9, 0.85, 1.0) * 0.5;
  }

  col += smoothstep(0.8, 1.0, sin(p.x*8.0 + u_time*0.3)) * 0.03;

  col = mix(col, col * (0.5 + u_color1), 0.10);

  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_GRID = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}

float terrain(vec2 p, float t){
  float h = 0.0;
  h += sin(p.x*1.2 + t*0.4) * 0.25;
  h += sin(p.y*1.5 - t*0.3) * 0.2;
  h += noise(p*0.8 + t*0.1) * 0.5;
  return h;
}

void main(){
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = (u_mouse * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;

  float horizon = 0.15;
  float yFromHorizon = horizon - p.y;
  float onGround = step(0.001, yFromHorizon);

  float depth = 1.0 / max(yFromHorizon, 0.001);
  vec2 w = vec2(p.x * depth, depth);
  w.y += u_time * 0.6;

  float warp = exp(-distance(p, m) * 2.5) * (0.5 + u_mouseDown*1.5);
  vec2 warpDir = normalize(m - p + 1e-5);
  w += warpDir * warp * 3.0;

  float pulse = 0.0;
  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = (c.xy * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;
    float d = distance(p, cp);
    float age = c.z;
    pulse += exp(-pow((d - age*0.5)*8.0, 2.0)) * exp(-age*0.8) * 1.5;
  }

  float h = terrain(w * 0.7, u_time) + pulse;

  vec2 gw = fract(w) - 0.5;
  float lineX = smoothstep(0.04, 0.0, abs(gw.x));
  float lineY = smoothstep(0.04, 0.0, abs(gw.y));
  float line = max(lineX, lineY);
  line *= exp(-depth * 0.04);

  float band = smoothstep(0.02, 0.0, abs(fract(h * 5.0) - 0.5));
  band *= 0.6 + 0.4*sin(u_time*0.5);

  vec3 col = mix(u_color1, u_color2, smoothstep(-0.3, 0.6, h));
  col *= (line * 1.2 + band * 0.8);

  float haze = exp(-yFromHorizon * 2.0);

  vec3 sky = vec3(0.01, 0.02, 0.05);
  sky += smoothstep(0.995, 1.0, hash(floor(p*400.0))) * 1.2;
  float hg = exp(-abs(p.y - horizon) * 20.0);
  sky += mix(u_color1, u_color2, 0.5) * hg * 0.8;

  vec3 final = mix(sky, col + haze * mix(u_color1, u_color2, 0.5)*0.2, onGround);

  final += exp(-distance(p, m) * 6.0) * vec3(1.0, 0.8, 1.0) * (0.3 + u_mouseDown*0.6);

  final *= 0.92 + 0.08 * sin(gl_FragCoord.y * 3.14159 * 0.5);

  gl_FragColor = vec4(final, 1.0);
}
`;

export const SHADER_NEBULA = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<6;i++){ v += a*noise(p); p = p*2.0 + 13.7; a *= 0.5; }
  return v;
}

vec3 stars(vec2 p, float density, float size){
  vec2 g = floor(p);
  vec2 f = fract(p);
  float h = hash(g);
  if(h < density) return vec3(0.0);
  vec2 sp = vec2(hash(g+17.3), hash(g+93.1));
  float d = distance(f, sp);
  float s = smoothstep(size, 0.0, d);
  vec3 col = mix(vec3(1.0, 0.85, 0.7), vec3(0.7, 0.85, 1.0), hash(g+0.1));
  s *= 0.7 + 0.3*sin(u_time*2.0 + h*30.0);
  return col * s;
}

void main(){
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = (u_mouse * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;

  vec2 toM = p - m;
  float r2 = dot(toM, toM);
  float lens = 0.15 / (r2 + 0.02);
  vec2 lensedP = p - normalize(toM + 1e-5) * lens * 0.05;

  float nova = 0.0;
  vec3 novaCol = vec3(0.0);
  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = (c.xy * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;
    float d = distance(p, cp);
    float age = c.z;
    float shell = exp(-pow((d - age*0.9)*5.0, 2.0)) * exp(-age*0.6);
    float core = exp(-d*8.0) * exp(-age*1.5);
    nova += shell + core*2.0;
    novaCol += (shell * vec3(1.0, 0.9, 0.7) + core * vec3(1.0, 0.7, 0.4)) * 1.5;
  }

  float n1 = fbm(lensedP * 1.5 + u_time*0.02);
  float n2 = fbm(lensedP * 3.5 - u_time*0.03 + n1);
  float cloud = pow(n2, 1.6) * 1.4;

  vec3 neb = mix(u_color1, u_color2, smoothstep(0.3, 0.8, n1));
  neb = mix(neb, u_color3, smoothstep(0.2, 0.7, n2) * 0.6);
  neb *= cloud;

  vec3 col = vec3(0.005, 0.008, 0.02) + neb * 0.7;

  vec3 st = vec3(0.0);
  st += stars(lensedP * 60.0, 0.985, 0.35);
  st += stars(lensedP * 120.0 + 7.3, 0.992, 0.3) * 0.8;
  st += stars(lensedP * 240.0 + 13.1, 0.996, 0.25) * 0.6;
  col += st;

  float ring = exp(-pow((sqrt(r2) - 0.08)*20.0, 2.0));
  col += ring * vec3(1.0, 0.85, 0.7) * 0.9;
  col *= smoothstep(0.02, 0.1, sqrt(r2));

  col += exp(-sqrt(r2)*25.0) * vec3(1.0, 0.95, 0.85) * (1.0 + u_mouseDown*2.0);

  col += novaCol * nova;

  col *= 1.0 + nova*0.3;

  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_TOPO = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<5;i++){ v += a*noise(p); p = p*2.1; a *= 0.5; }
  return v;
}

float height(vec2 p){
  float h = fbm(p*0.7 + vec2(u_time*0.05, -u_time*0.04)) * 1.2;
  h += fbm(p*2.0 - u_time*0.02) * 0.3;

  vec2 m = u_mouse;
  m.x *= u_resolution.x / u_resolution.y;
  vec2 pp = p;
  float d = distance(pp, m);
  h -= exp(-d*2.5) * (0.6 + u_mouseDown*1.8);

  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = c.xy;
    cp.x *= u_resolution.x / u_resolution.y;
    float cd = distance(pp, cp);
    float age = c.z;
    float damp = exp(-age*0.35);
    float ripple = sin(cd*14.0 - age*5.0) * exp(-pow((cd - age*0.25)*2.5, 2.0)) * damp;
    h += ripple * 0.4;
  }
  return h;
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = uv;
  p.x *= u_resolution.x / u_resolution.y;
  vec2 m = u_mouse; m.x *= u_resolution.x / u_resolution.y;

  float h = height(p);

  float dx = dFdx(h);
  float dy = dFdy(h);
  vec2 grad = vec2(dx, dy);
  float slope = length(grad) * u_resolution.y;

  float lineDensity = 14.0;
  float c = fract(h * lineDensity);
  float contour = smoothstep(0.08, 0.0, abs(c - 0.5) - 0.02);
  float major = fract(h * lineDensity / 5.0);
  float majorLine = smoothstep(0.05, 0.0, abs(major - 0.5) - 0.01) * 1.4;

  float ang = atan(grad.y, grad.x);
  float tick = sin(ang*3.0 + h*40.0 - u_time*2.0);
  float flow = smoothstep(0.92, 1.0, tick) * smoothstep(0.0, 0.15, slope);

  vec3 paper = vec3(0.97, 0.95, 0.90);
  vec3 ink = vec3(0.08, 0.12, 0.22);
  vec3 accent = vec3(0.92, 0.35, 0.20);

  float shade = smoothstep(-1.2, 1.2, h);
  vec3 base = mix(vec3(0.88, 0.90, 0.94), paper, shade);
  base = mix(base, vec3(0.72, 0.80, 0.95), smoothstep(0.4, -0.8, h)*0.6);
  base = mix(base, vec3(0.98, 0.82, 0.62), smoothstep(0.3, 1.1, h)*0.5);

  vec3 col = mix(base, ink, contour);
  col = mix(col, ink * 0.6, majorLine);

  float heat = exp(-distance(p, m)*3.0) * (0.5 + u_mouseDown);
  col = mix(col, accent, contour * heat * 0.9);
  col = mix(col, accent, flow * 0.7);

  col *= 0.97 + 0.06*hash(floor(gl_FragCoord.xy));

  vec2 vig = uv - 0.5;
  col *= 1.0 - dot(vig, vig) * 0.4;

  col = mix(col, col * (0.5 + u_color1), 0.10);

  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_ZEN = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<4;i++){ v += a*noise(p); p *= 2.0; a *= 0.5; }
  return v;
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = uv;
  p.x *= u_resolution.x / u_resolution.y;
  vec2 m = u_mouse; m.x *= u_resolution.x / u_resolution.y;

  // Gentle mouse pull — very soft distortion
  vec2 toM = m - p;
  float mDist = length(toM);
  vec2 pWarp = p + normalize(toM + 1e-5) * (0.06 / (mDist * mDist + 0.08));

  // Three slow interference sources
  vec2 s0 = vec2(0.25, 0.45); s0.x *= u_resolution.x / u_resolution.y;
  vec2 s1 = vec2(0.75, 0.55); s1.x *= u_resolution.x / u_resolution.y;
  vec2 s2 = vec2(0.50, 0.28); s2.x *= u_resolution.x / u_resolution.y;

  float wave = 0.0;
  wave += sin(distance(pWarp, s0) * 18.0 - u_time * 0.55) * 0.33;
  wave += sin(distance(pWarp, s1) * 14.0 - u_time * 0.45) * 0.34;
  wave += sin(distance(pWarp, s2) * 16.0 - u_time * 0.40) * 0.33;

  // Slow drift adds organic feel
  wave += (fbm(p * 1.1 + u_time * 0.025) - 0.5) * 0.25;
  wave += (fbm(p * 2.2 - u_time * 0.018) - 0.5) * 0.12;

  // Click ripples
  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = c.xy; cp.x *= u_resolution.x / u_resolution.y;
    float d = distance(pWarp, cp);
    float age = c.z;
    wave += sin(d*22.0 - age*2.8) * exp(-d*3.5) * exp(-age*0.45) * 2.0;
  }

  // Caustic highlights — bright where the wave gradient is steep
  float dx = dFdx(wave);
  float dy = dFdy(wave);
  float caustic = length(vec2(dx, dy)) * u_resolution.y * 0.5;
  caustic = pow(smoothstep(0.5, 3.5, caustic), 1.6);

  // Colour: deep water base tinted by palette
  float depth = 0.5 + 0.5 * sin(wave * PI);
  vec3 water = mix(u_color1 * 0.55, u_color2 * 0.75, depth);
  water += u_color3 * fbm(p * 3.5 + u_time * 0.008) * 0.07;

  vec3 col = water;
  // Caustic sparkle — white highlight with palette accent
  col += caustic * mix(u_color4 * 1.3, vec3(1.0), 0.45) * 0.75;

  // Very soft mouse glow
  col += exp(-mDist * 5.0) * u_color3 * 0.12 * (0.5 + u_mouseDown * 0.9);

  col += (hash(gl_FragCoord.xy + u_time * 0.07) - 0.5) * 0.010;

  vec2 vig = uv - 0.5;
  col *= 1.0 - dot(vig, vig) * 0.32;

  col = clamp(col, 0.0, 1.0);
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_GALAXY = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<5;i++){ v += a*noise(p); p *= 2.1; a *= 0.5; }
  return v;
}

float starField(vec2 p, float density, float size){
  vec2 g = floor(p);
  vec2 f = fract(p);
  float h = hash(g);
  if(h < density) return 0.0;
  vec2 sp = vec2(hash(g + 17.3), hash(g + 93.1));
  float d = distance(f, sp);
  float s = smoothstep(size, 0.0, d);
  s *= 0.55 + 0.45 * sin(u_time * 1.8 + h * 25.0);
  return s;
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = (uv - 0.5);
  p.x *= u_resolution.x / u_resolution.y;

  vec2 m = (u_mouse - 0.5);
  m.x *= u_resolution.x / u_resolution.y;

  // Gravitational lensing from cursor
  vec2 toM = p - m;
  float r2 = dot(toM, toM);
  float lens = 0.07 / (r2 + 0.012);
  vec2 lp = p - normalize(toM + 1e-5) * lens * 0.025;

  // Click shockwaves through the galaxy
  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = (c.xy - 0.5); cp.x *= u_resolution.x / u_resolution.y;
    float d = distance(lp, cp);
    float age = c.z;
    float ring = exp(-pow((d - age*0.25)*9.0, 2.0)) * exp(-age*0.55);
    lp += normalize(lp - cp + 1e-5) * ring * 0.022;
  }

  float radius = length(lp);
  float angle = atan(lp.y, lp.x);

  // Differential rotation — core spins faster than outer disk
  float rot = u_time * 0.05 / (1.0 + radius * 2.8);
  float rotAngle = angle + rot;

  // Two-arm logarithmic spiral (Milky Way-like)
  float armTightness = 3.2;
  float spiralPhase = 2.0 * (rotAngle - armTightness * log(radius + 0.06));
  float arm = 0.5 + 0.5 * cos(spiralPhase);
  arm = pow(arm, 2.8) * exp(-radius * 2.2);

  // Four-arm hint: a weaker second set of arms offset by PI/2
  float arm2Phase = 4.0 * (rotAngle - armTightness * log(radius + 0.06) + PI * 0.25);
  float arm2 = (0.5 + 0.5 * cos(arm2Phase)) * 0.25 * exp(-radius * 3.0);

  float totalArm = arm + arm2;

  // Core brightness: bulge + nucleus
  float bulge = exp(-radius * 5.5) * 1.8;
  float nucleus = exp(-radius * 22.0) * 4.5;
  float disk = exp(-radius * 1.6) * 0.4;

  float density = bulge + disk + totalArm * 1.4;

  // Warped coords for star sampling — stretched along arms
  vec2 wp = lp + normalize(lp + 1e-5) * totalArm * 0.12;

  // Star layers: dense clusters along arms, sparse background
  float st = 0.0;
  st += starField(wp * 28.0 + rot, max(0.0, 0.988 - density * 0.25), 0.40) * density;
  st += starField(wp * 60.0 + rot * 0.6 + 7.3, max(0.0, 0.993 - density * 0.15), 0.28) * density * 0.75;
  st += starField(wp * 120.0 + rot * 0.3 + 13.1, 0.9965, 0.22) * 0.45;
  st += starField(wp * 240.0 + 31.7, 0.9988, 0.16) * 0.25;

  // Gas & dust nebula along arms
  vec2 gasUv = vec2(cos(rotAngle), sin(rotAngle)) * radius;
  float gas = fbm(gasUv * 4.5 + u_time * 0.004) * totalArm;

  // Dust lanes: dark absorption between arms (offset PI/2)
  float dustPhase = 2.0 * (rotAngle - armTightness * log(radius + 0.06) + PI * 0.5);
  float dust = pow(0.5 + 0.5 * cos(dustPhase), 2.2) * exp(-radius * 2.5);

  // Colours
  vec3 col = vec3(0.004, 0.005, 0.014); // deep space

  // Galactic core — warm yellow-white bloom
  vec3 coreCol = mix(vec3(1.0, 0.92, 0.78), u_color1 * 2.2, 0.35);
  col += coreCol * (bulge + nucleus);

  // Spiral arm nebula — palette colours
  vec3 armGlow = mix(u_color2, u_color3, smoothstep(0.2, 0.8, fbm(lp * 3.5)));
  col += armGlow * totalArm * 0.55;

  // Gas cloud tint
  col += u_color1 * gas * 0.6;

  // Stars: warm near core, palette-tinted in arms
  float armFrac = totalArm / (totalArm + 0.3);
  col += st * mix(vec3(0.88, 0.84, 1.0), u_color4 * 1.6, armFrac * 0.6);

  // Dust absorption — darkens disk between arms
  col *= 1.0 - dust * 0.38;

  // Background star haze
  col += starField((uv - 0.5) * 380.0 + 71.3, 0.9993, 0.12) * 0.18;

  // Lens flare ring around cursor
  float lensRing = exp(-pow((sqrt(r2) - 0.06) * 18.0, 2.0));
  col += lensRing * u_color3 * 0.7 * (0.4 + u_mouseDown * 1.4);
  col += exp(-sqrt(r2) * 20.0) * vec3(1.0, 0.95, 0.85) * (0.5 + u_mouseDown * 1.2);

  // Vignette
  col *= 1.0 - dot(uv - 0.5, uv - 0.5) * 0.42;

  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_TOPO_NOIR = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<5;i++){ v += a*noise(p); p = p*2.1; a *= 0.5; }
  return v;
}

float height(vec2 p){
  float h = fbm(p*0.7 + vec2(u_time*0.05, -u_time*0.04)) * 1.2;
  h += fbm(p*2.0 - u_time*0.02) * 0.3;

  vec2 m = u_mouse;
  m.x *= u_resolution.x / u_resolution.y;
  vec2 pp = p;
  float d = distance(pp, m);
  h -= exp(-d*2.5) * (0.6 + u_mouseDown*1.8);

  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = c.xy;
    cp.x *= u_resolution.x / u_resolution.y;
    float cd = distance(pp, cp);
    float age = c.z;
    float damp = exp(-age*0.35);
    float ripple = sin(cd*14.0 - age*5.0) * exp(-pow((cd - age*0.25)*2.5, 2.0)) * damp;
    h += ripple * 0.4;
  }
  return h;
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = uv;
  p.x *= u_resolution.x / u_resolution.y;
  vec2 m = u_mouse; m.x *= u_resolution.x / u_resolution.y;

  float h = height(p);

  float dx = dFdx(h);
  float dy = dFdy(h);
  vec2 grad = vec2(dx, dy);
  float slope = length(grad) * u_resolution.y;

  float lineDensity = 22.0;
  float c = fract(h * lineDensity);
  float contour = smoothstep(0.03, 0.0, abs(c - 0.5) - 0.003);
  float major = fract(h * lineDensity / 5.0);
  float majorLine = smoothstep(0.04, 0.0, abs(major - 0.5) - 0.008) * 1.2;

  float ang = atan(grad.y, grad.x);
  float tick = sin(ang*3.0 + h*40.0 - u_time*2.0);
  float flow = smoothstep(0.92, 1.0, tick) * smoothstep(0.0, 0.15, slope);

  float shade = smoothstep(-1.2, 1.2, h);
  vec3 bg = mix(vec3(0.04, 0.05, 0.08), u_color1 * 0.18, shade * 0.55);

  vec3 lineCol = mix(u_color2, vec3(0.85, 0.90, 1.0), 0.28);
  vec3 majorCol = clamp(u_color3 * 1.1 + vec3(0.08), 0.0, 1.0);

  vec3 col = mix(bg, lineCol, contour * 0.90);
  col = mix(col, majorCol, majorLine * 0.80);

  float heat = exp(-distance(p, m)*3.0) * (0.5 + u_mouseDown);
  col = mix(col, u_color3, contour * heat * 1.0);
  col = mix(col, u_color3 * 1.2, flow * 0.80);

  col *= 0.97 + 0.06 * hash(floor(gl_FragCoord.xy));

  vec2 vig = uv - 0.5;
  col *= 1.0 - dot(vig, vig) * 0.50;

  col = mix(col, col * (0.5 + u_color1), 0.18);

  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_AURORA_DEEP = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i);
  float b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.));
  float d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){
  float v = 0., a = 0.5;
  for(int i=0;i<5;i++){ v += a*noise(p); p *= 2.03; a *= 0.5; }
  return v;
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = uv;
  p.x *= u_resolution.x / u_resolution.y;
  vec2 m = u_mouse;
  m.x *= u_resolution.x / u_resolution.y;

  float wave = 0.0;
  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = c.xy; cp.x *= u_resolution.x / u_resolution.y;
    float d = distance(p, cp);
    float age = c.z;
    float ring = exp(-pow((d - age*0.7)*6.0, 2.0)) * exp(-age*0.9);
    wave += ring;
  }

  vec2 toM = (m - p);
  float pull = 0.25 / (0.25 + dot(toM, toM));
  vec2 flow = vec2(
    fbm(p*1.4 + vec2(u_time*0.08, 0.0)),
    fbm(p*1.4 + vec2(0.0, -u_time*0.06))
  ) - 0.5;
  flow += toM * pull * 0.35;
  flow += wave * normalize(toM + 1e-5) * 0.4;

  vec2 q = p + flow * 0.6;
  float n = fbm(q * 2.2 + u_time * 0.12);
  float n2 = fbm(q * 4.5 - u_time * 0.18);
  float band = sin(n * 6.2831 + n2 * 3.0 + u_time * 0.4);
  band = pow(0.5 + 0.5*band, 3.5);

  float t1 = smoothstep(0.0, 0.6, n);
  vec3 col = mix(u_color1, u_color2, t1);
  col = mix(col, u_color3, smoothstep(0.55, 1.0, n*n2*1.8));
  col = mix(col, u_color4, smoothstep(0.0, 0.2, pull*1.2) * 0.5);

  col *= band * 1.6;
  col += wave * vec3(1.0, 0.9, 1.0) * 2.0;

  vec3 bg = vec3(0.005, 0.002, 0.012);
  col = bg + col;

  float g = exp(-distance(p, m) * 4.0);
  col += g * vec3(0.4, 0.3, 0.6) * (0.15 + u_mouseDown * 1.2);

  col += (hash(gl_FragCoord.xy + u_time) - 0.5) * 0.018;
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_CHROME_BRUSHED = /* glsl */ `
#define PI 3.14159265

float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float sdCircle(vec2 p, float r){ return length(p) - r; }
float smin(float a, float b, float k){
  float h = clamp(0.5 + 0.5*(b-a)/k, 0.0, 1.0);
  return mix(b, a, h) - k*h*(1.0-h);
}

vec3 env(vec2 n){
  float y = n.y;
  vec3 sky = mix(vec3(0.78, 0.85, 1.05), vec3(0.25, 0.18, 0.38), smoothstep(-0.2, 0.9, y));
  vec3 warm = mix(vec3(1.0, 0.55, 0.25), u_color1 * 1.5 + vec3(0.1), 0.28) * smoothstep(0.1, -0.3, y);
  vec3 col = sky + warm;
  col += smoothstep(0.92, 1.0, sin(n.y*14.0 + n.x*3.0)) * 0.8;
  col += smoothstep(0.86, 1.0, sin(n.x*22.0)) * 0.5;
  return col;
}

void main(){
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = (u_mouse * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;

  float d = 1e9;

  float r0 = 0.18 + u_mouseDown * 0.05 + sin(u_time*2.0)*0.01;
  d = smin(d, sdCircle(p - m, r0), 0.12);

  for(int i=0;i<4;i++){
    float fi = float(i);
    vec2 wp = vec2(
      sin(u_time*0.3 + fi*1.7) * 0.7,
      cos(u_time*0.27 + fi*2.1) * 0.35
    );
    float rr = 0.11 + 0.04*sin(u_time + fi);
    d = smin(d, sdCircle(p - wp, rr), 0.18);
  }

  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 c = u_clicks[i];
    vec2 cp = (c.xy * u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;
    float age = c.z;
    vec2 target = mix(cp, m, clamp(age*0.15, 0.0, 0.85));
    float orbit = age*1.2 + float(i);
    target += vec2(cos(orbit), sin(orbit)) * 0.08 * exp(-age*0.15);
    float rr = 0.09 * exp(-age*0.12);
    if(rr > 0.005) d = smin(d, sdCircle(p - target, rr), 0.1);
  }

  vec3 col;
  if (d < 0.0){
    vec2 g = vec2(dFdx(d), dFdy(d));

    float scratchRow = floor(gl_FragCoord.y * 0.8);
    float scratchPhase = hash(vec2(scratchRow, 0.0)) * 6.28318;
    float scratch = sin(gl_FragCoord.x * 55.0 + scratchPhase) * 0.018
                  * (0.5 + 0.5 * hash(vec2(scratchRow, 1.0)));

    vec2 gR = g + vec2(scratch * 1.3, 0.0);
    vec2 gG = g + vec2(scratch,       0.0);
    vec2 gB = g + vec2(scratch * 0.6, 0.0);
    vec3 nR = normalize(vec3(gR * 40.0, 1.0));
    vec3 nG = normalize(vec3(gG * 40.0, 1.0));
    vec3 nB = normalize(vec3(gB * 40.0, 1.0));
    vec3 viewDir = vec3(0.0, 0.0, 1.0);
    vec3 rR = reflect(-viewDir, nR);
    vec3 rG = reflect(-viewDir, nG);
    vec3 rB = reflect(-viewDir, nB);

    col = vec3(env(rR.xy).r, env(rG.xy).g, env(rB.xy).b);
    float fres = pow(1.0 - nG.z, 3.0);
    col += fres * vec3(1.0, 0.95, 0.9) * 0.7;
    col *= mix(vec3(0.85, 0.88, 1.0), vec3(1.0), smoothstep(0.0, -0.15, d));
  } else {
    col = mix(vec3(0.07, 0.05, 0.10), vec3(0.02, 0.02, 0.04), length(p)*0.7);
    col += exp(-distance(p, m)*2.5) * vec3(0.35, 0.22, 0.5) * 0.4;
    float edge = smoothstep(0.02, 0.0, d);
    col += edge * vec3(0.9, 0.85, 1.0) * 0.5;
  }

  col += smoothstep(0.8, 1.0, sin(p.x*8.0 + u_time*0.3)) * 0.03;

  col = mix(col, col * (0.5 + u_color1), 0.10);

  gl_FragColor = vec4(col, 1.0);
}
`;

// ─── Music-themed monochrome shaders ─────────────────────────────────────────
// Ported from the Claude Design bundle. All produce grayscale output tinted
// by u_color1 so they respond to the active palette like the coloured shaders.

export const SHADER_JOY_DIVISION = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise(vec2 p){
  vec2 i=floor(p),f=fract(p);
  vec2 u=f*f*(3.0-2.0*f);
  return mix(mix(hash(i),hash(i+vec2(1,0)),u.x),
             mix(hash(i+vec2(0,1)),hash(i+vec2(1,1)),u.x),u.y);
}
float fbm(vec2 p){
  float v=0.0,a=0.5;
  for(int i=0;i<5;i++){v+=a*noise(p);p*=2.02;a*=0.5;}
  return v;
}
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  vec3 col=vec3(0.0);
  const int N=80;
  float lineW=0.0014;
  float maxAmp=0.085;
  for(int i=0;i<N;i++){
    float t=float(i)/float(N-1);
    float baseY=mix(0.42,-0.42,t);
    float vEnv=0.45+0.55*exp(-pow((t-0.5)*2.4,2.0));
    float hEnv=exp(-pow(p.x*1.9,2.0));
    float n1=fbm(vec2(p.x*3.5+float(i)*0.7,float(i)*0.31+u_time*0.18));
    float n2=fbm(vec2(p.x*11.0+float(i)*1.3,float(i)*0.11+u_time*0.34));
    float wave=(n1-0.5)*1.6+(n2-0.5)*0.55;
    wave+=0.6*pow(max(0.0,fbm(vec2(p.x*6.0+float(i)*2.1,u_time*0.22+float(i)*0.5))-0.55),2.0);
    float y=baseY+wave*maxAmp*hEnv*vEnv;
    if(p.y<y) col=vec3(0.0);
    float d=abs(p.y-y);
    float aa=fwidth(p.y)*1.2;
    float ln=1.0-smoothstep(lineW,lineW+aa,d);
    col=mix(col,vec3(1.0),ln);
  }
  col*=1.0-0.18*dot(p,p);
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_OSCILLOSCOPE = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise(vec2 p){
  vec2 i=floor(p),f=fract(p);
  vec2 u=f*f*(3.0-2.0*f);
  return mix(mix(hash(i),hash(i+vec2(1,0)),u.x),
             mix(hash(i+vec2(0,1)),hash(i+vec2(1,1)),u.x),u.y);
}
float waveform(float x,float t){
  float a=sin(x*3.0+t*0.6)*0.35;
  float b=sin(x*7.0-t*0.9+1.3)*0.18;
  float c=sin(x*15.0+t*1.4)*0.07;
  float env=0.6+0.4*sin(t*0.27);
  float w=noise(vec2(x*2.0,t*0.4))-0.5;
  return (a+b+c+w*0.06)*env;
}
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  vec3 col=vec3(0.0);
  for(int k=0;k<3;k++){
    float fk=float(k);
    float y=waveform(p.x*(1.0-fk*0.03)+fk*1.7,u_time+fk*0.4)*0.32;
    float d=abs(p.y-y);
    float core=exp(-d*d/0.0006);
    float glow=exp(-d*d/0.012)*0.35;
    col+=vec3(core+glow)*(1.0-fk*0.18);
  }
  vec2 g=abs(fract(p*6.0)-0.5);
  col+=vec3(0.025)*(1.0-smoothstep(0.0,0.03,min(g.x,g.y)));
  col+=vec3(0.04)*(1.0-smoothstep(0.0,0.001,abs(p.x)));
  col+=vec3(0.04)*(1.0-smoothstep(0.0,0.001,abs(p.y)));
  col+=(hash(gl_FragCoord.xy+u_time*60.0)-0.5)*0.012;
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_SPECTRUM = /* glsl */ `
float hashF(float x){return fract(sin(x*127.1)*43758.5453);}
float hash2(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise1(float x){
  float i=floor(x),f=fract(x);
  return mix(hashF(i),hashF(i+1.0),f*f*(3.0-2.0*f));
}
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  float aspect=u_resolution.x/u_resolution.y;
  vec3 col=vec3(0.0);
  const float BARS=64.0;
  float xN=(p.x+aspect*0.5)/aspect;
  float bar=floor(xN*BARS);
  float inBar=fract(xN*BARS);
  float gap=0.18;
  float fnorm=bar/BARS;
  float falloff=pow(1.0-fnorm*0.85,1.4);
  float drift=noise1(bar*1.13+u_time*0.5)*0.7+noise1(bar*0.31+u_time*1.7)*0.3;
  float pk=pow(noise1(bar*2.7+u_time*2.3),6.0)*0.5;
  float h=(drift*0.55+pk*0.45)*falloff;
  float baseline=-0.42;
  float topY=baseline+h*0.85;
  float bm=step(gap*0.5,inBar)*step(inBar,1.0-gap*0.5);
  float inside=step(p.y,topY)*step(baseline,p.y);
  float vGrad=smoothstep(baseline,topY+0.001,p.y);
  vec3 barCol=mix(vec3(0.18),vec3(0.95),vGrad);
  barCol+=vec3(0.4)*(1.0-smoothstep(0.0,0.01,abs(p.y-topY)));
  col+=barCol*inside*bm;
  float peakHoldY=baseline+(h*0.85+0.04+0.02*sin(u_time+bar));
  col+=vec3(0.6)*(1.0-smoothstep(0.0,0.005,abs(p.y-peakHoldY)))*bm*step(0.05,h);
  col+=vec3(0.18)*(1.0-smoothstep(0.0,0.0015,abs(p.y-baseline)));
  col+=(hash2(gl_FragCoord.xy)-0.5)*0.008;
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_VINYL = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise(vec2 p){
  vec2 i=floor(p),f=fract(p);
  vec2 u=f*f*(3.0-2.0*f);
  return mix(mix(hash(i),hash(i+vec2(1,0)),u.x),
             mix(hash(i+vec2(0,1)),hash(i+vec2(1,1)),u.x),u.y);
}
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  float r=length(p);
  float a=atan(p.y,p.x);
  float aRot=a+u_time*0.55;
  float wob=noise(vec2(aRot*1.4,r*22.0))*0.0015;
  float groove=sin((r+wob)*220.0);
  float lineMask=smoothstep(0.6,1.0,abs(groove));
  float disc=1.0-smoothstep(0.46,0.48,r);
  float label=smoothstep(0.16,0.155,r);
  vec3 col=vec3(0.05);
  col=mix(col,vec3(0.07),disc);
  col-=vec3(0.04)*lineMask*disc;
  float spec=pow(max(0.0,cos(aRot+u_time*0.4)),32.0);
  spec*=smoothstep(0.0,0.42,r)*(1.0-smoothstep(0.42,0.48,r));
  col+=vec3(0.18)*spec;
  col+=vec3(0.07)*pow(max(0.0,cos(aRot+u_time*0.4+3.14159)),24.0)*smoothstep(0.05,0.42,r);
  col=mix(col,vec3(0.11),label);
  col+=vec3(0.06)*(1.0-smoothstep(0.0,0.0015,abs(r-0.16)));
  col=mix(col,vec3(0.0),smoothstep(0.012,0.010,r));
  col+=vec3(0.12)*(1.0-smoothstep(0.0,0.002,abs(r-0.47)));
  col+=(noise(p*80.0+u_time*0.05)-0.5)*0.012*disc;
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_TAPE = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float hash1(float x){return fract(sin(x*127.1)*43758.5453);}
float noise(vec2 p){
  vec2 i=floor(p),f=fract(p);
  vec2 u=f*f*(3.0-2.0*f);
  return mix(mix(hash(i),hash(i+vec2(1,0)),u.x),
             mix(hash(i+vec2(0,1)),hash(i+vec2(1,1)),u.x),u.y);
}
float fbm(vec2 p){
  float v=0.0,a=0.5;
  for(int i=0;i<4;i++){v+=a*noise(p);p*=2.1;a*=0.5;}
  return v;
}
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  float cloud=fbm(vec2(p.x*1.6,p.y*1.6+u_time*0.06));
  cloud+=0.5*fbm(vec2(p.x*4.0,p.y*4.0-u_time*0.04));
  float base=mix(0.06,0.18,cloud*0.65);
  base+=smoothstep(0.7,1.0,sin((uv.y-u_time*0.04)*380.0))*0.018;
  base+=pow(max(0.0,sin((uv.y-u_time*0.12)*12.0)),6.0)*0.04;
  float dr=hash1(floor(uv.y*200.0)+floor(u_time*4.0));
  base-=step(0.985,dr)*(1.0-smoothstep(0.0,0.4,fract(u_time*4.0)))*0.18;
  base+=(hash(gl_FragCoord.xy+floor(u_time*30.0))-0.5)*0.09;
  base*=1.0-0.4*dot(p,p);
  vec3 col=vec3(base)*vec3(1.02,1.0,0.97);
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_PHASING = /* glsl */ `
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  vec3 col=vec3(0.04);
  for(int k=0;k<2;k++){
    float fk=float(k);
    float t=u_time*(0.06+fk*0.012);
    vec2 gp=p;
    gp.x+=t*(0.6+fk*0.05);
    gp.y+=t*0.04*(fk+1.0);
    vec2 cell=fract(gp*26.0)-0.5;
    vec2 id=floor(gp*26.0);
    float pulse=0.5+0.5*sin(u_time*0.4+id.x*0.31+id.y*0.27+fk*1.5);
    float r=length(cell);
    float dotR=0.18+0.08*pulse;
    float aa=fwidth(r)*1.5;
    float dm=1.0-smoothstep(dotR-aa,dotR+aa,r);
    float bright=(k==0)?0.55:0.35;
    col+=vec3(bright)*dm;
  }
  col*=mix(0.85,1.05,0.5+0.5*sin(p.x*1.2+u_time*0.13)*cos(p.y*1.3-u_time*0.09));
  col+=(fract(sin(dot(gl_FragCoord.xy,vec2(127.1,311.7)))*43758.5453)-0.5)*0.008;
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_SPECTROGRAM = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise(vec2 p){
  vec2 i=floor(p),f=fract(p);
  vec2 u=f*f*(3.0-2.0*f);
  return mix(mix(hash(i),hash(i+vec2(1,0)),u.x),
             mix(hash(i+vec2(0,1)),hash(i+vec2(1,1)),u.x),u.y);
}
float fbm(vec2 p){
  float v=0.0,a=0.5;
  for(int i=0;i<4;i++){v+=a*noise(p);p*=2.04;a*=0.5;}
  return v;
}
void main(){
  vec2 uv=gl_FragCoord.xy/u_resolution;
  float t=uv.x*1.6-u_time*0.05;
  float f=uv.y;
  float w=pow(1.0-f,1.6)+0.08;
  float v=fbm(vec2(t*9.0,f*30.0))*w*0.85;
  v+=fbm(vec2(t*22.0,f*50.0))*w*0.4;
  v+=pow(noise(vec2(floor(t*30.0),floor(f*16.0))),8.0)*w*0.55;
  float lineF=0.55+0.18*sin(t*1.4)*cos(t*0.7);
  v+=exp(-pow((f-lineF)*40.0,2.0))*0.7;
  v=pow(max(v*0.9,0.0),0.85);
  vec3 col=vec3(v);
  col+=vec3(0.025)*step(0.99,abs(sin(f*3.14159*8.0)))*step(uv.x,0.012);
  col+=vec3(0.25)*smoothstep(0.998,1.0,uv.x);
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_LISSAJOUS = /* glsl */ `
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  float a=3.0+0.4*sin(u_time*0.07);
  float b=4.0+0.4*cos(u_time*0.05);
  float phi=u_time*0.25;
  float A=0.36,B=0.36;
  float minD=1e9;
  const int N=110;
  for(int i=0;i<N;i++){
    float ft=float(i)/float(N)*6.28318530718;
    vec2 q=vec2(A*sin(a*ft+phi),B*sin(b*ft));
    float d=length(p-q);
    if(d<minD) minD=d;
  }
  float core=exp(-minD*minD/0.0006);
  float halo=exp(-minD*minD/0.018)*0.32;
  float wide=exp(-minD*minD/0.08)*0.06;
  vec3 col=vec3(core+halo+wide);
  col+=vec3(0.025)*(1.0-smoothstep(0.0,0.0008,abs(p.x)));
  col+=vec3(0.025)*(1.0-smoothstep(0.0,0.0008,abs(p.y)));
  vec2 ap=abs(p);
  col+=vec3(0.05)*step(0.44,max(ap.x,ap.y))*step(max(ap.x,ap.y),0.445);
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_DRONE = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float noise(vec2 p){
  vec2 i=floor(p),f=fract(p);
  vec2 u=f*f*(3.0-2.0*f);
  return mix(mix(hash(i),hash(i+vec2(1,0)),u.x),
             mix(hash(i+vec2(0,1)),hash(i+vec2(1,1)),u.x),u.y);
}
float fbm(vec2 p){
  float v=0.0,a=0.5;
  for(int i=0;i<4;i++){v+=a*noise(p);p*=2.05;a*=0.5;}
  return v;
}
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  float v=0.0;
  for(int i=0;i<7;i++){
    float fi=float(i);
    float seed=fi*1.71;
    float baseY=mix(-0.42,0.42,fract(seed*0.61803));
    baseY+=0.02*sin(u_time*0.05+fi*0.7);
    float width=mix(0.05,0.16,fract(seed*1.31));
    float amp=(0.5+0.5*sin(u_time*0.06+fi*1.3))*(0.5+0.5*sin(u_time*0.041+fi*2.1));
    float dy=p.y-baseY-(fbm(vec2(p.x*1.4+u_time*0.04,fi*3.0))-0.5)*0.04;
    v+=exp(-dy*dy/(width*width))*amp;
  }
  v*=mix(0.85,1.05,exp(-pow(p.x*1.2,2.0)))*0.55;
  v+=(fbm(vec2(p.x*8.0,p.y*30.0+u_time*0.1))-0.5)*0.03*v;
  vec3 col=vec3(v);
  col+=(hash(gl_FragCoord.xy+u_time*40.0)-0.5)*0.012;
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_REEL = /* glsl */ `
float hash(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float reelDisk(vec2 p,vec2 c,float r,float rot){
  vec2 q=p-c;
  float rad=length(q);
  float ang=atan(q.y,q.x)+rot;
  float v=0.0;
  v+=1.0-smoothstep(0.002,0.004,abs(rad-r));
  v+=(1.0-smoothstep(0.002,0.004,abs(rad-r*0.32)))*0.8;
  v+=smoothstep(r*0.13,r*0.115,rad)*0.6;
  if(rad<r-0.005&&rad>r*0.32+0.005){
    v+=smoothstep(0.985,0.998,abs(sin(ang*3.0)))*0.85;
  }
  if(rad<r*0.96&&rad>r*0.55) v+=0.04;
  return v;
}
void main(){
  vec2 p=(gl_FragCoord.xy-0.5*u_resolution)/u_resolution.y;
  float r=0.18;
  float rot=u_time*0.9;
  float v=reelDisk(p,vec2(-0.34,0.0),r,rot)+reelDisk(p,vec2(0.34,0.0),r,rot*1.04);
  if(p.x>-0.34&&p.x<0.34){
    v+=1.0-smoothstep(0.001,0.0025,abs(p.y-r));
    v+=1.0-smoothstep(0.001,0.0025,abs(p.y+r));
    if(abs(p.y)<r) v+=0.018;
  }
  v+=(1.0-smoothstep(0.012,0.014,length(p)))*0.6;
  v*=1.0-0.35*dot(p,p);
  vec3 col=vec3(v);
  col+=(hash(gl_FragCoord.xy+u_time*50.0)-0.5)*0.012;
  col*=mix(vec3(1.0),u_color1*2.5,0.20);
  gl_FragColor=vec4(col,1.0);
}
`;

export const SHADER_STANDING_WAVE = /* glsl */ `
void main(){
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
}
`;

const PATTERN_HELPERS = /* glsl */ `
#define PI 3.14159265

float h21(vec2 p){
  p = fract(p * vec2(123.34, 456.21));
  p += dot(p, p + 45.32);
  return fract(p.x * p.y);
}

float vnoise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = h21(i);
  float b = h21(i + vec2(1.0, 0.0));
  float c = h21(i + vec2(0.0, 1.0));
  float d = h21(i + vec2(1.0, 1.0));
  vec2 u = f*f*(3.0-2.0*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}

float aaLine(float v, float w){
  float aa = fwidth(v);
  return 1.0 - smoothstep(w - aa, w + aa, abs(v));
}

void finishPattern(vec2 frag, vec2 uv, vec3 col, float k, float a){
  float vg = 1.0 - smoothstep(0.6, 1.2, length(uv));
  col = mix(col, u_color2, clamp(k, 0.0, 1.0) * 0.85);
  col = mix(col, u_color3, clamp(a, 0.0, 1.0));
  col *= mix(0.78, 1.0, vg);
  col += (h21(frag) - 0.5) / 255.0;
  gl_FragColor = vec4(col, 1.0);
}

void finishColor(vec2 frag, vec2 uv, vec3 col){
  float vg = 1.0 - smoothstep(0.7, 1.3, length(uv));
  col *= mix(0.7, 1.05, vg);
  col += (h21(frag) - 0.5) / 255.0;
  gl_FragColor = vec4(col, 1.0);
}
`;

function patternShader(body: string): string {
	return `${PATTERN_HELPERS}
void main(){
  vec2 frag = gl_FragCoord.xy;
  vec2 uv = (frag - 0.5 * u_resolution) / min(u_resolution.x, u_resolution.y);
  vec3 col = u_color1;
  float k = 0.0;
  float a = 0.0;
${body}
  finishPattern(frag, uv, col, k, a);
}
`;
}

export const SHADER_PATTERN_GRID = patternShader(/* glsl */ `
  float scale = 22.0;
  vec2 g = uv * scale + vec2(u_time * 0.06, -u_time * 0.04);
  vec2 q = fract(g) - 0.5;
  k = max(aaLine(q.x, 0.02), aaLine(q.y, 0.02));
  vec2 cell = floor(g);
  float bigX = aaLine(fract(cell.x / 5.0 + 0.5) - 0.5, 0.5/5.0) * aaLine(q.x, 0.04);
  float bigY = aaLine(fract(cell.y / 5.0 + 0.5) - 0.5, 0.5/5.0) * aaLine(q.y, 0.04);
  a = max(bigX, bigY);
`);

export const SHADER_PATTERN_DOTS = patternShader(/* glsl */ `
  vec2 p = uv * 18.0;
  vec2 r = vec2(1.0, 1.7320508);
  vec2 hp = mod(p, r) - 0.5 * r;
  vec2 hp2 = mod(p - 0.5 * r, r) - 0.5 * r;
  float d = min(length(hp), length(hp2));
  float aa = fwidth(d);
  k = 1.0 - smoothstep(0.16 - aa, 0.16 + aa, d);
  float t = fract(u_time * 0.08);
  a = (1.0 - smoothstep(0.0, 0.02, abs(length(uv) - t * 0.9))) * (1.0 - k);
`);

export const SHADER_PATTERN_HATCH = patternShader(/* glsl */ `
  float s = 16.0;
  float ang = 0.6;
  vec2 r = vec2(cos(ang), sin(ang));
  float v = dot(uv, r) * s + u_time * 0.4;
  vec2 r2 = vec2(cos(-ang), sin(-ang));
  float v2 = dot(uv, r2) * s - u_time * 0.25;
  k = max(aaLine(fract(v) - 0.5, 0.06), aaLine(fract(v2) - 0.5, 0.02) * 0.5);
  a = aaLine(fract(v * 0.2) - 0.5, 0.02);
`);

export const SHADER_PATTERN_TRUCHET = patternShader(/* glsl */ `
  vec2 p = uv * 9.0 + vec2(u_time * 0.05, 0.0);
  vec2 ip = floor(p);
  vec2 fp = fract(p) - 0.5;
  if (step(0.5, h21(ip)) > 0.5) fp.x = -fp.x;
  float d1 = abs(length(fp - vec2(-0.5, -0.5)) - 0.5);
  float d2 = abs(length(fp - vec2(0.5, 0.5)) - 0.5);
  float d = min(d1, d2);
  float aa = fwidth(d);
  k = 1.0 - smoothstep(0.06, 0.06 + aa*2.0, d);
  a = (1.0 - smoothstep(0.18, 0.18 + aa*2.0, d)) * (1.0 - k) * 0.35;
`);

export const SHADER_PATTERN_WAVES = patternShader(/* glsl */ `
  float r = length(uv);
  float bands = sin(r * 36.0 - u_time * 1.2);
  float k1 = smoothstep(0.0, 0.04, bands);
  float k2 = smoothstep(0.0, 0.04, sin(r * 36.0 - u_time * 1.2 - 0.4));
  k = k1 * 0.55;
  a = (k2 - k1) * 0.6;
`);

export const SHADER_PATTERN_NOISE = patternShader(/* glsl */ `
  vec2 p = uv * 3.0;
  float n = vnoise(p + vec2(u_time * 0.08, 0.0));
  float n2 = vnoise(p * 2.1 - vec2(0.0, u_time * 0.05));
  float v = n * 0.65 + n2 * 0.35;
  float band = fract(v * 6.0);
  k = smoothstep(0.45, 0.5, band) * (1.0 - smoothstep(0.5, 0.55, band));
  a = smoothstep(0.85, 0.9, v) * 0.6;
`);

export const SHADER_PATTERN_PLASMA = patternShader(/* glsl */ `
  vec2 p = uv * 3.2;
  float t = u_time * 0.6;
  float v = sin(p.x + t)
          + sin(p.y * 1.3 + t * 1.1)
          + sin((p.x + p.y) * 0.9 + t * 0.7)
          + sin(length(p) * 2.0 - t * 1.3);
  v *= 0.25;
  col = 0.5 + 0.5 * cos(6.2831 * (vec3(0.0, 0.33, 0.67) + v) + u_time * 0.2);
  col = mix(u_color1, col, 0.92);
  finishColor(frag, uv, col);
  return;
`);

export const SHADER_PATTERN_KALEIDO = patternShader(/* glsl */ `
  float ang = atan(uv.y, uv.x);
  float r = length(uv);
  float seg = PI / 4.0;
  ang = mod(ang, seg);
  ang = abs(ang - seg * 0.5);
  vec2 q = vec2(cos(ang), sin(ang)) * r * 3.0 + vec2(u_time * 0.15, -u_time * 0.1);
  float n = vnoise(q) * 0.6 + vnoise(q * 2.3 + 5.0) * 0.4;
  float band = fract(n * 5.0 + u_time * 0.2);
  vec3 rb = 0.5 + 0.5 * cos(6.2831 * (vec3(0.0, 0.33, 0.67) + r * 1.5 + n * 1.2 + u_time * 0.1));
  col = mix(u_color1, rb, 0.95);
  col *= mix(0.6, 1.1, band);
  finishColor(frag, uv, col);
  return;
`);

export const SHADER_PATTERN_TUNNEL = patternShader(/* glsl */ `
  float r = length(uv);
  float ang = atan(uv.y, uv.x);
  float u1 = 1.0 / max(r, 0.001) + u_time * 0.8;
  float v1 = ang * 6.0 / PI;
  float c = mod(step(0.5, fract(u1 * 0.5)) + step(0.5, fract(v1 * 0.5)), 2.0);
  vec3 rb = 0.5 + 0.5 * cos(6.2831 * (vec3(0.0, 0.33, 0.67) + u1 * 0.1 + u_time * 0.15));
  col = mix(u_color1, rb, 0.9);
  col *= mix(0.55, 1.05, c);
  col *= smoothstep(0.0, 0.6, r);
  finishColor(frag, uv, col);
  return;
`);

export const SHADER_PATTERN_MELT = patternShader(/* glsl */ `
  vec2 p = uv * 2.0;
  float t = u_time * 0.25;
  vec2 w = vec2(vnoise(p + vec2(t, 0.0)), vnoise(p + vec2(0.0, t) + 7.3));
  p += (w - 0.5) * 2.5;
  float n = vnoise(p * 1.6 + t);
  float band = fract(n * 4.0 + u_time * 0.15);
  vec3 rb = 0.5 + 0.5 * cos(6.2831 * (vec3(0.0, 0.33, 0.67) + n * 1.8 + u_time * 0.1));
  col = mix(u_color1, rb, 0.92);
  col = mix(col, vec3(1.0), smoothstep(0.92, 0.98, band) * 0.5);
  finishColor(frag, uv, col);
  return;
`);

export const SHADER_PATTERN_SPEED = patternShader(/* glsl */ `
  float ang = atan(uv.y, uv.x);
  float r = length(uv);
  float seg = 36.0;
  float idx = floor(ang / 6.2831 * seg + 0.5);
  float jitter = h21(vec2(idx, 0.0)) - 0.5;
  float a0 = (idx + jitter * 0.6) / seg * 6.2831;
  float dAng = ang - a0;
  float thick = 0.003 + r * 0.018 + jitter * 0.004;
  float ln = 1.0 - smoothstep(thick, thick + 0.004, abs(sin(dAng)) * r);
  k = ln * mix(0.6, 1.0, step(0.0, sin(u_time * 0.5 + idx * 0.7)));
  float wedge = step(0.94, abs(cos(ang * 6.0 - u_time * 0.6)));
  a = wedge * smoothstep(0.0, 0.4, r) * (1.0 - smoothstep(0.7, 1.0, r));
`);

export const SHADER_PATTERN_VORTEX = patternShader(/* glsl */ `
  float r = length(uv);
  float ang = atan(uv.y, uv.x);
  float v = sin(8.0 * ang + log(max(r, 0.001)) * 6.0 - u_time * 1.2);
  k = smoothstep(0.0, 0.05, v) * 0.9;
  a = smoothstep(0.85, 0.95, v) * 0.7;
  a += (1.0 - smoothstep(0.0, 0.08, r)) * 0.8;
`);

export const SHADER_PATTERN_SHARDS = patternShader(/* glsl */ `
  vec2 p = uv * 5.0 + vec2(u_time * 0.05, 0.0);
  vec2 ip = floor(p);
  vec2 fp = fract(p);
  float md = 1e9;
  float md2 = 1e9;
  for (int j = -1; j <= 1; j++) {
    for (int i = -1; i <= 1; i++) {
      vec2 g = vec2(float(i), float(j));
      vec2 o = vec2(h21(ip + g), h21(ip + g + 17.3));
      o = 0.5 + 0.5 * sin(u_time * 0.4 + 6.2831 * o);
      vec2 d = g + o - fp;
      float dist = dot(d, d);
      if (dist < md) { md2 = md; md = dist; }
      else if (dist < md2) { md2 = dist; }
    }
  }
  float edge = sqrt(md2) - sqrt(md);
  float aa = fwidth(edge);
  k = 1.0 - smoothstep(0.04, 0.04 + aa * 2.0, edge);
  a = (1.0 - smoothstep(0.0, 0.12, sqrt(md))) * 0.6;
`);

export const SHADER_PATTERN_VECTOR = patternShader(/* glsl */ `
  float ang = atan(uv.y, uv.x);
  float radials = aaLine(fract(ang / 6.2831 * 16.0) - 0.5, 0.04);
  float r = length(uv);
  float rings = aaLine(fract(log(max(r, 0.001)) * 2.5 + u_time * 0.6) - 0.5, 0.05);
  k = max(radials, rings);
  a = (1.0 - smoothstep(0.0, 0.025, r)) * 0.9;
`);

// ─── Signature 1.0 shaders ───────────────────────────────────────────────────
// Five new looks distinct from everything above: a cinematic black hole, a
// kaleidoscopic fractal, refractive stained glass, curl-noise silk, and the
// first true-3D raymarched shader in the set. All palette-driven and interactive.

export const SHADER_BLACKHOLE = /* glsl */ `
#define PI 3.14159265
float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i), b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.)), d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){ float v=0., a=0.5; for(int i=0;i<5;i++){ v+=a*noise(p); p*=2.02; a*=0.5; } return v; }

void main(){
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = (u_mouse*u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;

  // Cursor nudges the whole system so the hole feels draggable without leaving frame.
  vec2 c = m * 0.35;
  vec2 q = p - c;
  float r = length(q);

  // Light bending: pull the sample toward the hole, strongest near the ring.
  float bend = 0.06 / (r + 0.04);
  vec2 sq = q - normalize(q + 1e-5) * bend * 0.05;
  float sa = atan(sq.y, sq.x);

  // Accretion disk seen edge-on, so squash it vertically. Swirls and rotates.
  float diskR = length(vec2(sq.x, sq.y * 3.2));
  float swirl = fbm(vec2(sa*2.0 + u_time*0.4, diskR*6.0 - u_time*0.6));
  float disk = smoothstep(0.42, 0.18, diskR) * smoothstep(0.12, 0.18, diskR);
  disk *= 0.6 + 0.8*swirl;
  disk *= 1.0 + 0.9*cos(sa);              // relativistic beaming: one side brighter

  float ring = exp(-pow((r - 0.12)*26.0, 2.0));   // photon ring
  float shadow = smoothstep(0.115, 0.10, r);       // event-horizon silhouette

  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 cl = u_clicks[i];
    vec2 cp = (cl.xy*u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y - c;
    float d = distance(q, cp);
    disk += exp(-pow((d - cl.z*0.5)*7.0, 2.0)) * exp(-cl.z*0.8) * 1.5;
  }

  vec3 hot = mix(u_color2, u_color3, 0.6);
  vec3 col = vec3(0.003, 0.004, 0.01);
  col += hot * disk * 1.4;
  col += mix(u_color3, vec3(1.0), 0.5) * ring * (1.2 + u_mouseDown*1.5);
  col *= 1.0 - shadow;
  col += hot * exp(-r*6.0) * 0.15;       // soft bloom toward the ring

  col += (hash(gl_FragCoord.xy + u_time) - 0.5) * 0.02;
  col *= 1.0 - dot(p, p) * 0.35;
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_KIFS = /* glsl */ `
#define PI 3.14159265
mat2 rot(float a){ float c=cos(a), s=sin(a); return mat2(c,-s,s,c); }

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = (u_mouse*u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;
  p *= 1.4;

  // Kaleidoscopic IFS: fold, rotate, scale a fixed number of times. The cursor
  // rotates the fold plane so the whole jewel reorganizes as you move.
  float trap = 1e9;
  float scale = 1.0;
  mat2 R = rot(u_time*0.08 + m.x*0.6);
  vec2 off = vec2(0.9, 0.6) + m*0.3;
  for(int i=0;i<10;i++){
    p = abs(p);
    p = R * p;
    p = p*1.35 - off;
    scale *= 1.35;
    trap = min(trap, length(p - vec2(0.2)));
  }

  float d = trap / scale;                 // scale-corrected orbit trap
  float shade = exp(-d*6.0);
  float bands = 0.5 + 0.5*sin(log(d + 0.001)*3.0 - u_time*0.5);

  vec3 col = mix(u_color1, u_color2, shade);
  col = mix(col, u_color3, bands*shade);
  col += u_color4 * pow(shade, 4.0) * (0.5 + u_mouseDown);

  for(int i=0;i<8;i++){
    if(i>=u_clickCount) break;
    vec3 cl = u_clicks[i];
    vec2 cp = (cl.xy*u_resolution.xy - 0.5*u_resolution.xy) / u_resolution.y;
    float dd = distance((gl_FragCoord.xy - 0.5*u_resolution.xy)/u_resolution.y, cp);
    col += u_color3 * exp(-dd*8.0) * exp(-cl.z*1.5);
  }

  col *= 1.0 - dot(uv - 0.5, uv - 0.5) * 0.5;
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_VORONOI_GLASS = /* glsl */ `
float h21(vec2 p){ p = fract(p*vec2(123.34, 456.21)); p += dot(p, p + 45.32); return fract(p.x*p.y); }
vec2 h22(vec2 p){ return vec2(h21(p), h21(p + 17.3)); }

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = (gl_FragCoord.xy - 0.5*u_resolution.xy) / min(u_resolution.x, u_resolution.y);
  vec2 mm = u_mouse - 0.5; mm.x *= u_resolution.x / u_resolution.y;

  vec2 g = p * 5.0;
  vec2 ip = floor(g), fp = fract(g);
  float f1 = 1e9, f2 = 1e9;
  vec2 id1 = vec2(0.0);
  for(int j=-1;j<=1;j++){
    for(int i=-1;i<=1;i++){
      vec2 o = vec2(float(i), float(j));
      vec2 cen = o + 0.5 + 0.4*sin(u_time*0.3 + 6.2831*h22(ip + o));
      float d = length(cen - fp);
      if(d < f1){ f2 = f1; f1 = d; id1 = ip + o; }
      else if(d < f2){ f2 = d; }
    }
  }

  float lead = smoothstep(0.06, 0.0, f2 - f1);   // bright leading between panes
  float bevel = smoothstep(0.0, 0.5, f1);         // fake glass thickness -> specular

  float sel = h21(id1);
  vec3 glass = mix(u_color1, u_color2, sel);
  glass = mix(glass, u_color3, smoothstep(0.6, 1.0, sel));
  glass = mix(glass, u_color4, smoothstep(0.85, 1.0, h21(id1 + 3.1)));
  glass *= 0.55 + 0.7*bevel;

  vec3 col = mix(glass, vec3(0.02, 0.02, 0.03), lead);   // dark leading came
  col += lead * mix(u_color3, vec3(1.0), 0.5) * 0.15;     // sheen on the came
  col += exp(-distance(p, mm)*3.0) * (0.2 + u_mouseDown*0.8) * mix(u_color3, vec3(1.0), 0.4) * 0.3;

  col *= 1.0 - dot(uv - 0.5, uv - 0.5) * 0.5;
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_CURL_FLOW = /* glsl */ `
float hash(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float noise(vec2 p){
  vec2 i = floor(p), f = fract(p);
  float a = hash(i), b = hash(i + vec2(1.,0.));
  float c = hash(i + vec2(0.,1.)), d = hash(i + vec2(1.,1.));
  vec2 u = f*f*(3.-2.*f);
  return mix(mix(a,b,u.x), mix(c,d,u.x), u.y);
}
float fbm(vec2 p){ float v=0., a=0.5; for(int i=0;i<4;i++){ v+=a*noise(p); p*=2.0; a*=0.5; } return v; }
vec2 curl(vec2 p){
  float e = 0.01;
  float x = fbm(p + vec2(0.0, e)) - fbm(p - vec2(0.0, e));
  float y = fbm(p + vec2(e, 0.0)) - fbm(p - vec2(e, 0.0));
  return vec2(x, -y) / (2.0*e);
}

void main(){
  vec2 uv = gl_FragCoord.xy / u_resolution.xy;
  vec2 p = uv; p.x *= u_resolution.x / u_resolution.y;
  vec2 m = u_mouse; m.x *= u_resolution.x / u_resolution.y;

  // Advect the sample along the curl of an fbm field to draw silky streaks.
  vec2 pos = p;
  float phase = 0.0;
  for(int i=0;i<6;i++){
    vec2 v = curl(pos*1.5 + u_time*0.05);
    vec2 tm = pos - m;                      // cursor injects a vortex
    v += vec2(-tm.y, tm.x) * (0.15 / (dot(tm, tm) + 0.05)) * (0.5 + u_mouseDown);
    pos += v * 0.03;
    phase += length(v);
  }

  float silk = pow(0.5 + 0.5*sin(phase*3.0 + pos.x*8.0 - u_time*0.6), 1.5);
  float speed = clamp(phase*0.15, 0.0, 1.0);

  vec3 col = mix(u_color1, u_color2, silk);
  col = mix(col, u_color3, speed*0.7);
  col += u_color4 * pow(silk, 4.0) * 0.4;
  col += exp(-distance(p, m)*5.0) * (0.15 + u_mouseDown*0.7) * mix(u_color3, vec3(1.0), 0.4) * 0.4;

  col *= 1.0 - dot(uv - 0.5, uv - 0.5) * 0.4;
  gl_FragColor = vec4(col, 1.0);
}
`;

export const SHADER_RAYMARCH_LATTICE = /* glsl */ `
mat2 rot(float a){ float c=cos(a), s=sin(a); return mat2(c,-s,s,c); }
float hash21(vec2 p){ return fract(sin(dot(p, vec2(127.1,311.7))) * 43758.5453); }
float map(vec3 p){
  p *= 2.0;
  return (abs(dot(sin(p), cos(p.zxy))) - 0.6) * 0.4;   // thin gyroid shell, lipschitz-tamed
}
vec3 calcNormal(vec3 p){
  vec2 e = vec2(0.001, 0.0);
  return normalize(vec3(
    map(p + e.xyy) - map(p - e.xyy),
    map(p + e.yxy) - map(p - e.yxy),
    map(p + e.yyx) - map(p - e.yyx)));
}

void main(){
  vec2 uv = (gl_FragCoord.xy - 0.5*u_resolution.xy) / u_resolution.y;
  vec2 m = u_mouse - 0.5;

  vec3 ro = vec3(0.0, 0.0, u_time*0.4);      // fly forward through the lattice
  vec3 rd = normalize(vec3(uv, 1.0));
  rd.yz = rot(m.y*0.6) * rd.yz;               // cursor tilts the camera
  rd.xz = rot(-m.x*0.6) * rd.xz;

  float t = 0.0, glow = 0.0, dh = 0.0;
  bool hit = false;
  for(int i=0;i<64;i++){
    float d = map(ro + rd*t);
    if(d < 0.001){ hit = true; dh = t; break; }
    glow += 0.02 / (1.0 + d*d*20.0);
    t += clamp(d, 0.02, 0.3);
    if(t > 12.0) break;
  }

  vec3 col = vec3(0.0);
  if(hit){
    vec3 pp = ro + rd*dh;
    vec3 n = calcNormal(pp);
    float diff = clamp(dot(n, normalize(vec3(0.5, 0.8, -0.4))), 0.0, 1.0);
    float fres = pow(1.0 - clamp(dot(n, -rd), 0.0, 1.0), 3.0);
    float fog = exp(-dh*0.18);
    vec3 base = mix(u_color1, u_color2, diff);
    base = mix(base, u_color3, fres);
    col = base*fog + u_color4*fres*fog*0.6;
  }
  col += u_color3 * glow * 0.4;                // volumetric glow through the shell
  col += u_mouseDown * u_color4 * glow * 0.3;

  col += (hash21(gl_FragCoord.xy) - 0.5) * 0.02;
  col *= 1.0 - dot(uv, uv) * 0.25;
  gl_FragColor = vec4(col, 1.0);
}
`;

export type WallpaperId = 'none' | 'aurora' | 'chrome' | 'grid' | 'nebula' | 'topo'
                        | 'topo-noir' | 'aurora-deep' | 'chrome-brushed'
                        | 'zen' | 'galaxy'
                        | 'blackhole' | 'kifs' | 'voronoi-glass' | 'curl-flow' | 'raymarch-lattice'
                        | 'joy-division' | 'oscilloscope' | 'spectrum' | 'vinyl' | 'tape'
                        | 'phasing' | 'spectrogram' | 'lissajous' | 'drone' | 'reel'
                        | 'standing-wave'
                        | 'pattern-grid' | 'pattern-dots' | 'pattern-hatch'
                        | 'pattern-truchet' | 'pattern-waves' | 'pattern-noise'
                        | 'pattern-plasma' | 'pattern-kaleido' | 'pattern-tunnel'
                        | 'pattern-melt' | 'pattern-speed' | 'pattern-vortex'
                        | 'pattern-shards' | 'pattern-vector';

export interface WallpaperOption {
	id: WallpaperId;
	label: string;
	sublabel: string;
	shader: string | null;
	/** When true, hidden in settings behind a "More" toggle. */
	extended?: boolean;
}

export const WALLPAPERS: WallpaperOption[] = [
	{ id: 'none', label: 'None', sublabel: 'Default gradient background', shader: null },
	{ id: 'aurora', label: 'Aurora Field', sublabel: 'Volumetric ribbons · cursor bends the flow', shader: SHADER_AURORA },
	{ id: 'chrome', label: 'Liquid Chrome', sublabel: 'Reflective metaballs track the cursor', shader: SHADER_CHROME },
	{ id: 'grid', label: 'Plasma Grid', sublabel: 'Holographic terrain warped by the cursor', shader: SHADER_GRID },
	{ id: 'nebula', label: 'Deep Nebula', sublabel: 'Starfield with gravitational lensing', shader: SHADER_NEBULA },
	{ id: 'topo', label: 'Topographic Flow', sublabel: 'Ink-on-bone contour field, cursor pulls', shader: SHADER_TOPO },
	{ id: 'topo-noir', label: 'Topo Noir', sublabel: 'Dark contour field · glowing palette lines', shader: SHADER_TOPO_NOIR },
	{ id: 'aurora-deep', label: 'Aurora Deep', sublabel: 'Pure-black field · sharp luminous ribbons', shader: SHADER_AURORA_DEEP },
	{ id: 'chrome-brushed', label: 'Chrome Brushed', sublabel: 'Brushed metal · chromatic aberration', shader: SHADER_CHROME_BRUSHED },
	{ id: 'zen',    label: 'Zen Water',     sublabel: 'Calm caustic ripples · cursor stirs the surface',             shader: SHADER_ZEN },
	{ id: 'galaxy', label: 'Spiral Galaxy', sublabel: 'Logarithmic arms · differential rotation · cursor bends gravity', shader: SHADER_GALAXY },
	{ id: 'blackhole',      label: 'Event Horizon', sublabel: 'Accretion disk · photon ring · cursor bends the light',      shader: SHADER_BLACKHOLE },
	{ id: 'kifs',           label: 'Fracture',      sublabel: 'Kaleidoscopic fractal jewel · cursor folds space',           shader: SHADER_KIFS },
	{ id: 'voronoi-glass',  label: 'Stained Glass', sublabel: 'Refractive glass panes · cursor lights the leading',         shader: SHADER_VORONOI_GLASS },
	{ id: 'curl-flow',      label: 'Silk',          sublabel: 'Curl-noise flow · cursor stirs a vortex',                     shader: SHADER_CURL_FLOW },
	{ id: 'pattern-speed',   label: 'Speed',   sublabel: 'Futurist radial force lines',        shader: SHADER_PATTERN_SPEED },
	{ id: 'pattern-vortex',  label: 'Vortex',  sublabel: 'Rotating logarithmic arms',          shader: SHADER_PATTERN_VORTEX },
	{ id: 'pattern-shards',  label: 'Shards',  sublabel: 'Cellular fractured shards',          shader: SHADER_PATTERN_SHARDS },
	{ id: 'pattern-vector',  label: 'Vector',  sublabel: 'Converging perspective grid',        shader: SHADER_PATTERN_VECTOR },
	{ id: 'pattern-plasma',  label: 'Plasma',  sublabel: 'Four-sine psychedelic plasma',       shader: SHADER_PATTERN_PLASMA },
	{ id: 'pattern-kaleido', label: 'Kaleido', sublabel: 'Eight-fold mirrored noise swirl',    shader: SHADER_PATTERN_KALEIDO },
	{ id: 'pattern-tunnel',  label: 'Tunnel',  sublabel: 'Polar checker zoom tunnel',          shader: SHADER_PATTERN_TUNNEL },
	{ id: 'pattern-melt',    label: 'Melt',    sublabel: 'Domain-warped color flow',           shader: SHADER_PATTERN_MELT },
	{ id: 'pattern-grid',    label: 'Grid',    sublabel: 'Soft anti-aliased pattern grid',     shader: SHADER_PATTERN_GRID,    extended: true },
	{ id: 'pattern-dots',    label: 'Dots',    sublabel: 'Hex dot lattice with a pulse ring',  shader: SHADER_PATTERN_DOTS,    extended: true },
	{ id: 'pattern-hatch',   label: 'Hatch',   sublabel: 'Animated diagonal crosshatch',       shader: SHADER_PATTERN_HATCH,   extended: true },
	{ id: 'pattern-truchet', label: 'Truchet', sublabel: 'Quarter-arc tiled weave',            shader: SHADER_PATTERN_TRUCHET, extended: true },
	{ id: 'pattern-waves',   label: 'Waves',   sublabel: 'Concentric sine bands',              shader: SHADER_PATTERN_WAVES,   extended: true },
	{ id: 'pattern-noise',   label: 'Noise',   sublabel: 'Value-noise contour bands',          shader: SHADER_PATTERN_NOISE,   extended: true },
	// Extended: music-themed monochrome shaders
	{ id: 'joy-division',   label: 'Unknown Pleasures', sublabel: 'Stacked pulsar ridgelines · Saville 1979',          shader: SHADER_JOY_DIVISION,   extended: true },
	{ id: 'oscilloscope',   label: 'Oscilloscope',      sublabel: 'Three phosphor traces · soft bloom',                shader: SHADER_OSCILLOSCOPE,   extended: true },
	{ id: 'spectrum',       label: 'Spectrum',           sublabel: '64-bar EQ analyzer · pink-noise weighted',          shader: SHADER_SPECTRUM,       extended: true },
	{ id: 'vinyl',          label: 'Vinyl Grooves',      sublabel: '33⅓ rpm · sweeping specular highlight',             shader: SHADER_VINYL,          extended: true },
	{ id: 'tape',           label: 'Tape Static',        sublabel: 'Oxide grain · horizontal band dropout',             shader: SHADER_TAPE,           extended: true },
	{ id: 'phasing',        label: 'Phasing Dots',       sublabel: 'Two dot grids at different phase rates · moiré',    shader: SHADER_PHASING,        extended: true },
	{ id: 'spectrogram',    label: 'Spectrogram',        sublabel: 'Waterfall spectrogram · melodic peak wanders',      shader: SHADER_SPECTROGRAM,    extended: true },
	{ id: 'lissajous',      label: 'Lissajous',          sublabel: 'Drifting frequency ratio · evolving figure',        shader: SHADER_LISSAJOUS,      extended: true },
	{ id: 'drone',          label: 'Drone Bands',        sublabel: 'Seven gaussian bands beating against each other',   shader: SHADER_DRONE,          extended: true },
	{ id: 'reel',           label: 'Reel to Reel',       sublabel: 'Two tape reels · six-spoke hubs · tape path',       shader: SHADER_REEL,           extended: true },
	{ id: 'standing-wave',  label: 'Standing Wave',      sublabel: 'Two-source interference · drifting source points',  shader: SHADER_STANDING_WAVE,  extended: true },
	// Raymarched 3D: the heaviest shader in the set, so it lives behind "More".
	{ id: 'raymarch-lattice', label: 'Lattice',          sublabel: 'Raymarched gyroid fly-through · cursor tilts the camera', shader: SHADER_RAYMARCH_LATTICE, extended: true },
];

export function wallpaperById(id: WallpaperId): WallpaperOption {
	return WALLPAPERS.find((w) => w.id === id) ?? WALLPAPERS[0];
}
