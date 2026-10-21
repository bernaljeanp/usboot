import { useEffect, useRef } from "react";

// The light bar: light behind a hexagonal vent grille, like the LED strip on the
// back of the laptop. Breathes when idle, sweeps while working, red while deleting.
const FRAG = `
precision mediump float;
uniform vec2 r;
uniform float t;
uniform vec3 c;
uniform float busy;
const vec2 S = vec2(1.0, 1.7320508);
vec4 hex(vec2 uv){
  vec2 h = S * .5;
  vec2 a = mod(uv, S) - h;
  vec2 b = mod(uv - h, S) - h;
  vec2 gv = dot(a, a) < dot(b, b) ? a : b;
  return vec4(gv, uv - gv);
}
float hexDist(vec2 p){ p = abs(p); return max(dot(p, normalize(S)), p.x); }
void main(){
  vec2 uv = gl_FragCoord.xy / r;
  float cell = 6.0;
  vec4 h = hex(gl_FragCoord.xy / cell);
  float hole = smoothstep(.40, .33, hexDist(h.xy));
  float x = h.z * cell / r.x;
  float idle = .55 + .45 * sin(t * .9 - x * 4.0);
  float sweep = exp(-pow((fract(x - t * .45) - .5) * 7.0, 2.0));
  float k = mix(idle * .55, .18 + sweep * 1.35, busy);
  float ends = smoothstep(0., .18, uv.x) * smoothstep(1., .82, uv.x);
  vec3 metal = vec3(.078, .086, .096) + .02 * uv.y;
  vec3 light = c * (k * (.35 + .65 * ends) + .06);
  gl_FragColor = vec4(mix(metal, metal * .25 + light, hole), 1.);
}`;

const VERT = `attribute vec2 a;void main(){gl_Position=vec4(a,0.,1.);}`;

// Window backdrop: brushed graphite whose sheen drifts very slowly, plus a faint
// spill of the light bar's colour from the top edge. Deliberately near-invisible.
const BACKDROP = `
precision mediump float;
uniform vec2 r;
uniform float t;
uniform vec3 c;
float h(vec2 p){return fract(sin(dot(p,vec2(127.1,311.7)))*43758.5453);}
float n(vec2 p){vec2 i=floor(p),f=fract(p);f=f*f*(3.-2.*f);
  return mix(mix(h(i),h(i+vec2(1,0)),f.x),mix(h(i+vec2(0,1)),h(i+vec2(1,1)),f.x),f.y);}
void main(){
  vec2 uv = gl_FragCoord.xy / r;
  float top = 1.0 - uv.y;
  // brushing: noise stretched along x
  float brush = n(vec2(gl_FragCoord.x * .004, gl_FragCoord.y * .9)) * .5 + n(vec2(gl_FragCoord.x * .01, gl_FragCoord.y * 2.3)) * .5;
  // a broad sheen band that drifts across the panel over ~40s
  float sheen = exp(-pow((uv.x - fract(t * .025) * 1.6 + .3) * 2.2 - top * .6, 2.0));
  vec3 base = vec3(.102, .114, .129);
  vec3 col = base + (brush - .5) * .012 + sheen * .018;
  float spill = exp(-top * 6.0) * (.75 + .25 * sin(t * .4 + uv.x * 3.0));
  col += c * spill * .045;
  col += (h(gl_FragCoord.xy + fract(t)) - .5) * .006;
  gl_FragColor = vec4(col, 1.);
}`;

/** Full-window backdrop canvas. 20 fps, half resolution, still frame for reduced motion. */
export function Backdrop({ color }: { color: Rgb }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const live = useRef(color);
  live.current = color;

  useEffect(() => {
    const canvas = ref.current;
    const gl = canvas?.getContext("webgl", { antialias: false });
    if (!canvas || !gl) return;
    const compile = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return s;
    };
    const prog = gl.createProgram()!;
    gl.attachShader(prog, compile(gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, BACKDROP));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return;
    gl.useProgram(prog);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const a = gl.getAttribLocation(prog, "a");
    gl.enableVertexAttribArray(a);
    gl.vertexAttribPointer(a, 2, gl.FLOAT, false, 0, 0);
    const [uR, uT, uC] = ["r", "t", "c"].map((n) => gl.getUniformLocation(prog, n));

    const resize = () => {
      canvas.width = Math.max(1, Math.floor(innerWidth / 2));
      canvas.height = Math.max(1, Math.floor(innerHeight / 2));
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform2f(uR, canvas.width, canvas.height);
    };
    resize();
    addEventListener("resize", resize);

    const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const start = performance.now();
    let raf = 0;
    let last = 0;
    const frame = (now: number) => {
      raf = requestAnimationFrame(frame);
      if (document.hidden || now - last < 50) return;
      last = now;
      gl.uniform1f(uT, still ? 8 : (now - start) / 1000);
      gl.uniform3f(uC, ...live.current);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    raf = requestAnimationFrame(frame);
    return () => {
      cancelAnimationFrame(raf);
      removeEventListener("resize", resize);
    };
  }, []);

  return <canvas ref={ref} className="backdrop" aria-hidden />;
}

export type Rgb = [number, number, number];

export default function LightBar({ color, busy }: { color: Rgb; busy: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const live = useRef({ color, busy: busy ? 1 : 0 });
  live.current.color = color;

  useEffect(() => {
    const canvas = ref.current;
    const gl = canvas?.getContext("webgl", { antialias: false });
    if (!canvas || !gl) return;

    const compile = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return s;
    };
    const prog = gl.createProgram()!;
    gl.attachShader(prog, compile(gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, compile(gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return;
    gl.useProgram(prog);

    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const a = gl.getAttribLocation(prog, "a");
    gl.enableVertexAttribArray(a);
    gl.vertexAttribPointer(a, 2, gl.FLOAT, false, 0, 0);
    const u = (n: string) => gl.getUniformLocation(prog, n);
    const [uR, uT, uC, uB] = [u("r"), u("t"), u("c"), u("busy")];

    const resize = () => {
      const dpr = Math.min(devicePixelRatio || 1, 2);
      canvas.width = Math.max(1, Math.floor(canvas.clientWidth * dpr));
      canvas.height = Math.max(1, Math.floor(canvas.clientHeight * dpr));
      gl.viewport(0, 0, canvas.width, canvas.height);
      gl.uniform2f(uR, canvas.width, canvas.height);
    };
    const ro = new ResizeObserver(resize);
    ro.observe(canvas);
    resize();

    const still = matchMedia("(prefers-reduced-motion: reduce)").matches;
    const start = performance.now();
    let raf = 0;
    let last = 0;
    let shownBusy = 0;
    const draw = (now: number) => {
      // ease busy in/out so the sweep fades rather than snaps
      shownBusy += (live.current.busy - shownBusy) * 0.08;
      gl.uniform1f(uT, still ? 4 : (now - start) / 1000);
      gl.uniform3f(uC, ...live.current.color);
      gl.uniform1f(uB, still ? live.current.busy : shownBusy);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
    };
    const frame = (now: number) => {
      raf = requestAnimationFrame(frame);
      if (document.hidden || now - last < 33) return;
      last = now;
      draw(now);
    };
    raf = requestAnimationFrame(frame);

    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  useEffect(() => {
    live.current.busy = busy ? 1 : 0;
  }, [busy]);

  return <canvas ref={ref} className="lightbar" aria-hidden />;
}
