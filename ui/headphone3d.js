// A 3D headphone in the style of the Sony WH-1000XM5, drawn procedurally with three.js.
// Original geometry made for this app: no Sony model, photo, texture or logo is used.
// The proportions were matched by eye against reference views of the real product.
//
// What makes it read as a product render is the lighting, not the shape: physically based
// materials, studio reflections (RoomEnvironment), tone mapping and a soft cast shadow.
//
// Motion: the headphone rises and the cups swing open from lying flat to the "hero" pose,
// with a small overshoot. Then it idles. It is interactive: drag to turn it (it keeps a little
// momentum and drifts back), hover to tilt it, tap for a pulse, double-tap to reset.
import * as THREE from 'three';
import { RoomEnvironment } from './vendor/RoomEnvironment.js';

const COLORS = {
  black:  { body: 0x1b1c1f, cushion: 0x111214, plate: 0x151618, rough: 0.56 },
  silver: { body: 0xd8d5ce, cushion: 0x2b2b2d, plate: 0xcbc7bf, rough: 0.52 },
  blue:   { body: 0x27364d, cushion: 0x0f131a, plate: 0x202d42, rough: 0.55 },
  pink:   { body: 0xd0aaa6, cushion: 0x2a2224, plate: 0xc09b97, rough: 0.52 },
};

const LED = {
  nearby:     { color: 0x2f9bff, hz: 1.2 },
  connecting: { color: 0x2f9bff, hz: 3.2 },
  connected:  { color: 0x30d158, hz: 0 },
  error:      { color: 0xff453a, hz: 0 },
};

/** How far each cup is swivelled in the hero pose, so the pad openings show from the front. */
const REST_SWIVEL = 0.42;
/** Cups start nearly flat, facing the viewer, and open up to REST_SWIVEL. */
const FLAT_SWIVEL = 1.45;

const spow = (x, e) => Math.sign(x) * Math.abs(x) ** e;
const clamp01 = (x) => Math.min(1, Math.max(0, x));
const easeOutCubic = (x) => 1 - (1 - x) ** 3;
const easeOutBack = (x) => {
  const c1 = 1.9, c3 = c1 + 1;
  return 1 + c3 * (x - 1) ** 3 + c1 * (x - 1) ** 2;
};
const lerp = (a, b, t) => a + (b - a) * t;
const wrapPi = (a) => Math.atan2(Math.sin(a), Math.cos(a));

/** Rounded, slightly boxy pod. The thickness axis is X, so the flat faces point sideways. */
function superEllipsoid(a, b, c, e1, e2, taper = 0, nu = 72, nv = 56) {
  const pos = [], uv = [], idx = [];
  for (let j = 0; j <= nv; j++) {
    const v = -Math.PI / 2 + (Math.PI * j) / nv;
    const sv = Math.sin(v), cv = Math.cos(v);
    for (let i = 0; i <= nu; i++) {
      const u = -Math.PI + (2 * Math.PI * i) / nu;
      const y = b * spow(cv, e1) * spow(Math.cos(u), e2);
      // A little narrower toward the bottom, like the real cups.
      pos.push(a * spow(sv, e1), y, c * spow(cv, e1) * spow(Math.sin(u), e2) * (1 + taper * (y / b)));
      uv.push(i / nu, j / nv);
    }
  }
  for (let j = 0; j < nv; j++) {
    for (let i = 0; i < nu; i++) {
      const p = j * (nu + 1) + i, q = p + 1, r = p + nu + 1, s = r + 1;
      idx.push(p, q, r, q, s, r);
    }
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  g.setAttribute('uv', new THREE.Float32BufferAttribute(uv, 2));
  g.setIndex(idx);
  g.computeVertexNormals();
  // computeVertexNormals leaves a visible seam where the grid wraps, and degenerate poles.
  const n = g.attributes.normal;
  for (let j = 0; j <= nv; j++) {
    const first = j * (nu + 1), last = first + nu;
    const nx = (n.getX(first) + n.getX(last)) / 2, ny = (n.getY(first) + n.getY(last)) / 2, nz = (n.getZ(first) + n.getZ(last)) / 2;
    n.setXYZ(first, nx, ny, nz);
    n.setXYZ(last, nx, ny, nz);
  }
  for (let i = 0; i <= nu; i++) {
    n.setXYZ(i, -1, 0, 0);
    n.setXYZ(nv * (nu + 1) + i, 1, 0, 0);
  }
  return g;
}

/** Sweep a rounded-rectangle profile along an elliptical arc in the XY plane (the headband). */
function bandGeometry({ rx, ry, cy, thick, width, radius, from, to, segs = 120, corner = 6 }) {
  const profile = [];
  const hp = thick / 2, hz = width / 2, r = radius;
  const corners = [[hp - r, hz - r, 0], [-(hp - r), hz - r, 90], [-(hp - r), -(hz - r), 180], [hp - r, -(hz - r), 270]];
  for (const [cp, cz, start] of corners) {
    for (let k = 0; k <= corner; k++) {
      const a = ((start + (90 * k) / corner) * Math.PI) / 180;
      profile.push([cp + r * Math.cos(a), cz + r * Math.sin(a)]);
    }
  }
  const m = profile.length;
  const pos = [], idx = [];
  for (let s = 0; s <= segs; s++) {
    const th = lerp(from, to, s / segs);
    const px = rx * Math.cos(th), py = cy + ry * Math.sin(th);
    const nl = Math.hypot(ry * Math.cos(th), rx * Math.sin(th));
    const nx = (ry * Math.cos(th)) / nl, ny = (rx * Math.sin(th)) / nl;
    for (const [p, z] of profile) pos.push(px + nx * p, py + ny * p, z);
  }
  for (let s = 0; s < segs; s++) {
    for (let k = 0; k < m; k++) {
      const a = s * m + k, b = s * m + ((k + 1) % m), c = (s + 1) * m + k, d = (s + 1) * m + ((k + 1) % m);
      idx.push(a, b, c, b, d, c);
    }
  }
  const g = new THREE.BufferGeometry();
  g.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  g.setIndex(idx);
  g.computeVertexNormals();
  return g;
}

export function createHeadphoneView(container) {
  const renderer = new THREE.WebGLRenderer({ alpha: true, antialias: true, powerPreference: 'low-power' });
  renderer.setClearColor(0x000000, 0);
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2));
  renderer.outputColorSpace = THREE.SRGBColorSpace;
  renderer.toneMapping = THREE.ACESFilmicToneMapping;
  renderer.toneMappingExposure = 1.2;
  renderer.shadowMap.enabled = true;
  renderer.shadowMap.type = THREE.PCFSoftShadowMap;
  renderer.domElement.style.cssText = 'width:100%;height:100%;display:block;touch-action:none;cursor:grab';
  container.appendChild(renderer.domElement);

  const scene = new THREE.Scene();
  const pmrem = new THREE.PMREMGenerator(renderer);
  scene.environment = pmrem.fromScene(new RoomEnvironment(), 0.035).texture;
  scene.environmentIntensity = 0.95;

  const camera = new THREE.PerspectiveCamera(26, 1, 0.1, 50);
  camera.position.set(0, 0.5, 7.1);
  camera.lookAt(0, -0.34, 0);

  const key = new THREE.DirectionalLight(0xffffff, 1.9);
  key.position.set(2.5, 5, 4);
  key.castShadow = true;
  key.shadow.mapSize.set(1024, 1024);
  key.shadow.bias = -0.0006;
  key.shadow.radius = 6;
  const rim = new THREE.DirectionalLight(0x86b6ff, 1.6);
  rim.position.set(3, 1.2, -3.5);
  scene.add(key, rim);

  // Materials: a satin finish like the real product, soft sheen on the pads, polished copper.
  const body = new THREE.MeshPhysicalMaterial({ roughness: 0.58, metalness: 0.04, clearcoat: 0.1, clearcoatRoughness: 0.55 });
  const plateMat = new THREE.MeshPhysicalMaterial({ roughness: 0.66, metalness: 0.02 });
  const cushion = new THREE.MeshPhysicalMaterial({ roughness: 0.6, metalness: 0, sheen: 0.6, sheenRoughness: 0.8, sheenColor: new THREE.Color(0x3a3a3a) });
  const dark = new THREE.MeshStandardMaterial({ color: 0x0b0c0e, roughness: 0.7 });
  const copper = new THREE.MeshStandardMaterial({ color: 0xc88a65, roughness: 0.25, metalness: 0.9 });
  const ledMat = new THREE.MeshBasicMaterial({ color: LED.nearby.color });

  const model = new THREE.Group();
  scene.add(model);

  // ---- band ----
  const RX = 0.99, RY = 1.1, CY = 0.0;
  model.add(new THREE.Mesh(bandGeometry({ rx: RX, ry: RY, cy: CY, thick: 0.25, width: 0.3, radius: 0.1, from: -0.07, to: Math.PI + 0.07 }), body));
  // The soft pad under the band.
  const bandPad = new THREE.Mesh(bandGeometry({ rx: RX, ry: RY, cy: CY, thick: 0.11, width: 0.25, radius: 0.05, from: 0.5, to: Math.PI - 0.5 }), cushion);
  bandPad.scale.set(0.93, 0.93, 1);
  bandPad.position.y = -0.005;
  model.add(bandPad);

  // ---- shared geometry ----
  const cupGeo = superEllipsoid(0.33, 0.76, 0.64, 0.36, 0.78, -0.1);
  const armGeo = new THREE.CapsuleGeometry(0.04, 0.5, 8, 16);
  const hingeGeo = new THREE.CylinderGeometry(0.075, 0.075, 0.14, 24);
  const padGeo = new THREE.TorusGeometry(0.47, 0.15, 28, 96);
  const discGeo = new THREE.CircleGeometry(0.46, 56);
  const plateGeo = new THREE.CircleGeometry(0.5, 72);
  const dotGeo = new THREE.CircleGeometry(0.016, 12);
  const ledGeo = new THREE.SphereGeometry(0.03, 16, 12);
  const pillGeo = new THREE.CapsuleGeometry(0.014, 0.1, 6, 12);

  const cups = [];
  for (const side of [-1, 1]) {
    // side -1 = left cup, whose outer face points to -X.
    const hinge = new THREE.Group();
    hinge.position.set(side * RX, 0.0, 0);
    model.add(hinge);

    // Slim flat arm that splays out a little, like the real slider.
    const arm = new THREE.Mesh(armGeo, dark);
    arm.scale.set(0.9, 1, 1.6);
    arm.position.y = -0.3;
    arm.rotation.z = side * 0.05;
    hinge.add(arm);

    const swivel = new THREE.Group(); // rotates about the vertical axis of the arm
    swivel.position.y = -0.1;
    hinge.add(swivel);
    swivel.add(new THREE.Mesh(hingeGeo, dark));

    const cup = new THREE.Group();
    // The arm lies along the cup's outer face, and the cup swings open around the arm.
    cup.position.set(-side * 0.29, -0.8, 0);
    cup.rotation.z = -side * 0.08; // slight toe-in at the bottom
    swivel.add(cup);
    cup.add(new THREE.Mesh(cupGeo, body));

    const inward = -side;
    // Pad: a thick soft ring on the inner side, with the driver cloth visible inside it.
    const pad = new THREE.Mesh(padGeo, cushion);
    pad.rotation.y = Math.PI / 2;
    pad.scale.set(1.0, 1.28, 1);
    pad.position.x = inward * 0.32;
    cup.add(pad);
    const cloth = new THREE.Mesh(discGeo, dark);
    cloth.rotation.y = (inward * Math.PI) / 2;
    cloth.scale.set(0.8, 1.14, 1);
    cloth.position.x = inward * 0.37;
    cup.add(cloth);

    // Outer face: a big flat touch disc, a little below the middle, in a slightly different matte.
    const plate = new THREE.Mesh(plateGeo, plateMat);
    plate.rotation.y = (side * Math.PI) / 2;
    plate.scale.set(1.0, 1.2, 1);
    plate.position.set(side * 0.333, -0.09, 0.02);
    cup.add(plate);

    // Microphone holes and the copper accents on the front edge of the cup.
    for (const [dy, dz] of [[0.34, 0.16], [0.28, -0.1]]) {
      const dot = new THREE.Mesh(dotGeo, dark);
      dot.rotation.y = (side * Math.PI) / 2;
      dot.position.set(side * 0.335, dy + 0.16, dz);
      cup.add(dot);
    }
    for (const [py, px] of [[0.05, 0.13], [-0.52, 0.09]]) {
      const pill = new THREE.Mesh(pillGeo, copper);
      pill.position.set(side * px, py, 0.57);
      pill.scale.set(0.8, 1, 0.8);
      cup.add(pill);
    }
    if (side === -1) {
      const led = new THREE.Mesh(ledGeo, ledMat);
      led.position.set(side * 0.27, -0.62, 0.48);
      cup.add(led);
    }
    cups.push({ side, swivel, plate });
  }

  // A real soft shadow, cast on an invisible floor under the floating headphone.
  const floor = new THREE.Mesh(new THREE.PlaneGeometry(14, 14), new THREE.ShadowMaterial({ opacity: 0.3 }));
  floor.rotation.x = -Math.PI / 2;
  floor.position.y = -1.7;
  floor.receiveShadow = true;
  scene.add(floor);
  model.traverse((o) => { if (o.isMesh) o.castShadow = true; });

  let phase = 'nearby';
  let running = false;
  let raf = 0;
  let introStart = performance.now();
  let frozen = false; // for still renders: no idle motion
  const hover = { x: 0, y: 0, tx: 0, ty: 0 };
  // User turning: yaw and pitch added to the idle pose.
  const user = { yaw: 0, pitch: 0, vy: 0, vp: 0, last: -1e9 };
  let drag = null;
  let pulse = 0;

  function setColor(name) {
    const c = COLORS[name] || COLORS.black;
    body.color.setHex(c.body);
    body.roughness = c.rough;
    cushion.color.setHex(c.cushion);
    plateMat.color.setHex(c.plate);
  }

  function resize() {
    const w = container.clientWidth || 1, h = container.clientHeight || 1;
    renderer.setSize(w, h, false);
    camera.aspect = w / h;
    camera.updateProjectionMatrix();
  }
  const ro = new ResizeObserver(resize);
  ro.observe(container);
  resize();

  // ---- interaction ----
  const el = renderer.domElement;
  el.addEventListener('pointerdown', (e) => {
    drag = { x: e.clientX, y: e.clientY, moved: 0, lx: e.clientX, ly: e.clientY };
    user.vy = user.vp = 0;
    el.setPointerCapture(e.pointerId);
    el.style.cursor = 'grabbing';
  });
  el.addEventListener('pointermove', (e) => {
    const r = el.getBoundingClientRect();
    hover.tx = ((e.clientX - r.left) / r.width - 0.5) * 2;
    hover.ty = ((e.clientY - r.top) / r.height - 0.5) * 2;
    if (!drag) return;
    const dx = e.clientX - drag.lx, dy = e.clientY - drag.ly;
    drag.lx = e.clientX; drag.ly = e.clientY;
    drag.moved += Math.abs(dx) + Math.abs(dy);
    user.yaw += dx * 0.012;
    user.pitch = Math.max(-0.55, Math.min(0.55, user.pitch + dy * 0.008));
    user.vy = dx * 0.012;
    user.vp = dy * 0.008;
    user.last = performance.now();
  });
  const release = () => {
    if (!drag) return;
    if (drag.moved < 5) pulse = 1; // a tap, not a drag
    drag = null;
    user.last = performance.now();
    el.style.cursor = 'grab';
  };
  el.addEventListener('pointerup', release);
  el.addEventListener('pointercancel', release);
  el.addEventListener('pointerleave', () => { hover.tx = hover.ty = 0; });
  el.addEventListener('dblclick', () => { user.yaw = wrapPi(user.yaw); user.vy = user.vp = 0; user.last = -1e9; pulse = 1; });

  function frame(now) {
    const t = (now - introStart) / 1000;
    const p = clamp01(t / 2.3);

    // Intro: rise and turn to the hero pose, while the cups swing open from lying flat.
    const rise = easeOutCubic(clamp01(p / 0.5));
    const turn = easeOutCubic(clamp01(p / 0.9));
    const unfold = easeOutBack(clamp01((p - 0.24) / 0.58));
    const swivel = lerp(FLAT_SWIVEL, REST_SWIVEL, unfold);
    cups.forEach((c) => { c.swivel.rotation.y = c.side * swivel; });

    hover.x = lerp(hover.x, hover.tx, 0.08);
    hover.y = lerp(hover.y, hover.ty, 0.08);

    // The user's turning keeps a little momentum, then eases back to the idle pose.
    if (!drag) {
      user.yaw += user.vy; user.pitch += user.vp;
      user.vy *= 0.93; user.vp *= 0.93;
      if (now - user.last > 2200) {
        user.yaw = lerp(wrapPi(user.yaw), 0, 0.035);
        user.pitch = lerp(user.pitch, 0, 0.035);
      }
    }
    pulse *= 0.9;

    const idle = frozen ? 0 : Math.max(0, t - 1.9);
    const busy = phase === 'connecting' && !frozen ? 0.22 * Math.sin(t * 3.0) : 0;
    const shake = phase === 'error' && !frozen && t % 3 < 0.5 ? 0.05 * Math.sin(t * 60) : 0;
    const base = -0.3;
    model.rotation.y = lerp(-1.3, base, turn) + 0.12 * Math.sin(idle * 0.7) * clamp01(idle) + hover.x * 0.25 + busy + shake + user.yaw;
    model.rotation.x = 0.03 * Math.sin(idle * 0.5) + hover.y * 0.08 + user.pitch;
    const bob = frozen ? 0 : 0.045 * Math.sin(idle * 1.1);
    model.position.y = lerp(-0.9, 0, rise) + bob;
    model.scale.setScalar(lerp(0.84, 1, rise) * (1 + 0.05 * pulse));
    floor.material.opacity = 0.3 * rise;
    renderer.domElement.style.opacity = String(clamp01(p / 0.08));

    scene.environmentRotation.y = t * 0.16;

    const led = LED[phase] || LED.nearby;
    ledMat.color.setHex(led.color);
    const flash = pulse > 0.05 ? 1 + 2 * pulse : 1;
    const blink = led.hz ? 0.55 + 0.45 * Math.sin(t * led.hz * Math.PI * 2) : 1;
    ledMat.color.multiplyScalar(blink * flash);

    renderer.render(scene, camera);
    if (running) raf = requestAnimationFrame(frame);
  }

  return {
    start() {
      if (running) return;
      running = true;
      raf = requestAnimationFrame(frame);
    },
    /** Stop drawing. A menu-bar app must not keep a WebGL loop alive in a hidden window. */
    stop() {
      running = false;
      cancelAnimationFrame(raf);
    },
    replayIntro() {
      introStart = performance.now();
    },
    setColor,
    setPhase(p) { phase = p; },
    /** One still frame, for checking the shape. `pose` sets the turn in radians. */
    renderOnce(at = 2, pose = {}) {
      frozen = true;
      introStart = performance.now() - at * 1000;
      user.yaw = pose.yaw ?? 0;
      user.pitch = pose.pitch ?? 0;
      user.last = performance.now();
      frame(performance.now());
    },
  };
}
