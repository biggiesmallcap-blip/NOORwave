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

export type WallpaperId = 'none' | 'aurora' | 'chrome' | 'grid' | 'nebula' | 'topo'
                        | 'topo-noir' | 'aurora-deep' | 'chrome-brushed'
                        | 'zen' | 'galaxy';

export interface WallpaperOption {
	id: WallpaperId;
	label: string;
	sublabel: string;
	shader: string | null;
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
	{ id: 'galaxy', label: 'Spiral Galaxy', sublabel: 'Logarithmic arms · differential rotation · cursor bends gravity', shader: SHADER_GALAXY }
];

export function wallpaperById(id: WallpaperId): WallpaperOption {
	return WALLPAPERS.find((w) => w.id === id) ?? WALLPAPERS[0];
}
