import { useEffect, useRef } from "react";
import type { Rgb } from "./Shader";

// The drive as a honeycomb of sectors, like the old defrag map revived: cells ignite as the
// write head passes, the head itself burns white-hot, and verification sweeps a cooler light
// over what was written. Same hex grille as the light bar, so it reads as one machine.
const FRAG = `
precision mediump float;
uniform vec2 r;
uniform float t;
uniform vec3 c;
uniform float written;
uniform float verified;
uniform float reserve;
const vec2 S = vec2(1.0, 1.7320508);
vec4 hex(vec2 uv){
  vec2 h = S * .5;
  vec2 a = mod(uv, S) - h;
  vec2 b = mod(uv - h, S) - h;
  vec2 gv = dot(a, a) < dot(b, b) ? a : b;
  return vec4(gv, uv - gv);
}
float hexDist(vec2 p){ p = abs(p); return max(dot(p, normalize(S)), p.x); }
float rnd(vec2 p){ return fract(sin(dot(p, vec2(12.9898, 78.233))) * 43758.5453); }
void main(){
  float cell = 15.0;
  vec4 h = hex(gl_FragCoord.xy / cell);
  vec2 id = h.zw;
  vec2 grid = r / cell / S;
  // order cells like a disk: left to right, top to bottom
  float row = floor((r.y / cell - id.y) / S.y * 2.0 + .5);
  float rows = floor(grid.y * 2.0) + 1.0;
  float k = (row + clamp(id.x * cell / r.x, 0., 1.)) / rows;
  float body = smoothstep(.46, .40, hexDist(h.xy));
  float n = rnd(id);

  float shade = gl_FragCoord.y / r.y;
  vec3 metal = vec3(.07, .078, .088) + .025 * shade;
  vec3 cold = vec3(.135, .15, .165) + .035 * n + .03 * shade;
  vec3 col = cold;
  // where the image will go: a slow breathing outline until it's written
  float ring = smoothstep(.36, .40, hexDist(h.xy)) * smoothstep(.47, .43, hexDist(h.xy));
  bool landing = k < reserve && k >= written;
  if (landing) col = mix(cold, c * .55, .38 + .12 * sin(t * 2.0 - k * 9.0));
  if (k < written) {
    // freshly written cells cool from white-hot to the light colour
    float age = clamp((written - k) * 22.0, 0., 1.);
    vec3 hot = mix(vec3(1., .95, .8), c, age);
    col = mix(c * .55, hot, (1. - age) * .9 + .1) * (.85 + .25 * n);
  }
  if (k < verified) {
    float age = clamp((verified - k) * 18.0, 0., 1.);
    col = mix(vec3(.95, 1., 1.), c * .9 + .08, age);
  }
  // the head: a bright pulsing cell where the work is happening
  float head = verified > 0. ? verified : written;
  float d = abs(k - head) * rows;
  float working = step(.0001, head) * step(head, reserve - .0001);
  col += c * exp(-d * d * 9.0) * (1.2 + .4 * sin(t * 14.0)) * working;
  // faint glow bleeding between cells
  float glow = smoothstep(.62, .40, hexDist(h.xy)) * .35 * step(k, max(written, verified));
  vec3 outc = mix(metal, col, body) + c * glow * (1. - body) * .4;
  if (landing) outc += c * ring * (.55 + .2 * sin(t * 2.0 - k * 9.0));
  gl_FragColor = vec4(outc, 1.);
}`;

const VERT = `attribute vec2 a;void main(){gl_Position=vec4(a,0.,1.);}`;

export default function SectorMap({ color, written, verified, reserve }: { color: Rgb; written: number; verified: number; reserve: number }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const live = useRef({ color, written, verified, reserve });
  live.current = { color, written, verified, reserve };

  useEffect(() => {
    const el = canvas.current!;
    const gl = el.getContext("webgl", { antialias: false, premultipliedAlpha: false });
    if (!gl) return;
    const shader = (type: number, src: string) => {
      const s = gl.createShader(type)!;
      gl.shaderSource(s, src);
      gl.compileShader(s);
      return s;
    };
    const prog = gl.createProgram()!;
    gl.attachShader(prog, shader(gl.VERTEX_SHADER, VERT));
    gl.attachShader(prog, shader(gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(prog);
    gl.useProgram(prog);
    gl.bindBuffer(gl.ARRAY_BUFFER, gl.createBuffer());
    gl.bufferData(gl.ARRAY_BUFFER, new Float32Array([-1, -1, 3, -1, -1, 3]), gl.STATIC_DRAW);
    const a = gl.getAttribLocation(prog, "a");
    gl.enableVertexAttribArray(a);
    gl.vertexAttribPointer(a, 2, gl.FLOAT, false, 0, 0);
    const u = (n: string) => gl.getUniformLocation(prog, n);
    const [ur, ut, uc, uw, uv, ures] = ["r", "t", "c", "written", "verified", "reserve"].map(u);
    const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

    let frame = 0;
    const draw = (ms: number) => {
      const dpr = Math.min(devicePixelRatio, 2);
      const w = Math.round(el.clientWidth * dpr);
      const h = Math.round(el.clientHeight * dpr);
      if (el.width !== w || el.height !== h) (el.width = w), (el.height = h);
      gl.viewport(0, 0, w, h);
      const { color, written, verified, reserve } = live.current;
      gl.uniform2f(ur, w, h);
      gl.uniform1f(ut, reduced ? 0 : ms / 1000);
      gl.uniform3f(uc, ...color);
      gl.uniform1f(uw, written);
      gl.uniform1f(uv, verified);
      gl.uniform1f(ures, reserve);
      gl.drawArrays(gl.TRIANGLES, 0, 3);
      frame = requestAnimationFrame(draw);
    };
    frame = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(frame);
  }, []);

  return <canvas ref={canvas} className="sector-map" aria-hidden />;
}
