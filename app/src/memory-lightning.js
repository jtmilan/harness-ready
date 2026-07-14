// Memory-graph "Second Brain" WebGL view — grown from a port of the Base44
// "Synapse" reference app's canvas renderer (constants extracted verbatim from
// its bundle — see .claude/context/2026-07-05-force-directed-brain-graph.md and
// the scratchpad base44/SPEC.md): pulsing category-colored gradient orbs on a
// #08080f field with a faint 60px grid, connections drawn as smooth OSCILLATOR
// waves (per-edge traveling sinusoids, endpoints pinned — replaced the
// reference's 150ms jitter re-snap by operator request), synced alpha
// breathing, sparse spawn-and-travel particles along the straight chords, and a
// relaxation physics layout (repulsion r120 / springs rest200 / damping .92)
// that settles to a calm equilibrium — the scene's "life" is pulse + waves +
// particles, not positional drift. Hard links carry everything (flow, halo,
// thunder); the memory engine's heuristic "suggested" edges draw as a FAINT,
// thin, static overlay (suggSys — clearly tentative, no flow, never struck)
// and contribute springs, exactly the original design. The whole cloud yaws
// slowly about the view center ("planetary" presentation): the 2D sim gains a
// stable per-node z (category shells + jitter), a pivot Group spins on the
// GPU, and labels/hover consume the yaw-projected x (lightning-core yawX) so
// DOM chips + hit-tests track the rotation exactly.
//
// Carrier is Three.js (vendored fat-line bundle), NOT canvas 2D — same visuals
// via the SPEC §H mapping (gradients/shadowBlur → sprite shaders + soft
// passes). Blending map, all deliberate: base wires + halo are ALPHA-OVER and
// ONE instance per edge (joint round-cap overlap beaded under every blend
// mode); the jagged strike family is premultiplied MAX-blend (overlaps
// SATURATE — additive fallback without EXT_blend_minmax); the sel overlay
// stays additive. Slot dim/clear works by scaling vertex colors toward black —
// black adds nothing under additive and never wins a max. Other named
// deviations, all deliberate: dt-capped physics (no 120Hz double-speed), plain
// labels with FULL titles, hover isolation + click-vs-drag slop + zoom-scaled
// hit pads (absent in the original), our category keys under the original's
// hex palette, tightened node halos (2.5× sprite, weaker glow) for dense
// stores, plasma-sphere orb shading (offset hot core, chromatic rim fringe,
// single-tail bloom) and a static atmosphere quad in place of flat black.
//
// This module is the RENDERER + local interactions only: layoutGraph output
// (pixel coords, y-down) seeds the sim; it owns a canvas + a DOM label layer
// inside #graph-body. Pure color math lives in lightning-core.js, the sim in
// force-core.js; integration (create/dispose, edit panel, persistence) lives in
// main.js. Data-in only — no fetch, no invoke.
//
// createLightningGraph({ container, layout, width, height, onHover, onSelect,
// onCreateAt, seedPositions }) returns { dispose(), setSelected(id|null),
// getPositions() }, or null when a WebGL context cannot be created or the build
// fails — the caller falls back to the static SVG view. There is no resize
// handle: a viewport change invalidates the layout, so the caller re-layouts
// and re-creates (positions survive via getPositions → seedPositions). WKWebView
// caps live WebGL contexts per page (see MAX_WEBGL in main.js), so dispose()
// really releases the context (renderer.dispose() + forceContextLoss()), never
// just detaches DOM.
//
// onHover (optional): fired ONLY when the hovered node CHANGES — onHover(node,
// { x, y }) with the laid node object passed through verbatim and its SCREEN
// anchor (world × view); onHover(null) when the pointer leaves every node's hit
// circle (or the canvas). Never fired per frame, never debounced.
import * as THREE from "/vendor/three-lightning.mjs";
import { categoryColor, electricColor, hexToRgb01, jagOffsets1D, nodeZOffset, rollBranch, strikeEnvelope, strikeOvershoot, yawDepth, yawX } from "./lightning-core.js";
import { createForceSim, stepForceSim, pinNode, releaseNode, settleForceSim } from "./force-core.js";

// ---- reference constants (SPEC.md §A–§F; verbatim unless marked) -------------
const PIXEL_RATIO_CAP = 2; // ours: retina is plenty; >2 just burns fill rate
const CLEAR_COLOR = 0x08080f; // the reference field color
// Camera sits FAR back with a deep frustum: the planetary yaw swings world z
// by ±(cloud radius + Z depth band) — hundreds of px — and the near/far planes
// must never clip the turning cloud. Orthographic → the distance itself has
// zero visual effect (no perspective), only the clip range matters.
const CAMERA_Z = 2000;
const CAMERA_FAR = 4000; // visible world z ∈ (CAMERA_Z−FAR, CAMERA_Z−0.1] = (−2000, 1999.9]

// Connections: straight lines that carry FLOWING ELECTRIC CURRENT — a bright
// Gaussian "packet" rides each wire from the FROM node to the TO node, looping,
// so the base line itself reads as current in a cable (operator request; the
// ambient jag/oscillator looks were removed earlier). The occasional THUNDER
// STRIKES stay as the dramatic accent on top of the steady flow.
// STRIKE jag slab granularity ONLY. Base wires are ONE fat-line instance per
// edge (16 collinear instances put a round-cap overlap at every interior joint
// — a row of ~15 beads no blending mode fully hides; the flow wave that needed
// per-station colors lives in the patched shader now) and the sel overlay
// draws single full-chord segments. Only the jagged strike/branch polylines
// still use the slab.
const BOLT_SEGMENTS = 16;
const BOLT_STATIONS = BOLT_SEGMENTS + 1; // slab layout (strike jag stations)
const LINK_WIDTH_PX = 1.2; // idle stroke
const SEL_WIDTH_PX = 2.5; // bolts touching the selected node
const SEL_OPACITY = 0.8;
const SEL_MAX = 24; // selected-bolt overlay slot capacity

// THUNDER STRIKES — the drama layer. Every so often one random link FIRES: a
// jagged bolt snaps across it instantly, re-strikes twice (each re-strike
// rolls a fresh jag shape AND fresh fork branches — the flicker), then fades.
// The cinematic grammar, FOUR pooled passes over one jag slab, dying inside
// out — white-hot instant core → blue-white heat bleed → wide violet wash:
//   CORE   — white-hot 3px line; cubic ease-out fade, DEAD at STRIKE_CORE_END
//   GLOW   — inner heat bleed: ~7px, hot blue-white, SHARING the core's
//            positions; lingers ~180ms past the core (STRIKE_GLOW_END)
//   AURA   — ~12px deep-violet gradient hugging the bolt, the LAST light to
//            die (~285ms past core, STRIKE_AURA_END = the strike's retirement)
// Both glow layers are shaped by a smooth TRANSVERSE falloff (bright spine →
// edges melting to zero — installMaxPremultFalloffPatch), so they read as
// light around the filament, never as flat ribbons.
//   BRANCH — 1–2 short fork arcs off interior stations of the main bolt:
//            same jag algorithm, 25–45% of the parent span, thinner + dimmer,
//            re-rolled with the parent and fading with its core
// Both glow layers POP at birth and at every re-strike (strikeOvershoot — a
// camera-flash ×1.4 easing back) before settling into their cubic tails.
// Colors come from the FIXED electric ramp (lightning-core.js): white-hot →
// blue-white → violet as the envelope decays — independent of the edge's
// category hue, so electricity always reads as electricity. All four render
// premultiplied MAX-blend (installMaxPremultPatch): the jag's round caps
// still fill elbow gaps, but cap/segment overlaps SATURATE instead of sum —
// no joint beads — and zeroed slots stay invisible (black never wins a max),
// so retired slots keep stale positions. Without EXT_blend_minmax (WebGL1-
// only contexts) they keep plain additive: glints return, nothing breaks.
const STRIKE_MAX = 5; // concurrent strikes — thunder hits OFTEN (operator request)
const STRIKE_DUR_MS = 950; // flash → flicker → afterglow, total
const STRIKE_GAP_MIN_MS = 450;
const STRIKE_GAP_VAR_MS = 850;
const STRIKE_WIDTH_PX = 3;
const STRIKE_JAG_RATIO = 0.12; // jag amplitude vs edge length …
const STRIKE_JAG_MAX_PX = 30; // … capped
const STRIKE_HOLD = 0.1; // fraction of the duration at full blaze before decay
const STRIKE_REROLLS = [0.1, 0.22]; // re-strike moments (fresh jag + branches)
const STRIKE_CORE_END = 0.85; // core is DEAD here (~807ms) — the glow layers own everything after
// INNER GLOW — the bolt's heat bleeding out: NARROW, hot, blue-white, hugging
// the arc. Shaped by the transverse falloff (installMaxPremultFalloffPatch):
// its opacity is the CENTERLINE peak; brightness melts smoothly to zero at
// the stroke edge. Lingers a beat past the core.
const STRIKE_GLOW_WIDTH_PX = 7; // heat-bleed width (core is 3px) — hugs the bolt
const STRIKE_GLOW_OPACITY = 0.8; // CENTERLINE peak (falloff-shaped) — taste ≈ 0.7–0.9
const STRIKE_GLOW_RAMP_T = 0.65; // blue-white heat (ramp: 0 = pure violet … 1 = white)
const STRIKE_GLOW_END = 1.04; // dies ≈ 180ms after the core (k units of STRIKE_DUR_MS)
const STRIKE_GLOW_SHIMMER = 0.3; // how much of the core's stepped flicker echoes here
// OUTER AURA — the "shady border": a NARROW deep-violet gradient wrapping the
// bolt, falloff-shaped like the glow (bright spine, edges melting to nothing
// — smooth shading, not a ribbon), and the LAST light to die: every strike
// leaves a dying violet trace hugging where the bolt was.
const STRIKE_AURA_WIDTH_PX = 12; // narrow drama — taste range ≈ 10–16
const STRIKE_AURA_OPACITY = 0.3; // CENTERLINE peak (falloff-shaped) — taste ≈ 0.22–0.4
const STRIKE_AURA_RAMP_T = 0.12; // pinned deep in the violet end of the ramp
const STRIKE_AURA_END = 1.15; // dies ≈ 285ms after the core — the strike RETIRES here
const STRIKE_AURA_SHIMMER = 0.15; // calmer flicker echo — the wash dies smoothly
// CAMERA-FLASH POP — at birth and at every re-strike, BOTH glow layers
// overshoot to ×(1 + BOOST) and ease back over WINDOW of the strike's life
// (cubic — strikeOvershoot in lightning-core.js) before settling into their
// tails: the light POPS, then lingers. The core keeps its approved envelope.
const STRIKE_FLASH_BOOST = 0.4; // ×1.4 at the flash instant (taste range ≈ 0.3–0.5)
const STRIKE_FLASH_WINDOW = 0.15; // each pop decays over 15% of the life (~140ms)
const STRIKE_FLASH_EVENTS = Object.freeze([0, ...STRIKE_REROLLS]); // birth + the re-strikes
const BRANCH_WIDTH_PX = 1.6; // forks are thinner than the 3px core
const BRANCH_GAIN = 0.6; // fork brightness vs the parent core
const BRANCH_SECOND_P = 0.5; // chance a strike forks twice instead of once
// Fork rolling ranges (see rollBranch in lightning-core.js): bud station on
// the parent (interior only), angle off the chord (radians, either sign),
// reach as a fraction of the parent span. Frozen + hoisted — never re-created.
const BRANCH_ROLL = Object.freeze({ tMin: 0.25, tMax: 0.75, angMin: 0.35, angMax: 0.95, lenMin: 0.25, lenMax: 0.45 });
// Idle alpha breathes 0.15–0.35 in sync across ALL bolts: 0.25 + sin(time*3)*0.1
const BOLT_ALPHA_BASE = 0.25;
const BOLT_ALPHA_AMP = 0.1;
const BOLT_ALPHA_SPEED = 3; // rad/s
// Suggested edges are OURS (no reference sibling): the deterministic suggest()
// heuristic (shared tags + shared link targets + token overlap) drawn as a
// FAINT, thin, breathing overlay — clearly tentative next to the hard links.
// Fat lines can't dash, so faint + thin + static (no flow wave, never a
// thunder target) is the differentiator. Slow half-breath via the tick.
const SUGGESTED_OPACITY_BASE = 0.12;
const SUGGESTED_OPACITY_AMP = 0.05;
const SUGGESTED_WIDTH_PX = 1;

// PLANETARY ROTATION — the whole memory cloud yaws slowly about the vertical
// axis through the view center, "like planetary stuff in 3d" (operator). The
// force sim stays strictly 2D; depth comes from a STABLE per-node z-offset
// (category shells + jitter — see nodeZOffset in lightning-core.js) and the
// yaw is a scene-graph Group rotation: buffers stay sim-space, the GPU turns
// them (zero per-frame CPU beyond one rotation.y write + the label/hover
// projection the node loop already pays for). Grid + atmosphere stay FIXED
// as background. Rotation PAUSES while a node is being dragged (the pointer→
// sim inverse stays stable) and under reduced motion (a small fixed tilt
// keeps the 3D readable without any animation).
const ROTATE_ON = true; // master switch for the ambient yaw
const ROTATE_SECS_PER_REV = 75; // one full revolution — slow + ambient (taste ≈ 60–90)
const REDUCED_MOTION_YAW = 0.35; // rad — the static tilt of the no-animation frame
const Z_SPREAD = 220; // category shells span this world-px depth band. NOTE: the
// directive suggested ~2.5 units, which under a PERSPECTIVE camera would read —
// this renderer is ORTHOGRAPHIC (z shows ONLY via rotation parallax + depth
// cues), so a visible cloud thickness needs world-px scale. Deviation noted.
const Z_JITTER = 90; // per-node scatter inside its shell (never a coplanar sheet)
const NODE_DEPTH_SIZE = 0.15; // sprite size swells/shrinks ±15% across near/far
const NODE_DEPTH_DIM = 0.35; // far-side orbs dim to 65% (near side stays full)

// FLOWING CURRENT — the base lines' own electricity. NOT a discrete travelling
// dot (that read as a bead on the wire — operator "has a dot, want smooth"):
// a SMOOTH brightness wave scrolls ALONG each wire, so the whole line shimmers
// like current in a cable with no localized blob. Lives ON THE GPU: each wire
// is ONE fat-line instance (see buildArcSystem — per-edge segmentation put a
// round-cap overlap bead at every interior joint, the operator's "row of
// dots"; blending cannot hide double coverage, so the joints are gone), which
// leaves no per-station vertex colors to modulate — instead installFlowPatch
// rewrites the LineMaterial shaders to evaluate the same wave per FRAGMENT:
// C∞ along the wire, resolution-independent. Per-instance attributes carry
// each wire's phase stagger + hover dim; uTime is the only per-frame CPU
// write. The halo mesh SHARES the wire geometry + patch so its glow rides the
// same wave. The crest may exceed 1; FLOW_MAX bounds it (output clamps at 1
// anyway — the min just bounds the plateau). Scroll direction A→B =
// source→dest current direction.
const FLOW_SPEED = 0.35; // wave scrolls this many cycles/sec along the wire
const FLOW_CYCLES = 1; // bright bands along a wire at once (1 = one long smooth band, no dot)
const FLOW_GAIN = 1.1; // wave hue-brightness boost at the crest
const FLOW_WHITE = 0.35; // crest push toward white (kept LOW — high white = a hot dot)
const FLOW_BASE_LEVEL = 0.5; // trough brightness (the wire is always a smooth visible line)
const FLOW_MAX = 3; // color clamp (headroom for the blaze)

// ---- the GPU flow patch (base-wire + halo LineMaterials) --------------------
// Anchor strings must match the vendored LineMaterial sources VERBATIM
// (/vendor/three-lightning.mjs, ShaderLib.line) — installFlowPatch validates
// them against material.vertexShader/.fragmentShader at BUILD time and throws
// (→ failBuild → SVG fallback) rather than ever rendering unpatched, beaded
// wires. All four anchors sit OUTSIDE the #ifdef WORLD_UNITS / USE_DASH
// branches we compile out, and each appears exactly once in its stage.
const FLOW_VERT_ANCHOR_DECL = "attribute vec3 instanceColorEnd;";
const FLOW_VERT_ANCHOR_MAIN = "float aspect = resolution.x / resolution.y;";
const FLOW_FRAG_ANCHOR_DECL = "uniform float opacity;";
const FLOW_FRAG_ANCHOR_OUT = "gl_FragColor = vec4( diffuseColor.rgb, alpha );";
// position.y < 0.5 is the fat-line quad's own start-vs-end vertex test (the
// vendored vertex shader picks instanceColorStart vs End with it) — vT rides
// the same split: 0.0 at the start vertices, 1.0 at the end, interpolating
// linearly across the quad = parametric t along the wire.
const FLOW_VERT_DECL_GLSL = `
		attribute float instFlowPhase;
		attribute float instDim;
		varying float vT;
		varying float vFlowPhase;
		varying float vDim;
`;
const FLOW_VERT_MAIN_GLSL = `
			vT = ( position.y < 0.5 ) ? 0.0 : 1.0;
			vFlowPhase = instFlowPhase;
			vDim = instDim;
`;
const FLOW_FRAG_DECL_GLSL = `
		uniform float uTime;
		uniform float uFlowOn;
		varying float vT;
		varying float vFlowPhase;
		varying float vDim;
`;
// The exact math the deleted CPU writeLinkColors used, per fragment: wave 0..1
// scrolling along t; level/white lift the hue; ×dim; clamp. uFlowOn 0 (reduced
// motion) collapses to a static full-hue wire — hover dim still applies.
// FLOW_* bake as literals (they are constants, not runtime knobs); uTime and
// uFlowOn stay uniforms. diffuseColor.rgb already carries the per-instance hue
// at the anchor (it sits after #include <color_fragment>).
const FLOW_FRAG_OUT_GLSL = `
			float flowWave = 0.5 + 0.5 * sin( ( vT * ${FLOW_CYCLES.toFixed(1)} - uTime * ${FLOW_SPEED.toFixed(4)} - vFlowPhase ) * 6.2831853 );
			float flowLevel = mix( 1.0, ${FLOW_BASE_LEVEL.toFixed(4)} + ${FLOW_GAIN.toFixed(4)} * flowWave, uFlowOn );
			float flowWhite = ${FLOW_WHITE.toFixed(4)} * flowWave * uFlowOn;
			gl_FragColor = vec4( min( ( diffuseColor.rgb * flowLevel + vec3( flowWhite ) ) * vDim, vec3( ${FLOW_MAX.toFixed(1)} ) ), alpha );
`;
// Validate the anchors NOW (a LineMaterial IS a ShaderMaterial —
// .vertexShader/.fragmentShader are the exact strings onBeforeCompile
// receives) and install the compile-time patch. `uniforms` = shared
// { uTime, uFlowOn } uniform OBJECTS — one .value write drives every patched
// material. Both patched materials get the same-source callback, so three's
// program cache (keyed on onBeforeCompile.toString()) can share the program
// while keeping the unpatched overlay materials on their own programs.
function installFlowPatch(material, uniforms, label) {
  const checks = [
    [material.vertexShader, FLOW_VERT_ANCHOR_DECL],
    [material.vertexShader, FLOW_VERT_ANCHOR_MAIN],
    [material.fragmentShader, FLOW_FRAG_ANCHOR_DECL],
    [material.fragmentShader, FLOW_FRAG_ANCHOR_OUT],
  ];
  for (const [src, anchor] of checks) {
    if (typeof src !== "string" || !src.includes(anchor)) {
      const msg = `memory-lightning: flow shader anchor ${JSON.stringify(anchor)} not found (${label}) — vendored LineMaterial changed; refusing to render unpatched wires`;
      console.error(msg);
      throw new Error(msg);
    }
  }
  material.onBeforeCompile = (shader) => {
    shader.uniforms.uTime = uniforms.uTime;
    shader.uniforms.uFlowOn = uniforms.uFlowOn;
    shader.vertexShader = shader.vertexShader
      .replace(FLOW_VERT_ANCHOR_DECL, FLOW_VERT_ANCHOR_DECL + FLOW_VERT_DECL_GLSL)
      .replace(FLOW_VERT_ANCHOR_MAIN, FLOW_VERT_MAIN_GLSL + FLOW_VERT_ANCHOR_MAIN);
    shader.fragmentShader = shader.fragmentShader
      .replace(FLOW_FRAG_ANCHOR_DECL, FLOW_FRAG_ANCHOR_DECL + FLOW_FRAG_DECL_GLSL)
      .replace(FLOW_FRAG_ANCHOR_OUT, FLOW_FRAG_OUT_GLSL);
  };
}

// ---- the strike-family bead-kill: premultiplied MAX blending ----------------
// The jagged strike core/branch/glow polylines NEED their per-segment round
// caps (they fill the elbow gaps where segments bend), so unlike the base
// wires their joints cannot be deleted — instead the overlap is defanged:
// under blendEquation MAX the framebuffer takes max(src, dst), so the
// cap-over-body lens at every joint renders exactly as bright as the body —
// beads structurally gone (and core-over-glow overlap stops double-
// brightening). The unused-slot contract survives unchanged: zeroed colors
// never WIN a max, exactly as they never ADDED under additive.
//
// CRITICAL: MIN/MAX blend equations IGNORE blend factors, so the srcAlpha
// scaling additive relied on vanishes — the shader must PREMULTIPLY instead.
// At the anchor, `alpha` already carries material.opacity (the fragment's
// first line is `float alpha = opacity;`), so the glow's low opacity rides
// through the premultiply, and the envelope fades live in the vertex COLORS
// untouched. Alpha stays in the output only for the alpha-to-coverage branch
// (it shapes the round caps there; our materials compile the discard branch,
// where output alpha is inert) — the canvas is opaque, stored alpha never
// composites.
const MAX_PREMULT_OUT_GLSL = `
			gl_FragColor = vec4( diffuseColor.rgb * alpha, alpha );
`;
// Same fail-loud contract as installFlowPatch, same verbatim final-output
// anchor. Installs the premultiply patch AND flips the material to
// CustomBlending/MaxEquation — always together, one call (a premultiplied
// output under plain SRC_ALPHA additive would double-apply opacity; the
// caller gates the whole call on one capability check, all-or-nothing).
// This callback's source differs from installFlowPatch's and from the
// default Material.onBeforeCompile, so three's program cache (keyed on
// onBeforeCompile.toString()) keeps max-premult, flow-patched, and
// unpatched programs strictly apart.
function installMaxPremultPatch(material, label) {
  const src = material.fragmentShader;
  if (typeof src !== "string" || !src.includes(FLOW_FRAG_ANCHOR_OUT)) {
    const msg = `memory-lightning: strike shader anchor ${JSON.stringify(FLOW_FRAG_ANCHOR_OUT)} not found (${label}) — vendored LineMaterial changed; refusing to render mispatched bolts`;
    console.error(msg);
    throw new Error(msg);
  }
  material.onBeforeCompile = (shader) => {
    shader.fragmentShader = shader.fragmentShader.replace(FLOW_FRAG_ANCHOR_OUT, MAX_PREMULT_OUT_GLSL);
  };
  material.blending = THREE.CustomBlending;
  material.blendEquation = THREE.MaxEquation;
  material.blendSrc = THREE.OneFactor; // factors are IGNORED by MAX — set sane constants anyway
  material.blendDst = THREE.OneFactor;
}

// ---- glow shading: smooth TRANSVERSE falloff (inner glow + outer aura) ------
// A stock LineMaterial ribbon is CONSTANT brightness from centerline to edge —
// the glow layers read as flat bands, not light (operator: "the shading make
// it smooth"). This flavor of the premult-MAX patch multiplies a smooth
// transverse profile into the output: brightness peaks at the centerline and
// melts to EXACTLY zero at the edge — real light falloff.
//
// Transverse distance comes from the fat-line quad's own parameterization
// (VERIFIED against the vendored LineSegmentsGeometry uv array [-1,2, 1,2,
// -1,1, 1,1, -1,-1, 1,-1, -1,-2, 1,-2]: uv.x = ±1 ACROSS the width, 0 at the
// centerline; uv.y runs ALONG the segment with |uv.y| > 1 in the round-cap
// regions): body d = |vUv.x|; caps d = the exact radial measure the cap
// discard uses — the profile wraps the round caps seamlessly (no bright cap
// discs) and identical profiles overlap identically at joints, so under MAX
// the bolt stays bead-free. Profile: squared smoothstep — s = f²(3−2f),
// falloff = s² — a Gaussian-ish wing: flat-ish bright center, soft tail,
// zero-slope landing at the edge. The min() clamp guards MSAA centroid
// samples that evaluate varyings a hair outside the quad. Material opacity
// therefore becomes the CENTERLINE PEAK (edges no longer carry it) — the
// STRIKE_GLOW_/STRIKE_AURA_ opacity constants compensate for the lost ribbon
// energy.
const MAX_PREMULT_FALLOFF_OUT_GLSL = `
			float fallT = abs( vUv.x );
			if ( abs( vUv.y ) > 1.0 ) {
				float fallA = vUv.x;
				float fallB = ( vUv.y > 0.0 ) ? vUv.y - 1.0 : vUv.y + 1.0;
				fallT = sqrt( fallA * fallA + fallB * fallB );
			}
			float fallW = 1.0 - min( fallT, 1.0 );
			float fallS = fallW * fallW * ( 3.0 - 2.0 * fallW );
			float falloff = fallS * fallS;
			gl_FragColor = vec4( diffuseColor.rgb * alpha * falloff, alpha * falloff );
`;
// Semantic guards: the falloff replicates the cap block's radial math, so the
// exact cap parameterization line and the vUv varying must still exist in the
// vendored fragment — validated fail-loud like every anchor (the cap line
// appears in BOTH the A2C and discard branches; presence is the contract).
const FALLOFF_GUARD_VUV = "varying vec2 vUv;";
const FALLOFF_GUARD_CAP = "float b = ( vUv.y > 0.0 ) ? vUv.y - 1.0 : vUv.y + 1.0;";
// A SEPARATE function from installMaxPremultPatch ON PURPOSE: three keys its
// program cache on onBeforeCompile.toString(), so the falloff flavor needs a
// DISTINCT callback source — one shared closure branching on a captured flag
// would stringify identically and the first-compiled program would be reused
// for the wrong flavor (core would inherit the falloff, or the glow would
// lose it). Same blend state; same all-or-nothing maxBlendOK gating by the
// caller. Applies to the inner glow + outer aura ONLY — the 3px core is the
// bolt itself (a hot filament SHOULD have a defined edge) and the 1.6px
// branches have no width to spend on a profile.
function installMaxPremultFalloffPatch(material, label) {
  const src = material.fragmentShader;
  for (const anchor of [FLOW_FRAG_ANCHOR_OUT, FALLOFF_GUARD_VUV, FALLOFF_GUARD_CAP]) {
    if (typeof src !== "string" || !src.includes(anchor)) {
      const msg = `memory-lightning: falloff shader anchor ${JSON.stringify(anchor)} not found (${label}) — vendored LineMaterial changed; refusing to render mispatched glow`;
      console.error(msg);
      throw new Error(msg);
    }
  }
  material.onBeforeCompile = (shader) => {
    shader.fragmentShader = shader.fragmentShader.replace(FLOW_FRAG_ANCHOR_OUT, MAX_PREMULT_FALLOFF_OUT_GLSL);
  };
  material.blending = THREE.CustomBlending;
  material.blendEquation = THREE.MaxEquation;
  material.blendSrc = THREE.OneFactor; // factors are IGNORED by MAX — set sane constants anyway
  material.blendDst = THREE.OneFactor;
}

// Halo pass ≈ the reference's shadowBlur 10 line glow: a wider, faint copy
// UNDER the crisp bolts, sharing their position/color buffers.
const HALO_WIDTH_PX = 5;
const HALO_OPACITY = 0.05;

// Nodes — sizes keep the reference law (radius = (selected ? 28 : 20) +
// pulse*4, pulse = sin(time*2+phase)*.3+.7; white 0.8 ring at r+3 when
// selected), but the RENDERING is ours: a luminous plasma/glass sphere, not
// the reference's flat gradient disc (operator: "luminous glass or plasma
// spheres with soft bloom, subtle inner gradients, and elegant color
// separation"). Layer recipe lives in NODE_FRAG; every knob is a constant here.
const NODE_R_BASE = 20;
const NODE_R_SEL_BONUS = 8; // selected: 20 → 28
const NODE_PULSE_PX = 4;
// 2.5× sprite (was the reference's 3×): shorter halo reach + weaker glow —
// dense stores were washing into a fog of overlapping halos (operator: "more
// polish, clearer, not too glowing").
const NODE_SPRITE_SCALE = 2.5;
// The pulse's ±1.2px size wobble is SUB-pixel once the map is zoomed far out —
// it reads as shimmer/aliasing, not life. Its amplitude ramps 0 → full across
// this zoom window (dead below LO, reference-exact at/above HI).
const NODE_PULSE_ZOOM_LO = 0.3;
const NODE_PULSE_ZOOM_HI = 0.75;
// PLASMA BODY — inside → out:
//   hot core   — small near-white nucleus, offset toward the upper-left like a
//                specular highlight on glass (sells the sphere read)
//   inner lift — nonlinear (quadratic) brightening of the hue toward center
//   rim        — darker-but-saturated hue from NODE_RIM_START out; still glows
//   fringe     — per-channel rim radii split the last ~2% into hue-adjacent
//                tones (violet-out chromatic lens fringe — subtle, matches the
//                electric palette)
const NODE_CORE_OFF = 0.1; // core center offset per axis, in body radii (up-left ≈ 14% diagonal)
const NODE_CORE_REACH = 0.55; // core falloff reach, in body radii (squared → tight nucleus)
const NODE_CORE_WHITE = 0.85; // how near-white the nucleus gets (1 = pure white)
const NODE_INNER_LIFT = 0.45; // hue brightening at dead center (quadratic falloff)
const NODE_RIM_START = 0.55; // rim darkening begins here (body-radius units)
const NODE_RIM_DARK = 0.6; // rim hue multiplier — darker, never dead
const NODE_FRINGE = 0.012; // per-channel rr offset (±1.2%) for the chromatic rim
// SOFT BLOOM — ONE smooth tail from the body edge to EXACTLY the sprite edge
// (rr=1): a wide whisper-faint term plus a tail³ collar hugging the body. Both
// smoothstep-shaped (zero slope at both ends) → no band seams (the old
// two-step halo seamed visibly at bodyR·1.6) and no sprite-square cutoff.
const NODE_BLOOM_WIDE_A = 0.035; // far-glow peak alpha
const NODE_BLOOM_COLLAR_A = 0.085; // close-in collar peak alpha (idle)
const NODE_BLOOM_SEL_A = 0.1; // collar bonus while selected
const NODE_BLOOM_HOT_A = 0.07; // collar bonus while hovered
// HOVER — the hovered orb answers: slightly larger, hotter core, bigger bloom
// (aHot attribute, written on hover CHANGE only — never per frame).
const NODE_HOT_R_BONUS_PX = 2;
const NODE_HOT_CORE_BOOST = 0.15; // core whiteness 0.85 → 1.0 hovered
// DIMMED (hover isolation): alpha tracks vDim fully, but color keeps this
// fraction of full brightness — faded orbs stay hue-tinted, never muddy gray.
const NODE_DIM_HUE_KEEP = 0.35;

// ATMOSPHERE — one static radial-gradient quad under everything (built once,
// zero per-frame cost, NOT a fullscreen pass): an extremely subtle indigo pool
// centered on the view so the near-black field reads as depth, not flatness.
// Alpha peaks at ATMO_ALPHA dead center and smoothsteps to EXACTLY 0 at the
// quad rim (radius = max(viewW, viewH) · ATMO_RADIUS_VIEW).
const ATMO_COLOR = 0x10142a; // deep indigo, a breath above CLEAR_COLOR
const ATMO_ALPHA = 0.2;
const ATMO_RADIUS_VIEW = 0.85;

// Particles: OFF (operator request — no dots riding the lines; the thunder
// strikes alone carry the flow). Pool machinery kept, spawn disabled.
const PARTICLE_SPAWN_MS = 320;
const PARTICLE_MAX = 0;
const PARTICLE_SPEED_MIN = 0.005; // progress per 60Hz frame (reference law)
const PARTICLE_SPEED_VAR = 0.01;
const PARTICLE_R_MIN = 1.5;
const PARTICLE_R_VAR = 2;

// Background grid: 60 world-px cells, rgba(255,255,255,0.015).
const GRID_STEP = 60;
const GRID_OPACITY = 0.015;
const GRID_EXTENT = 6000; // static world coverage (pan/zoom stay inside at 0.2×)

// View: wheel-zoom clamp [0.2, 3] (reference).
const ZOOM_MIN = 0.2;
const ZOOM_MAX = 3;

// Hover isolation (OURS — the reference has no hover effects; kept because the
// operator approved it): non-adjacent edges/nodes dim while a node is hovered.
const HOVER_HIT_PAD_PX = 8;
const HOVER_EPS_PX = 0.5;
const HOVER_EDGE_DIM = 0.12;
const HOVER_NODE_ADJ = 0.85;
const HOVER_NODE_REST = 0.2;

// ---- node sprite shaders (plasma sphere in one point sprite) -----------------
// uTime is SECONDS (the reference's frame-locked "time" ≈ seconds at 60Hz).
// The sprite spans the 2.5r halo; rr = 0 at center → 1 at sprite edge; the body
// occupies rr ≤ 1/2.5. vRing carries the r+3 ring position in rr units (radius-
// dependent). aSel sizes 20→28 and raises the bloom; aHot marks the hovered orb
// (hotter core, bigger bloom, +2px); vDim = hover isolation. GLSL stays in the
// WebGL1 dialect (varying/gl_FragColor, no fwidth) like every shader here.
const NODE_VERT = `
  attribute float aPhase;
  attribute float aDim;
  attribute vec3 aColor;
  attribute float aSel;
  attribute float aHot;
  attribute float aDepth;
  uniform float uTime;
  uniform float uSize;
  uniform float uZoom;
  varying float vDim;
  varying vec3 vColor;
  varying float vSel;
  varying float vRing;
  varying float vHot;
  varying float vDepth;
  void main() {
    float pulse = sin(uTime * 2.0 + aPhase) * 0.3 + 0.7;
    // Zoomed far out the pulse is sub-pixel shimmer — damp its amplitude.
    float pulseAmp = ${NODE_PULSE_PX.toFixed(1)} * smoothstep(${NODE_PULSE_ZOOM_LO.toFixed(2)}, ${NODE_PULSE_ZOOM_HI.toFixed(2)}, uZoom);
    float radius = ${NODE_R_BASE.toFixed(1)} + aSel * ${NODE_R_SEL_BONUS.toFixed(1)} + aHot * ${NODE_HOT_R_BONUS_PX.toFixed(1)} + pulse * pulseAmp;
    vDim = aDim;
    vColor = aColor;
    vSel = aSel;
    vHot = aHot;
    vDepth = aDepth;
    vRing = (radius + 3.0) / (radius * ${NODE_SPRITE_SCALE.toFixed(1)}); // r+3 ring in rr units
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    // Planetary depth cue: near orbs swell slightly, far orbs shrink (aDepth
    // is the CPU-normalized yaw depth, −1 far … +1 near — subtle, not a zoom).
    gl_PointSize = radius * 2.0 * ${NODE_SPRITE_SCALE.toFixed(1)} * uSize * uZoom * (1.0 + aDepth * ${NODE_DEPTH_SIZE.toFixed(2)});
    gl_Position = projectionMatrix * mv;
  }
`;
const NODE_FRAG = `
  varying float vDim;
  varying vec3 vColor;
  varying float vSel;
  varying float vRing;
  varying float vHot;
  varying float vDepth;
  void main() {
    vec2 pc = gl_PointCoord;
    float rr = length(pc - vec2(0.5)) * 2.0;    // 0 center → 1 sprite edge (= 2.5r)
    float bodyR = 1.0 / ${NODE_SPRITE_SCALE.toFixed(1)};
    // Body coverage split per RGB channel: red sees the rim marginally early,
    // blue marginally late (covB ≥ covG ≥ covR by construction), so the last
    // ~2% of the rim separates into hue-adjacent tones — a violet-out lens
    // fringe (subtle; matches the electric palette), never an RGB glitch.
    float covR = 1.0 - smoothstep(bodyR - 0.02, bodyR, rr * (1.0 + ${NODE_FRINGE.toFixed(3)}));
    float covG = 1.0 - smoothstep(bodyR - 0.02, bodyR, rr);
    float covB = 1.0 - smoothstep(bodyR - 0.02, bodyR, rr * (1.0 - ${NODE_FRINGE.toFixed(3)}));
    float inside = covB;                        // alpha reaches wherever any channel does
    vec3 chroma = vec3(covR, covG, covB) / max(covB, 1.0e-4); // 1,1,1 deep inside the body
    // Plasma body: nonlinear inner gradient lifting the hue toward center, a
    // darker-but-still-glowing rim, and a small near-white nucleus offset
    // toward the upper-left — the fake spherical specular that sells "glass".
    float q = clamp(rr / bodyR, 0.0, 1.0);      // 0 center → 1 body rim
    float dc = length(pc - vec2(0.5 - ${NODE_CORE_OFF.toFixed(2)} * bodyR * 0.5)) * 2.0 / bodyR;
    float core = 1.0 - smoothstep(0.0, ${NODE_CORE_REACH.toFixed(2)}, dc);
    core *= core;                               // tight hot nucleus, smooth decay
    float rim = smoothstep(${NODE_RIM_START.toFixed(2)}, 1.0, q);
    float lift = (1.0 - q) * (1.0 - q);
    vec3 bodyCol = vColor * (1.0 + ${NODE_INNER_LIFT.toFixed(2)} * lift);
    bodyCol = mix(bodyCol, vColor * ${NODE_RIM_DARK.toFixed(2)}, rim);
    bodyCol = mix(bodyCol, vec3(1.0), core * (${NODE_CORE_WHITE.toFixed(2)} + vHot * ${NODE_HOT_CORE_BOOST.toFixed(2)}));
    bodyCol = min(bodyCol * chroma, vec3(1.0));
    float bodyA = mix(0.95, 0.84, q);           // glassy: a touch more translucent at the rim
    // Soft bloom: ONE smooth tail, body edge → EXACTLY the sprite edge (rr=1).
    // smoothstep is zero-slope at both ends → no band seams, no square cutoff.
    // collar (tail³) hugs the body; the wide term is the whisper-faint far glow.
    float tail = 1.0 - smoothstep(bodyR, 1.0, rr);
    float collar = tail * tail * tail;
    float bloomA = ${NODE_BLOOM_WIDE_A.toFixed(3)} * tail
      + (${NODE_BLOOM_COLLAR_A.toFixed(3)} + vSel * ${NODE_BLOOM_SEL_A.toFixed(3)} + vHot * ${NODE_BLOOM_HOT_A.toFixed(3)}) * collar;
    bloomA *= (1.0 - inside);
    // Selected/connecting ring: white 0.8, ~2px band at r+3 (unchanged law).
    float ring = vSel * (1.0 - smoothstep(0.012, 0.03, abs(rr - vRing))) * 0.8;
    vec3 col = mix(vColor, bodyCol, inside);    // the bloom carries the raw hue
    col = mix(col, vec3(1.0), ring);
    float alpha = max(inside * bodyA, bloomA);
    alpha = max(alpha, ring);
    // Hover-dim: alpha tracks vDim fully; color keeps a hint of hue so dimmed
    // orbs fade gracefully instead of muddying to gray on the near-black field.
    float colDim = mix(vDim, 1.0, ${NODE_DIM_HUE_KEEP.toFixed(2)});
    // Planetary depth cue: FAR-side orbs (vDepth < 0) dim gently — the near
    // side stays exactly as approved. Subtle: planetary, not a carousel.
    float depthDim = 1.0 - max(0.0, -vDepth) * ${NODE_DEPTH_DIM.toFixed(2)};
    gl_FragColor = vec4(col * colDim * depthDim, alpha * vDim * depthDim);
  }
`;

// ---- particle sprite shaders (soft glowing dot, alpha-over) -------------------
const PARTICLE_VERT = `
  attribute float aPSize;
  attribute vec3 aPColor;
  attribute float aPAlpha;
  uniform float uSize;
  uniform float uZoom;
  varying vec3 vPColor;
  varying float vPAlpha;
  void main() {
    vPColor = aPColor;
    vPAlpha = aPAlpha;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_PointSize = aPSize * 2.0 * 1.8 * uSize * uZoom; // ×1.8: soft blur-8 skirt
    gl_Position = projectionMatrix * mv;
  }
`;
const PARTICLE_FRAG = `
  varying vec3 vPColor;
  varying float vPAlpha;
  void main() {
    float r = length(gl_PointCoord - vec2(0.5)) * 2.0;
    float a = (1.0 - smoothstep(0.35, 1.0, r)) * vPAlpha;
    gl_FragColor = vec4(vPColor, a);
  }
`;

// ---- atmosphere shaders (one static quad — see the ATMO_* constants) ---------
// Color is baked at module load (constants never change at runtime); alpha is a
// t² smoothstep pool hitting exactly 0 at the quad rim, so the quad edge can
// never print. `uv` comes from three's built-in ShaderMaterial attributes.
const ATMO_RGB = hexToRgb01(ATMO_COLOR).map((v) => v.toFixed(4));
const ATMO_VERT = `
  varying vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;
const ATMO_FRAG = `
  varying vec2 vUv;
  void main() {
    float d = length(vUv - vec2(0.5)) * 2.0;    // 0 center → 1 quad edge
    float t = 1.0 - smoothstep(0.0, 1.0, d);
    gl_FragColor = vec4(${ATMO_RGB[0]}, ${ATMO_RGB[1]}, ${ATMO_RGB[2]}, ${ATMO_ALPHA.toFixed(2)} * t * t);
  }
`;

// Cheap deterministic 0..1 hash — placement jitter staggers only (bolt jitter
// is Math.random per regen, like the reference).
function hash01(x) {
  const s = Math.sin(x * 127.1 + 311.7) * 43758.5453123;
  return s - Math.floor(s);
}

// Build one batched connection system: ONE preallocated Float32Array of segment
// pairs (`segments` per edge), ONE LineSegmentsGeometry, ONE LineMaterial.
// setPositions/setColors run exactly ONCE (the vendored geometry WRAPS a
// Float32Array — no copy — in an InstancedInterleavedBuffer shared by its
// start/end attributes), so per-edge subarray slots alias GPU-source memory:
// step() rewrites them in place and flips ONE needsUpdate per frame.
//
// `segments` is the BEAD dial: every fat-line instance draws round caps at
// both ends, so consecutive collinear instances overlap caps at each shared
// joint — double coverage that NO blending mode hides (additive doubles it;
// alpha-over still composites 1-(1-a)² > a). BOTH systems therefore pass 1 —
// one A→B instance per edge, zero interior joints, zero beads (the wire-end
// caps land under the opaque node orbs).
//
// `zArr` (optional Float32Array by node index): the planetary depth — each
// endpoint takes its node's stable z-offset (lerped along multi-segment
// chords) so wires stay ATTACHED to their orbs while the pivot group yaws.
function buildArcSystem(aIdx, bIdx, nodesRef, widthPx, opacity, viewW, viewH, track, edgeColors, blending, segments, zArr) {
  const count = aIdx.length;
  const floatsPerEdge = segments * 6;
  const positions = new Float32Array(count * floatsPerEdge);
  const colors = new Float32Array(count * floatsPerEdge).fill(1);
  const baseColors = new Float32Array(count * floatsPerEdge).fill(1);
  const slots = [];
  const colorSlots = [];
  const baseColorSlots = [];

  for (let i = 0; i < count; i++) {
    slots.push(positions.subarray(i * floatsPerEdge, (i + 1) * floatsPerEdge));
    const cs = colors.subarray(i * floatsPerEdge, (i + 1) * floatsPerEdge);
    colorSlots.push(cs);
    baseColorSlots.push(baseColors.subarray(i * floatsPerEdge, (i + 1) * floatsPerEdge));
    // Bake the FROM-node color into this edge's segments. Live + base both
    // start here; hover dimming scales base → live.
    if (edgeColors && edgeColors[i] != null) {
      const [r, g, b] = hexToRgb01(edgeColors[i]);
      for (let j = 0; j < floatsPerEdge; j += 3) {
        cs[j] = r; cs[j + 1] = g; cs[j + 2] = b;
      }
      baseColorSlots[i].set(cs);
    }
  }


  const geometry = new THREE.LineSegmentsGeometry();
  track.geometries.push(geometry);
  if (count > 0) {
    geometry.setPositions(positions);
    geometry.setColors(colors);
  }
  const instanceBuffer = count > 0 ? geometry.attributes.instanceStart.data : null;
  const colorBuffer = count > 0 ? geometry.attributes.instanceColorStart.data : null;
  const material = new THREE.LineMaterial({
    color: 0xffffff, // vertex colors carry the per-edge from-node hue
    linewidth: widthPx,
    transparent: true,
    opacity,
    vertexColors: true,
    // Per-system, caller-chosen:
    //  · linkSys → NORMAL (alpha-over), the reference's actual mode. It draws
    //    only REAL edges (no unused black slots) and its hover-dim is the
    //    instDim attribute the flow patch reads — never black-out colors — so
    //    alpha-over is safe (and with segments=1 there are no joints to bead
    //    under ANY blending).
    //  · suggSys → ADDITIVE: it dims by scaling vertex colors toward black
    //    (fillEdgeColors — hover isolation on the LIVE suggested overlay), and
    //    black is invisible ONLY under additive — under alpha-over it paints
    //    dark ropes (the phantom-rope bug).
    blending,
    depthWrite: false,
    depthTest: false,
  });
  track.materials.push(material);
  material.worldUnits = false; // linewidth is in PIXELS
  material.side = THREE.DoubleSide; // y-down ortho flips winding; never let culling eat the quads
  material.resolution.set(viewW, viewH); // fat lines render wrong without this

  const mesh = new THREE.LineSegments2(geometry, material);
  mesh.frustumCulled = false; // ortho camera always covers the layout box
  mesh.visible = count > 0;

  // Per-frame rebuild from LIVE endpoints: `segments` collinear pairs along
  // the straight chord (segments=1 → exactly the A→B chord), z lerped between
  // the endpoints' planetary depths. Zero per-frame allocation.
  const step = () => {
    if (count === 0) return;
    for (let i = 0; i < count; i++) {
      const A = nodesRef[aIdx[i]];
      const B = nodesRef[bIdx[i]];
      const live = slots[i];
      if (!A || !B) { live.fill(0); continue; }
      const zA = zArr ? zArr[aIdx[i]] : 0;
      const zB = zArr ? zArr[bIdx[i]] : 0;
      const dx = B.x - A.x;
      const dy = B.y - A.y;
      let w = 0;
      let px = A.x;
      let py = A.y;
      let pz = zA;
      for (let j = 1; j <= segments; j++) {
        const tj = j / segments;
        const sx = A.x + dx * tj;
        const sy = A.y + dy * tj;
        const sz = zA + (zB - zA) * tj;
        live[w++] = px; live[w++] = py; live[w++] = pz;
        live[w++] = sx; live[w++] = sy; live[w++] = sz;
        px = sx; py = sy; pz = sz;
      }
    }
    instanceBuffer.needsUpdate = true;
  };
  step();

  return { count, positions, slots, colors, colorSlots, baseColors, baseColorSlots, colorBuffer, geometry, material, mesh, step, baseOpacity: opacity };
}

// Create the graph view inside `container` (#graph-body — position:relative,
// overflow:hidden). `layout` seeds the force sim; width/height are the
// container's CSS-pixel size (world coords == layout pixel coords, y-down).
// Returns { dispose, setSelected, getPositions } or null (WebGL unavailable /
// build threw) — the caller falls back to SVG.
export function createLightningGraph({ container, layout, width, height, onHover, onSelect, onCreateAt, seedPositions }) {
  if (!container || !layout) return null;
  const nodes = Array.isArray(layout.nodes) ? layout.nodes : [];
  const allEdges = Array.isArray(layout.edges) ? layout.edges : [];
  const viewW = Math.max(1, width || 1);
  const viewH = Math.max(1, height || 1);

  // Renderer FIRST — if the context can't be created we return null having
  // touched nothing in the DOM, and the caller keeps its SVG fallback.
  let renderer = null;
  try {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: false, powerPreference: "low-power" });
  } catch (_) {
    return null;
  }
  let gl = null;
  try { gl = renderer.getContext(); } catch (_) {}
  if (!gl) {
    try { renderer.dispose(); } catch (_) {}
    return null;
  }

  // Anything below can still throw with a LIVE context held (shader compile at
  // the reduced-motion first render, a malformed layout node, …). WKWebView
  // caps live WebGL contexts (~8, shared with the terminal panes), so a
  // mid-build throw must release everything created so far — geometries,
  // materials, the renderer+context, and any DOM we appended — then return
  // null so the caller falls back to SVG.
  const track = { geometries: [], materials: [] };
  let canvas = null;
  let labelLayer = null;
  let onVisibility = null;
  const failBuild = () => {
    try { renderer.setAnimationLoop(null); } catch (_) {}
    if (onVisibility) { try { document.removeEventListener("visibilitychange", onVisibility); } catch (_) {} }
    for (const g of track.geometries) { try { g.dispose(); } catch (_) {} }
    for (const m of track.materials) { try { m.dispose(); } catch (_) {} }
    try { renderer.dispose(); } catch (_) {}
    try { renderer.forceContextLoss(); } catch (_) {}
    if (canvas) { try { canvas.remove(); } catch (_) {} }
    if (labelLayer) { try { labelLayer.remove(); } catch (_) {} }
  };

  try {
    const pixelRatio = Math.min(window.devicePixelRatio || 1, PIXEL_RATIO_CAP);
    renderer.setPixelRatio(pixelRatio);
    renderer.setSize(viewW, viewH);
    renderer.setClearColor(CLEAR_COLOR, 1);

    // MAX blend equation availability: core in WebGL2, EXT_blend_minmax on
    // WebGL1. Checked ONCE; gates the strike-family bead-kill ALL-OR-NOTHING
    // per material (blend switch + premultiply patch travel together inside
    // installMaxPremultPatch) — without it the bolts keep plain additive:
    // joint glints return, nothing breaks.
    const maxBlendOK =
      !!(renderer.capabilities && renderer.capabilities.isWebGL2) ||
      !!gl.getExtension("EXT_blend_minmax");

    // Box styling comes from styles.css (.graph-body canvas / .lightning-labels);
    // only data-driven per-node label offsets are inline.
    canvas = renderer.domElement;

    // Ortho camera mapped so world coords == layout pixel coords, y-down: top=0,
    // bottom=height keeps y growing downward exactly like the SVG view.
    const camera = new THREE.OrthographicCamera(0, viewW, 0, viewH, 0.1, CAMERA_FAR);
    camera.position.z = CAMERA_Z;

    const scene = new THREE.Scene();

    // Background grid — 60px world cells, whisper-faint, static geometry over a
    // generous extent so pan/zoom never runs off its edge.
    const gridPts = [];
    for (let x = -GRID_EXTENT; x <= GRID_EXTENT; x += GRID_STEP) {
      gridPts.push(x, -GRID_EXTENT, 0, x, GRID_EXTENT, 0);
    }
    for (let y = -GRID_EXTENT; y <= GRID_EXTENT; y += GRID_STEP) {
      gridPts.push(-GRID_EXTENT, y, 0, GRID_EXTENT, y, 0);
    }
    const gridGeometry = new THREE.BufferGeometry();
    track.geometries.push(gridGeometry);
    gridGeometry.setAttribute("position", new THREE.BufferAttribute(new Float32Array(gridPts), 3));
    const gridMaterial = new THREE.LineBasicMaterial({
      color: 0xffffff,
      transparent: true,
      opacity: GRID_OPACITY,
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(gridMaterial);
    const gridMesh = new THREE.LineSegments(gridGeometry, gridMaterial);
    gridMesh.frustumCulled = false;
    gridMesh.renderOrder = -1;
    scene.add(gridMesh);

    // Atmosphere — ONE static transparent quad under the grid (built once,
    // never touched per frame; a fullscreen shader pass was rejected as cost):
    // an extremely subtle indigo radial pool anchored to the layout's home
    // region, so the field reads as depth instead of flat black. Its alpha is
    // exactly 0 at the quad rim — panning past it never shows an edge.
    const atmoR = Math.max(viewW, viewH) * ATMO_RADIUS_VIEW;
    const atmoGeometry = new THREE.PlaneGeometry(atmoR * 2, atmoR * 2);
    track.geometries.push(atmoGeometry);
    const atmoMaterial = new THREE.ShaderMaterial({
      vertexShader: ATMO_VERT,
      fragmentShader: ATMO_FRAG,
      transparent: true,
      blending: THREE.NormalBlending,
      depthWrite: false,
      depthTest: false,
      side: THREE.DoubleSide, // y-down ortho flips winding; never let culling eat it
    });
    track.materials.push(atmoMaterial);
    const atmoMesh = new THREE.Mesh(atmoGeometry, atmoMaterial);
    atmoMesh.position.set(viewW / 2, viewH / 2, 0);
    atmoMesh.frustumCulled = false;
    atmoMesh.renderOrder = -2; // under the grid (-1)
    scene.add(atmoMesh);

    // ---- planetary rotation rig ----------------------------------------------
    // The WHOLE memory cloud (nodes, every line system, sel overlay, strikes,
    // particles) rides one pivot: rotPivot sits at the view center and yaws;
    // rotInner recenters the children's absolute sim coordinates onto that
    // pivot. Buffers stay sim-space — the GPU applies the turn (fat-line
    // widths are computed in screen space AFTER modelView, and the flow
    // shader's vT is parametric along the instance, so both are rotation-
    // independent). Grid + atmosphere stay OUTSIDE as fixed background.
    // rotTheta advances in tick (PAUSED while a node is dragged, so the
    // pointer→sim inverse is stable); rotCos/rotSin are the per-frame
    // projection pair syncNodesAndLabels feeds to yawX/yawDepth for labels,
    // hit-tests, anchors, and the aDepth cue.
    const rotPivot = new THREE.Group();
    rotPivot.position.set(viewW / 2, viewH / 2, 0);
    const rotInner = new THREE.Group();
    rotInner.position.set(-viewW / 2, -viewH / 2, 0);
    rotPivot.add(rotInner);
    scene.add(rotPivot);
    const rotCx = viewW / 2; // the yaw axis (world x of the pivot)
    let rotTheta = 0;
    let rotCos = 1;
    let rotSin = 0;

    // Per-node category color (hex) — orbs AND each bolt's tint (FROM node).
    const nodeColors = nodes.map((n) => categoryColor(n));

    // Resolve every edge endpoint to a node INDEX now, while node coords are
    // still pristine layoutGraph output (the force sim jitters and then moves
    // them — coordinate matching would break the moment it starts). O(E·N) once.
    const nodeIndexAtCoord = (x, y) => {
      for (let i = 0; i < nodes.length; i++) {
        if (Math.abs(nodes[i].x - x) <= HOVER_EPS_PX && Math.abs(nodes[i].y - y) <= HOVER_EPS_PX) return i;
      }
      return -1;
    };
    // Hard links vs the suggest() heuristic (operator re-enabled 2026-07-07):
    // suggested edges now DRAW — as the faint thin suggSys overlay, clearly
    // tentative next to the solid flowing links — and contribute springs +
    // hover adjacency, exactly the original design. They never carry flow and
    // are never thunder targets (strikes sample linkSys only). The `suggested`
    // flag arrives on the laid edges from graph-core's layoutGraph (kind
    // "suggested" from the memory_graph payload) — the renderer just stopped
    // hiding it.
    const linkEdges = allEdges.filter((e) => !e.suggested);
    const suggEdges = allEdges.filter((e) => e.suggested);
    const resolveIdx = (edges) => {
      const a = new Int32Array(edges.length);
      const b = new Int32Array(edges.length);
      for (let i = 0; i < edges.length; i++) {
        a[i] = nodeIndexAtCoord(edges[i].x1, edges[i].y1);
        b[i] = nodeIndexAtCoord(edges[i].x2, edges[i].y2);
      }
      return [a, b];
    };
    const [linkA, linkB] = resolveIdx(linkEdges);
    const [suggA, suggB] = resolveIdx(suggEdges);
    const edgeSourceColors = (aIdx) => Array.from(aIdx, (i) => (i >= 0 ? nodeColors[i] : null));

    // The relaxation sim: springs over ALL edges (link + suggested), seeded from
    // the previous session's positions when provided. It mutates node.x/y in
    // place; bolts, particles, labels, hit-tests, and the node buffer read live.
    const simA = new Int32Array(linkA.length + suggA.length);
    const simB = new Int32Array(linkA.length + suggA.length);
    simA.set(linkA); simA.set(suggA, linkA.length);
    simB.set(linkB); simB.set(suggB, linkB.length);
    // Category clustering: nodes sharing a category share a group anchor in the
    // sim (alphabetical key order — stable across re-renders so clusters don't
    // shuffle on save/reload). Uncategorized notes cluster together too.
    const catKeys = [...new Set(nodes.map((n) => ((n.category || "").trim().toLowerCase()) || "~none"))].sort();
    const catIndex = new Map(catKeys.map((k, i) => [k, i]));
    const groups = nodes.map((n) => catIndex.get(((n.category || "").trim().toLowerCase()) || "~none"));
    const sim = createForceSim(nodes, simA, simB, { width: viewW, height: viewH, seedPositions, groups });

    // Stable per-node planetary depth: category shells + jitter (pure math in
    // lightning-core). Written ONCE — the sim never touches z; every line
    // writer and the node buffer read it so the cloud turns as one rigid body.
    const zOff = new Float32Array(nodes.length);
    for (let i = 0; i < nodes.length; i++) {
      zOff[i] = nodeZOffset(groups[i], catKeys.length, hash01(i * 3.77 + 9.1), Z_SPREAD, Z_JITTER);
    }

    const linkSys = buildArcSystem(linkA, linkB, nodes, LINK_WIDTH_PX, BOLT_ALPHA_BASE, viewW, viewH, track, edgeSourceColors(linkA), THREE.NormalBlending, 1, zOff);
    const suggSys = buildArcSystem(suggA, suggB, nodes, SUGGESTED_WIDTH_PX, SUGGESTED_OPACITY_BASE, viewW, viewH, track, edgeSourceColors(suggA), THREE.AdditiveBlending, 1, zOff);

    // ---- GPU flowing current + hover dim on the base wires --------------------
    // Shared uniform OBJECTS wired into both patched materials: flowTime.value
    // is the flow's ENTIRE per-frame CPU cost; flowOn 0 = reduced-motion
    // statics (the shader collapses to a plain full-hue wire, dim still works).
    const flowTime = { value: 0 };
    const flowOn = { value: 1 };
    const flowUniforms = { uTime: flowTime, uFlowOn: flowOn };
    // Per-instance attributes the patched shaders read: instFlowPhase staggers
    // each wire's wave (constant — wires never pulse in lockstep; same hash
    // stream the CPU path used), instDim is the hover-isolation scalar (1 =
    // full, HOVER_EDGE_DIM = faded) written by applyHover on hover CHANGE
    // only — never per frame. They live on the ONE wire geometry, which the
    // halo mesh shares, so both passes dim and flow identically.
    let linkDimAttr = null;
    if (linkSys.count > 0) {
      const phases = new Float32Array(linkSys.count);
      for (let fi = 0; fi < linkSys.count; fi++) phases[fi] = hash01(fi * 7.13 + 2.7);
      linkSys.geometry.setAttribute("instFlowPhase", new THREE.InstancedBufferAttribute(phases, 1));
      linkDimAttr = new THREE.InstancedBufferAttribute(new Float32Array(linkSys.count).fill(1), 1);
      linkSys.geometry.setAttribute("instDim", linkDimAttr);
    }
    installFlowPatch(linkSys.material, flowUniforms, "linkSys");

    // Halo pass ≈ bolt shadowBlur: SHARES the wire geometry OBJECT outright —
    // one instance per edge, one buffer upload serving both meshes (no second
    // buffer view to flip) — under a wider, fainter material carrying the same
    // flow patch, so the glow rides the same wave and dims with the same
    // instDim. Alpha-over like the wires; with one instance per edge there are
    // no joints for ANY blending mode to bead.
    const haloMaterial = new THREE.LineMaterial({
      color: 0xffffff,
      linewidth: HALO_WIDTH_PX,
      transparent: true,
      opacity: HALO_OPACITY,
      vertexColors: true,
      blending: THREE.NormalBlending,
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(haloMaterial);
    haloMaterial.worldUnits = false;
    haloMaterial.side = THREE.DoubleSide;
    haloMaterial.resolution.set(viewW, viewH);
    installFlowPatch(haloMaterial, flowUniforms, "halo");
    const haloMesh = new THREE.LineSegments2(linkSys.geometry, haloMaterial);
    haloMesh.frustumCulled = false;
    haloMesh.visible = linkSys.count > 0;

    // Selected-bolt overlay (SPEC: bolts touching the selected node draw at
    // alpha 0.8 / 2.5px / blur 20): SEL_MAX slots; each frame the selected
    // node's links draw as ONE full-chord A→B segment each — NOT the
    // 16-segment slab. This material is ADDITIVE (its unused-black-slot trick
    // needs a black-is-invisible mode), and under additive every collinear
    // joint's round-cap overlap doubles into a bright bead — a dotted line.
    // One segment has no joints, so the selected wires are perfectly smooth.
    // DELIBERATELY NOT premult-MAX like the strike family: these single
    // chords all share the selected node as an endpoint, so geometrically
    // they can only cross AT that node — which sits under its opaque orb.
    // No joint beads, no visible crossings → additive changes nothing here
    // and keeps this long-verified overlay on the stock unpatched program.
    // Unused slots stay black (invisible) AND clipped out via instanceCount.
    const floatsPerEdge = BOLT_SEGMENTS * 6;
    const selPositions = new Float32Array(SEL_MAX * floatsPerEdge);
    const selColors = new Float32Array(SEL_MAX * floatsPerEdge);
    const selGeometry = new THREE.LineSegmentsGeometry();
    track.geometries.push(selGeometry);
    selGeometry.setPositions(selPositions);
    selGeometry.setColors(selColors);
    selGeometry.instanceCount = 0; // nothing selected yet — draw no instances
    const selBuffer = selGeometry.attributes.instanceStart.data;
    const selColorBuffer = selGeometry.attributes.instanceColorStart.data;
    const selMaterial = new THREE.LineMaterial({
      color: 0xffffff,
      linewidth: SEL_WIDTH_PX,
      transparent: true,
      opacity: SEL_OPACITY,
      vertexColors: true,
      blending: THREE.AdditiveBlending, // black unused slots stay invisible
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(selMaterial);
    selMaterial.worldUnits = false;
    selMaterial.side = THREE.DoubleSide;
    selMaterial.resolution.set(viewW, viewH);
    const selMesh = new THREE.LineSegments2(selGeometry, selMaterial);
    selMesh.frustumCulled = false;
    selMesh.visible = false;

    // Thunder-strike CORE overlay: STRIKE_MAX slots of jagged segment pairs.
    // Slot geometry is a midpoint-displacement bolt (jagOffsets1D — 2^4+1
    // stations matches BOLT_SEGMENTS=16) re-rolled at each re-strike; per-slot
    // vertex colors carry the electric-ramp blaze (uniform along the bolt).
    // Blending: premultiplied MAX when available (joint/cap overlaps SATURATE
    // — no beads), constructor's additive as the WebGL1-no-extension fallback.
    // Zeroed slot colors are invisible under BOTH (black never wins a max and
    // adds nothing under additive), so retired slots keep stale positions
    // safely and the dim-to-black clear contract is unchanged.
    const strikePositions = new Float32Array(STRIKE_MAX * floatsPerEdge);
    const strikeColors = new Float32Array(STRIKE_MAX * floatsPerEdge);
    const strikeOff = new Float32Array(STRIKE_MAX * BOLT_STATIONS); // jag offsets per slot
    const strikeGeometry = new THREE.LineSegmentsGeometry();
    track.geometries.push(strikeGeometry);
    strikeGeometry.setPositions(strikePositions);
    strikeGeometry.setColors(strikeColors);
    const strikeBuffer = strikeGeometry.attributes.instanceStart.data;
    const strikeColorBuffer = strikeGeometry.attributes.instanceColorStart.data;
    const strikeMaterial = new THREE.LineMaterial({
      color: 0xffffff,
      linewidth: STRIKE_WIDTH_PX,
      transparent: true,
      opacity: 1, // intensity lives in the vertex colors
      vertexColors: true,
      blending: THREE.AdditiveBlending, // fallback — upgraded to premult-MAX below when capable
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(strikeMaterial);
    strikeMaterial.worldUnits = false;
    strikeMaterial.side = THREE.DoubleSide;
    strikeMaterial.resolution.set(viewW, viewH);
    if (maxBlendOK) installMaxPremultPatch(strikeMaterial, "strike-core");
    const strikeMesh = new THREE.LineSegments2(strikeGeometry, strikeMaterial);
    strikeMesh.frustumCulled = false;
    strikeMesh.visible = linkSys.count > 0;

    // Strike INNER GLOW — the bolt's heat bleeding out: a SECOND buffer view
    // over strikePositions (exactly the halo↔linkSys pattern: floats written
    // once, every view's needsUpdate flipped) with its OWN color slab so it
    // can outlive the core — hot blue-white, dying at STRIKE_GLOW_END (~180ms
    // past the core at STRIKE_CORE_END). Premult-MAX like the rest of the
    // strike family (its own joints saturate, and the core no longer double-
    // brightens over it); STRIKE_GLOW_OPACITY rides through the premultiply.
    // Unused zeroed slots stay invisible under max AND the additive fallback.
    const glowColors = new Float32Array(STRIKE_MAX * floatsPerEdge);
    const strikeGlowGeometry = new THREE.LineSegmentsGeometry();
    track.geometries.push(strikeGlowGeometry);
    strikeGlowGeometry.setPositions(strikePositions);
    strikeGlowGeometry.setColors(glowColors);
    const glowPosBuffer = strikeGlowGeometry.attributes.instanceStart.data;
    const glowColorBuffer = strikeGlowGeometry.attributes.instanceColorStart.data;
    const strikeGlowMaterial = new THREE.LineMaterial({
      color: 0xffffff,
      linewidth: STRIKE_GLOW_WIDTH_PX,
      transparent: true,
      opacity: STRIKE_GLOW_OPACITY,
      vertexColors: true,
      blending: THREE.AdditiveBlending, // fallback — upgraded to premult-MAX below when capable
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(strikeGlowMaterial);
    strikeGlowMaterial.worldUnits = false;
    strikeGlowMaterial.side = THREE.DoubleSide;
    strikeGlowMaterial.resolution.set(viewW, viewH);
    if (maxBlendOK) installMaxPremultFalloffPatch(strikeGlowMaterial, "strike-glow");
    const strikeGlowMesh = new THREE.LineSegments2(strikeGlowGeometry, strikeGlowMaterial);
    strikeGlowMesh.frustumCulled = false;
    strikeGlowMesh.visible = linkSys.count > 0;

    // Strike OUTER AURA — the dramatic "shady border": a THIRD buffer view
    // over strikePositions with its own color slab. Widest and faintest of
    // the light layers, pinned deep-violet by STRIKE_AURA_RAMP_T, and the
    // longest-lived — the whole strike retires only when the aura dies at
    // STRIKE_AURA_END, so every bolt leaves a dying violet wash. Premult-MAX
    // (soft overlapping washes are exactly what max was made for — the aura
    // never stacks itself or the glow toward white); zeroed slots invisible
    // under max and the additive fallback alike.
    const auraColors = new Float32Array(STRIKE_MAX * floatsPerEdge);
    const strikeAuraGeometry = new THREE.LineSegmentsGeometry();
    track.geometries.push(strikeAuraGeometry);
    strikeAuraGeometry.setPositions(strikePositions);
    strikeAuraGeometry.setColors(auraColors);
    const auraPosBuffer = strikeAuraGeometry.attributes.instanceStart.data;
    const auraColorBuffer = strikeAuraGeometry.attributes.instanceColorStart.data;
    const strikeAuraMaterial = new THREE.LineMaterial({
      color: 0xffffff,
      linewidth: STRIKE_AURA_WIDTH_PX,
      transparent: true,
      opacity: STRIKE_AURA_OPACITY,
      vertexColors: true,
      blending: THREE.AdditiveBlending, // fallback — upgraded to premult-MAX below when capable
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(strikeAuraMaterial);
    strikeAuraMaterial.worldUnits = false;
    strikeAuraMaterial.side = THREE.DoubleSide;
    strikeAuraMaterial.resolution.set(viewW, viewH);
    if (maxBlendOK) installMaxPremultFalloffPatch(strikeAuraMaterial, "strike-aura");
    const strikeAuraMesh = new THREE.LineSegments2(strikeAuraGeometry, strikeAuraMaterial);
    strikeAuraMesh.frustumCulled = false;
    strikeAuraMesh.visible = linkSys.count > 0;

    // Strike BRANCHES — 1–2 short fork arcs per strike, budding from interior
    // stations of the main bolt. Slot pool: branch slots slot*2 and slot*2+1
    // belong to strike slot `slot` — a fixed mapping, no searching, no
    // allocation. Same jag algorithm as the parent, thinner + dimmer, re-rolled
    // with the parent, fading with its core. Premult-MAX (additive fallback);
    // unused slots keep zeroed colors — invisible under both.
    const BRANCH_MAX = STRIKE_MAX * 2;
    const branchPositions = new Float32Array(BRANCH_MAX * floatsPerEdge);
    const branchColors = new Float32Array(BRANCH_MAX * floatsPerEdge);
    const branchOff = new Float32Array(BRANCH_MAX * BOLT_STATIONS); // fork jag offsets
    const branchSpec = new Float32Array(BRANCH_MAX * 3); // [bud station t, fork angle, reach fraction]
    const branchCount = new Uint8Array(STRIKE_MAX); // live forks per strike slot
    const branchGeometry = new THREE.LineSegmentsGeometry();
    track.geometries.push(branchGeometry);
    branchGeometry.setPositions(branchPositions);
    branchGeometry.setColors(branchColors);
    const branchBuffer = branchGeometry.attributes.instanceStart.data;
    const branchColorBuffer = branchGeometry.attributes.instanceColorStart.data;
    const branchMaterial = new THREE.LineMaterial({
      color: 0xffffff,
      linewidth: BRANCH_WIDTH_PX,
      transparent: true,
      opacity: 1, // intensity lives in the vertex colors
      vertexColors: true,
      blending: THREE.AdditiveBlending, // fallback — upgraded to premult-MAX below when capable
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(branchMaterial);
    branchMaterial.worldUnits = false;
    branchMaterial.side = THREE.DoubleSide;
    branchMaterial.resolution.set(viewW, viewH);
    if (maxBlendOK) installMaxPremultPatch(branchMaterial, "strike-branch");
    const branchMesh = new THREE.LineSegments2(branchGeometry, branchMaterial);
    branchMesh.frustumCulled = false;
    branchMesh.visible = linkSys.count > 0;

    // Active strikes: { edge, start, slot, rolled } (rolled = re-strike stage).
    const strikes = [];
    let nextStrikeAt = -1;
    const strikeGap = () => STRIKE_GAP_MIN_MS + Math.random() * STRIKE_GAP_VAR_MS;
    // Roll a strike slot's whole SHAPE: the main-bolt jag plus 1–2 fork
    // branches (bud station / angle / reach + their own jags). Runs at spawn
    // and at each re-strike moment only — never per frame — so branches always
    // re-roll together with their parent (the flicker stays one event).
    const rollStrikeShape = (slot) => {
      jagOffsets1D(4, strikeOff.subarray(slot * BOLT_STATIONS, (slot + 1) * BOLT_STATIONS));
      const n = 1 + (Math.random() < BRANCH_SECOND_P ? 1 : 0);
      branchCount[slot] = n;
      for (let b = 0; b < 2; b++) {
        const bs = slot * 2 + b;
        if (b < n) {
          rollBranch(Math.random, branchSpec, bs * 3, BRANCH_ROLL);
          jagOffsets1D(4, branchOff.subarray(bs * BOLT_STATIONS, (bs + 1) * BOLT_STATIONS));
        } else {
          // The unrolled fork slot must go dark NOW — it may hold a previous
          // strike's colors (positions may stay stale: black never wins a max
          // and adds nothing under the additive fallback — gone either way).
          branchColors.fill(0, bs * floatsPerEdge, (bs + 1) * floatsPerEdge);
        }
      }
    };
    const spawnStrike = (t) => {
      if (linkSys.count === 0 || strikes.length >= STRIKE_MAX) return;
      const busy = new Set(strikes.map((s) => s.edge));
      // While hovering, strike only the hovered node's edges (isolation holds).
      const pool = hoveredIdx >= 0 ? linkAdj[hoveredIdx] : null;
      let edge = -1;
      for (let tries = 0; tries < 6; tries++) {
        const cand = pool
          ? (pool.length ? pool[Math.floor(Math.random() * pool.length)] : -1)
          : Math.floor(Math.random() * linkSys.count);
        if (cand >= 0 && !busy.has(cand)) { edge = cand; break; }
      }
      if (edge < 0) return;
      const used = new Set(strikes.map((s) => s.slot));
      let slot = -1;
      for (let s = 0; s < STRIKE_MAX; s++) { if (!used.has(s)) { slot = s; break; } }
      if (slot < 0) return;
      rollStrikeShape(slot);
      strikes.push({ edge, start: t, slot, rolled: 0 });
    };
    // Scratch RGB triples for the electric ramp — created ONCE at build;
    // updateStrikes writes into them every frame (zero per-frame allocation).
    const coreRgb = new Float32Array(3);
    const glowRgb = new Float32Array(3);
    const auraRgb = new Float32Array(3);
    const branchRgb = new Float32Array(3);
    const updateStrikes = (t) => {
      if (linkSys.count === 0) return;
      if (nextStrikeAt < 0) nextStrikeAt = t + strikeGap();
      if (t >= nextStrikeAt) { spawnStrike(t); nextStrikeAt = t + strikeGap(); }
      for (let s = strikes.length - 1; s >= 0; s--) {
        const st = strikes[s];
        const k = (t - st.start) / STRIKE_DUR_MS;
        const base = st.slot * floatsPerEdge;
        const bBase = st.slot * 2 * floatsPerEdge; // this strike's two fork slots
        // The strike retires when its LAST light dies — the outer aura. Core
        // and branches went dark at STRIKE_CORE_END, the inner glow at
        // STRIKE_GLOW_END; their envelopes already write zeros past those.
        if (k >= STRIKE_AURA_END) {
          strikeColors.fill(0, base, base + floatsPerEdge);
          glowColors.fill(0, base, base + floatsPerEdge);
          auraColors.fill(0, base, base + floatsPerEdge);
          branchColors.fill(0, bBase, bBase + 2 * floatsPerEdge);
          strikes.splice(s, 1);
          continue;
        }
        // Re-strikes: fresh jag shape + fresh forks at each roll moment.
        while (st.rolled < STRIKE_REROLLS.length && k >= STRIKE_REROLLS[st.rolled]) {
          rollStrikeShape(st.slot);
          st.rolled++;
        }
        // Envelopes — the cinematic read: instant snap, brief hold, then the
        // layers die INSIDE OUT on cubic ease-outs — core at STRIKE_CORE_END,
        // hot inner glow at STRIKE_GLOW_END (~+180ms), violet outer aura last
        // at STRIKE_AURA_END (~+285ms). The stepped shimmer rides the core
        // (thunder never dies smoothly); the glow layers carry softer echoes
        // of it, PLUS the camera-flash pop at birth and every re-strike.
        const shimmer = 0.78 + 0.22 * hash01(Math.floor(t / 45) * 0.618 + st.edge);
        const flash = strikeOvershoot(k, STRIKE_FLASH_EVENTS, STRIKE_FLASH_WINDOW, STRIKE_FLASH_BOOST);
        const coreI = strikeEnvelope(k, STRIKE_HOLD, STRIKE_CORE_END) * shimmer;
        const glowI = strikeEnvelope(k, STRIKE_HOLD, STRIKE_GLOW_END)
          * (1 - STRIKE_GLOW_SHIMMER + STRIKE_GLOW_SHIMMER * shimmer) * flash;
        const auraI = strikeEnvelope(k, STRIKE_HOLD, STRIKE_AURA_END)
          * (1 - STRIKE_AURA_SHIMMER + STRIKE_AURA_SHIMMER * shimmer) * flash;
        // Geometry: jagged bolt over the LIVE chord.
        const A = nodes[linkA[st.edge]];
        const B = nodes[linkB[st.edge]];
        if (!A || !B) {
          strikeColors.fill(0, base, base + floatsPerEdge);
          glowColors.fill(0, base, base + floatsPerEdge);
          auraColors.fill(0, base, base + floatsPerEdge);
          branchColors.fill(0, bBase, bBase + 2 * floatsPerEdge);
          strikes.splice(s, 1);
          continue;
        }
        const dx = B.x - A.x;
        const dy = B.y - A.y;
        const len = Math.hypot(dx, dy);
        const ux = len === 0 ? 0 : dx / len;
        const uy = len === 0 ? 0 : dy / len;
        const nx = -uy;
        const ny = ux;
        const S = Math.min(len * STRIKE_JAG_RATIO, STRIKE_JAG_MAX_PX);
        const offBase = st.slot * BOLT_STATIONS; // direct indexing — no per-frame subarray views
        // Planetary depth: the bolt's stations lerp z between its endpoints'
        // shells (jag offsets stay in the sim plane), so strikes cling to
        // their wire while the cloud yaws.
        const zA = zOff[linkA[st.edge]];
        const zB = zOff[linkB[st.edge]];
        let w = base;
        let px = A.x;
        let py = A.y;
        let pz = zA;
        for (let j = 1; j <= BOLT_SEGMENTS; j++) {
          const tj = j / BOLT_SEGMENTS;
          const o = strikeOff[offBase + j] * S;
          const sx = A.x + dx * tj + nx * o;
          const sy = A.y + dy * tj + ny * o;
          const sz = zA + (zB - zA) * tj;
          strikePositions[w] = px; strikePositions[w + 1] = py; strikePositions[w + 2] = pz;
          strikePositions[w + 3] = sx; strikePositions[w + 4] = sy; strikePositions[w + 5] = sz;
          w += 6;
          px = sx; py = sy; pz = sz;
        }
        // ELECTRIC colors — the fixed ramp, uniform along the bolt (uniform
        // per-station brightness = the bolt reads as ONE arc, never beads):
        // white-hot core, blue-white heat bleed (STRIKE_GLOW_RAMP_T high),
        // deep-violet wash (STRIKE_AURA_RAMP_T pinned low) — the three lights
        // separate in hue as well as width, so the layering reads at a glance.
        electricColor(coreI, coreRgb);
        const coR = coreRgb[0] * coreI;
        const coG = coreRgb[1] * coreI;
        const coB = coreRgb[2] * coreI;
        electricColor(glowI * STRIKE_GLOW_RAMP_T, glowRgb);
        const glR = glowRgb[0] * glowI;
        const glG = glowRgb[1] * glowI;
        const glB = glowRgb[2] * glowI;
        electricColor(auraI * STRIKE_AURA_RAMP_T, auraRgb);
        const auR = auraRgb[0] * auraI;
        const auG = auraRgb[1] * auraI;
        const auB = auraRgb[2] * auraI;
        for (let j = 0; j < floatsPerEdge; j += 3) {
          strikeColors[base + j] = coR; strikeColors[base + j + 1] = coG; strikeColors[base + j + 2] = coB;
          glowColors[base + j] = glR; glowColors[base + j + 1] = glG; glowColors[base + j + 2] = glB;
          auraColors[base + j] = auR; auraColors[base + j + 1] = auG; auraColors[base + j + 2] = auB;
        }
        // BRANCHES: fork arcs budding from the CURRENT jag's interior stations —
        // geometry rebuilt from the live chord every frame, params/jags only
        // re-rolled with the parent. They carry the core envelope at
        // BRANCH_GAIN and sample the ramp slightly lower (bluer than the
        // white-hot core). A degenerate chord (< 1px) draws no forks.
        const bn = len < 1 ? 0 : branchCount[st.slot];
        electricColor(coreI * 0.85, branchRgb);
        const brR = branchRgb[0] * coreI * BRANCH_GAIN;
        const brG = branchRgb[1] * coreI * BRANCH_GAIN;
        const brB = branchRgb[2] * coreI * BRANCH_GAIN;
        for (let b = 0; b < 2; b++) {
          const bs = st.slot * 2 + b;
          const bcBase = bs * floatsPerEdge;
          if (b >= bn) { branchColors.fill(0, bcBase, bcBase + floatsPerEdge); continue; }
          const sp = bs * 3;
          const js = Math.round(branchSpec[sp] * BOLT_SEGMENTS); // bud station (t 0.25–0.75 → 4..12)
          const bang = branchSpec[sp + 1];
          const blen = branchSpec[sp + 2] * len;
          const o0 = strikeOff[offBase + js] * S;
          const bx0 = A.x + dx * (js / BOLT_SEGMENTS) + nx * o0;
          const by0 = A.y + dy * (js / BOLT_SEGMENTS) + ny * o0;
          const bz0 = zA + (zB - zA) * (js / BOLT_SEGMENTS); // fork lives flat at its bud's depth
          const ca = Math.cos(bang);
          const sa = Math.sin(bang);
          const bux = ux * ca - uy * sa; // chord direction rotated by the fork angle
          const buy = ux * sa + uy * ca;
          const bnx = -buy;
          const bny = bux;
          const bdx = bux * blen;
          const bdy = buy * blen;
          const Sb = Math.min(blen * STRIKE_JAG_RATIO, STRIKE_JAG_MAX_PX);
          const boffBase = bs * BOLT_STATIONS;
          let bw = bcBase;
          let bpx = bx0;
          let bpy = by0;
          for (let j = 1; j <= BOLT_SEGMENTS; j++) {
            const tj = j / BOLT_SEGMENTS;
            const bo = branchOff[boffBase + j] * Sb;
            const sx = bx0 + bdx * tj + bnx * bo;
            const sy = by0 + bdy * tj + bny * bo;
            branchPositions[bw] = bpx; branchPositions[bw + 1] = bpy; branchPositions[bw + 2] = bz0;
            branchPositions[bw + 3] = sx; branchPositions[bw + 4] = sy; branchPositions[bw + 5] = bz0;
            bw += 6;
            bpx = sx; bpy = sy;
          }
          for (let j = 0; j < floatsPerEdge; j += 3) {
            branchColors[bcBase + j] = brR; branchColors[bcBase + j + 1] = brG; branchColors[bcBase + j + 2] = brB;
          }
        }
      }
      strikeBuffer.needsUpdate = true;
      strikeColorBuffer.needsUpdate = true;
      glowPosBuffer.needsUpdate = true; // second view over strikePositions
      glowColorBuffer.needsUpdate = true;
      auraPosBuffer.needsUpdate = true; // third view over strikePositions
      auraColorBuffer.needsUpdate = true;
      branchBuffer.needsUpdate = true;
      branchColorBuffer.needsUpdate = true;
    };

    // Particles: pooled spawn-and-travel dots (SPEC §C) — one spawns every 80ms
    // on a random link connection, travels the straight chord from→to between
    // LIVE node positions, fading 1.0 → 0.5, then dies.
    const partPos = new Float32Array(PARTICLE_MAX * 3);
    const partSize = new Float32Array(PARTICLE_MAX);
    const partColor = new Float32Array(PARTICLE_MAX * 3);
    const partAlpha = new Float32Array(PARTICLE_MAX);
    const partEdge = new Int32Array(PARTICLE_MAX).fill(-1);
    const partProgress = new Float32Array(PARTICLE_MAX);
    const partSpeed = new Float32Array(PARTICLE_MAX);
    const partGeometry = new THREE.BufferGeometry();
    track.geometries.push(partGeometry);
    const partPosAttr = new THREE.BufferAttribute(partPos, 3);
    const partAlphaAttr = new THREE.BufferAttribute(partAlpha, 1);
    partGeometry.setAttribute("position", partPosAttr);
    partGeometry.setAttribute("aPSize", new THREE.BufferAttribute(partSize, 1));
    partGeometry.setAttribute("aPColor", new THREE.BufferAttribute(partColor, 3));
    partGeometry.setAttribute("aPAlpha", partAlphaAttr);
    const partMaterial = new THREE.ShaderMaterial({
      uniforms: { uSize: { value: pixelRatio }, uZoom: { value: 1 } },
      vertexShader: PARTICLE_VERT,
      fragmentShader: PARTICLE_FRAG,
      transparent: true,
      blending: THREE.AdditiveBlending, // glowing dots; dead slots (alpha 0) invisible
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(partMaterial);
    const partMesh = new THREE.Points(partGeometry, partMaterial);
    partMesh.frustumCulled = false;
    partMesh.visible = linkSys.count > 0 && PARTICLE_MAX > 0;

    let partSpawnAcc = 0;
    const spawnParticle = () => {
      if (linkSys.count === 0) return;
      let slot = -1;
      for (let p = 0; p < PARTICLE_MAX; p++) { if (partEdge[p] < 0) { slot = p; break; } }
      if (slot < 0) return; // pool saturated — skip (reference is unbounded, ~28 live)
      // While hovering, spawn only on the hovered node's edges (our isolation).
      const pool = hoveredIdx >= 0 ? linkAdj[hoveredIdx] : null;
      const e = pool
        ? (pool.length ? pool[Math.floor(Math.random() * pool.length)] : -1)
        : Math.floor(Math.random() * linkSys.count);
      if (e < 0) return;
      partEdge[slot] = e;
      partProgress[slot] = 0;
      partSpeed[slot] = PARTICLE_SPEED_MIN + Math.random() * PARTICLE_SPEED_VAR;
      partSize[slot] = PARTICLE_R_MIN + Math.random() * PARTICLE_R_VAR;
      const si = linkA[e];
      const [r, g, b] = hexToRgb01(si >= 0 ? nodeColors[si] : 0xffffff);
      partColor[slot * 3] = r; partColor[slot * 3 + 1] = g; partColor[slot * 3 + 2] = b;
      partGeometry.attributes.aPSize.needsUpdate = true;
      partGeometry.attributes.aPColor.needsUpdate = true;
    };
    const updateParticles = (dtMs) => {
      if (linkSys.count === 0) return;
      partSpawnAcc += dtMs;
      while (partSpawnAcc >= PARTICLE_SPAWN_MS) {
        partSpawnAcc -= PARTICLE_SPAWN_MS;
        spawnParticle();
      }
      const f = Math.min(dtMs, 50) / (50 / 3); // frames' worth (speed is per-frame)
      for (let p = 0; p < PARTICLE_MAX; p++) {
        const e = partEdge[p];
        if (e < 0) { partAlpha[p] = 0; continue; }
        partProgress[p] += partSpeed[p] * f;
        const ai = linkA[e];
        const bi = linkB[e];
        if (partProgress[p] >= 1 || ai < 0 || bi < 0) {
          partEdge[p] = -1;
          partAlpha[p] = 0;
          continue;
        }
        const A = nodes[ai];
        const B = nodes[bi];
        const t = partProgress[p];
        partPos[p * 3] = A.x + (B.x - A.x) * t;
        partPos[p * 3 + 1] = A.y + (B.y - A.y) * t;
        partAlpha[p] = 1 - t * 0.5;
      }
      partPosAttr.needsUpdate = true;
      partAlphaAttr.needsUpdate = true;
    };

    // Every content mesh rides the planetary pivot (rotInner) — the grid +
    // atmosphere stay scene-level fixed background. renderOrder is global in
    // three (independent of parenting), so the stack order is unchanged.
    suggSys.mesh.renderOrder = 0;
    haloMesh.renderOrder = 1;
    linkSys.mesh.renderOrder = 2;
    selMesh.renderOrder = 3;
    partMesh.renderOrder = 4;
    rotInner.add(suggSys.mesh);
    rotInner.add(haloMesh);
    rotInner.add(linkSys.mesh);
    rotInner.add(selMesh);
    // Strike stack (max/additive → order-independent color; kept explicit for
    // clarity): widest violet aura at the bottom, hot glow over it, forks,
    // then the white-hot core on top.
    strikeAuraMesh.renderOrder = 3.25;
    rotInner.add(strikeAuraMesh);
    strikeGlowMesh.renderOrder = 3.3;
    rotInner.add(strikeGlowMesh);
    branchMesh.renderOrder = 3.4;
    rotInner.add(branchMesh);
    strikeMesh.renderOrder = 3.5; // above the selected bolts, under particles
    rotInner.add(strikeMesh);
    rotInner.add(partMesh);

    // Nodes: ONE THREE.Points — layered gradient orbs per the reference (halo,
    // offset-center body, selection ring), per-vertex color/phase/dim/selection.
    const nodeCount = nodes.length;
    const nodePos = new Float32Array(nodeCount * 3);
    const nodePhase = new Float32Array(nodeCount);
    const nodeDim = new Float32Array(nodeCount).fill(1);
    const nodeColorAttrArr = new Float32Array(nodeCount * 3);
    const nodeSel = new Float32Array(nodeCount); // 0/1 selection flag
    const nodeHot = new Float32Array(nodeCount); // 0/1 hovered flag (hotter core + bigger bloom)
    const nodeDepth = new Float32Array(nodeCount); // −1 far … +1 near (yaw depth cue, per frame)
    for (let i = 0; i < nodeCount; i++) {
      const nd = nodes[i];
      nodePos[i * 3] = nd.x;
      nodePos[i * 3 + 1] = nd.y;
      nodePos[i * 3 + 2] = zOff[i]; // planetary depth — static; the sim owns only x/y
      nodePhase[i] = hash01(i * 12.9898 + 4.1414) * Math.PI * 2;
      const [r, g, b] = hexToRgb01(nodeColors[i]);
      nodeColorAttrArr[i * 3] = r;
      nodeColorAttrArr[i * 3 + 1] = g;
      nodeColorAttrArr[i * 3 + 2] = b;
    }
    const nodeGeometry = new THREE.BufferGeometry();
    track.geometries.push(nodeGeometry);
    const nodePosAttr = new THREE.BufferAttribute(nodePos, 3);
    nodeGeometry.setAttribute("position", nodePosAttr);
    nodeGeometry.setAttribute("aPhase", new THREE.BufferAttribute(nodePhase, 1));
    const nodeDimAttr = new THREE.BufferAttribute(nodeDim, 1);
    nodeGeometry.setAttribute("aDim", nodeDimAttr);
    nodeGeometry.setAttribute("aColor", new THREE.BufferAttribute(nodeColorAttrArr, 3));
    const nodeSelAttr = new THREE.BufferAttribute(nodeSel, 1);
    nodeGeometry.setAttribute("aSel", nodeSelAttr);
    const nodeHotAttr = new THREE.BufferAttribute(nodeHot, 1);
    nodeGeometry.setAttribute("aHot", nodeHotAttr);
    const nodeDepthAttr = new THREE.BufferAttribute(nodeDepth, 1);
    nodeGeometry.setAttribute("aDepth", nodeDepthAttr);
    const nodeMaterial = new THREE.ShaderMaterial({
      uniforms: {
        uTime: { value: 0 }, // SECONDS
        uSize: { value: pixelRatio },
        uZoom: { value: 1 },
      },
      vertexShader: NODE_VERT,
      fragmentShader: NODE_FRAG,
      transparent: true,
      blending: THREE.NormalBlending, // reference is alpha-over
      depthWrite: false,
      depthTest: false,
    });
    track.materials.push(nodeMaterial);
    const nodePoints = new THREE.Points(nodeGeometry, nodeMaterial);
    nodePoints.frustumCulled = false;
    nodePoints.renderOrder = 5;
    nodePoints.visible = nodeCount > 0;
    rotInner.add(nodePoints); // orbs ride the planetary pivot with the lines

    // Labels: DOM, not WebGL — chips styled in styles.css (.lightning-label);
    // per-node left/top stay inline because they're data, not style.
    labelLayer = document.createElement("div");
    labelLayer.className = "lightning-labels";
    const labelEls = [];
    for (const nd of nodes) {
      const el = document.createElement("div");
      el.className = "lightning-label";
      // One-word display label (graph-core assignShortLabels); the hover card
      // still shows the full title. textContent only — never innerHTML.
      el.textContent = nd.label || nd.title;
      el.style.left = nd.x + "px";
      el.style.top = nd.y + "px";
      labelLayer.appendChild(el);
      labelEls.push(el);
    }

    container.appendChild(canvas); // canvas first …
    container.appendChild(labelLayer); // … labels above it

    // Per-frame position sync: the sim mutated node.x/y — mirror into the GPU
    // point buffer (sim-space; the pivot group rotates it) and project through
    // the CURRENT yaw for everything DOM/CPU-side: label chips, the projX
    // array hit-testing + hover anchors read, and the aDepth cue (yaw depth
    // normalized to −1..1 by the cloud radius). yawX/yawDepth are the same
    // pure functions the tests pin — CPU projection and GPU rotation can
    // never disagree. Tens of nodes → cheap; zero allocation.
    const projX = new Float32Array(nodeCount); // yaw-projected world-x per node
    for (let i = 0; i < nodeCount; i++) projX[i] = nodes[i].x; // θ=0 identity — valid before the first frame
    const depthInv = 1 / (Math.max(viewW, viewH) * 0.5 + (Z_SPREAD + Z_JITTER) * 0.5);
    const syncNodesAndLabels = () => {
      for (let i = 0; i < nodeCount; i++) {
        const x = nodes[i].x;
        const px = yawX(x, rotCx, zOff[i], rotCos, rotSin);
        projX[i] = px;
        const d = yawDepth(x, rotCx, zOff[i], rotCos, rotSin) * depthInv;
        nodeDepth[i] = d < -1 ? -1 : d > 1 ? 1 : d;
        nodePos[i * 3] = x;
        nodePos[i * 3 + 1] = nodes[i].y;
        labelEls[i].style.left = px + "px";
        labelEls[i].style.top = nodes[i].y + "px"; // yaw never moves y
      }
      nodePosAttr.needsUpdate = true;
      nodeDepthAttr.needsUpdate = true;
    };

    // ---- view (zoom + pan) ---------------------------------------------------
    // World coords stay = layout/sim px; the VIEW maps them to screen:
    // screen = world * zoom + pan. The ortho frustum, the orb/particle point
    // sizes (uZoom), and the DOM label layer (CSS transform) all follow the same
    // three numbers, so every surface stays pixel-aligned at any zoom.
    let zoom = 1;
    let panX = 0;
    let panY = 0;
    const applyView = () => {
      camera.left = -panX / zoom;
      camera.right = (viewW - panX) / zoom;
      camera.top = -panY / zoom;
      camera.bottom = (viewH - panY) / zoom;
      camera.updateProjectionMatrix();
      nodeMaterial.uniforms.uZoom.value = zoom;
      partMaterial.uniforms.uZoom.value = zoom;
      labelLayer.style.transformOrigin = "0 0";
      labelLayer.style.transform = "translate(" + panX + "px, " + panY + "px) scale(" + zoom + ")";
      if (reducedMotion) renderer.render(scene, camera); // no loop → repaint now
    };

    // Adjacency, straight from the index arrays resolved at build.
    const linkAdj = []; // node index → link-system edge indices
    const suggAdj = []; // node index → suggested-system edge indices
    const adjNodes = []; // node index → Set of adjacent node indices
    for (let i = 0; i < nodeCount; i++) {
      linkAdj.push([]);
      suggAdj.push([]);
      adjNodes.push(new Set());
    }
    const indexEdges = (aIdx, bIdx, adj) => {
      for (let i = 0; i < aIdx.length; i++) {
        const a = aIdx[i];
        const b = bIdx[i];
        if (a >= 0) adj[a].push(i);
        if (b >= 0 && b !== a) adj[b].push(i);
        if (a >= 0 && b >= 0 && a !== b) {
          adjNodes[a].add(b);
          adjNodes[b].add(a);
        }
      }
    };
    indexEdges(linkA, linkB, linkAdj);
    indexEdges(suggA, suggB, suggAdj);

    // ---- hover isolation (ours) ----------------------------------------------
    // Colors/attributes are rewritten ONLY on hover change, never per frame.
    let hoveredIdx = -1;

    // Restore true source colors when idle; when hovering, dim EVERY edge to
    // base*HOVER_EDGE_DIM then restore the adjacent edges to full base color.
    const fillEdgeColors = (sys, adjList) => {
      if (sys.count === 0) return;
      if (!adjList) {
        sys.colors.set(sys.baseColors);
      } else {
        for (let j = 0; j < sys.colors.length; j++) sys.colors[j] = sys.baseColors[j] * HOVER_EDGE_DIM;
        for (const i of adjList) sys.colorSlots[i].set(sys.baseColorSlots[i]);
      }
      sys.colorBuffer.needsUpdate = true;
    };

    const applyHover = (idx) => {
      const hovering = idx >= 0;
      // linkSys hover isolation lives in the instDim per-instance attribute the
      // patched wire/halo shaders read — non-adjacent wires fade, the hovered
      // node's wires stay full. Written HERE on hover change only, never per
      // frame; the GPU flow wave keeps riding the dimmed hue.
      if (linkDimAttr) {
        const dims = linkDimAttr.array;
        if (!hovering) dims.fill(1);
        else {
          dims.fill(HOVER_EDGE_DIM);
          for (const e of linkAdj[idx]) dims[e] = 1;
        }
        linkDimAttr.needsUpdate = true;
      }
      fillEdgeColors(suggSys, hovering ? suggAdj[idx] : null);
      for (let i = 0; i < nodeCount; i++) {
        nodeDim[i] = !hovering ? 1 : i === idx ? 1 : adjNodes[idx].has(i) ? HOVER_NODE_ADJ : HOVER_NODE_REST;
        nodeHot[i] = hovering && i === idx ? 1 : 0; // hovered orb answers (hover-CHANGE writes only)
      }
      nodeDimAttr.needsUpdate = true;
      nodeHotAttr.needsUpdate = true;
      for (let i = 0; i < labelEls.length; i++) {
        const cl = labelEls[i].classList;
        if (!hovering || i === idx || adjNodes[idx].has(i)) cl.remove("dim");
        else cl.add("dim");
        if (hovering && i === idx) cl.add("hot");
        else cl.remove("hot");
      }
      // Reduced motion has no animation loop: paint the new state right away
      // (the instDim attribute uploads on this render).
      if (reducedMotion) renderer.render(scene, camera);
    };

    const setHovered = (idx) => {
      if (idx === hoveredIdx) return; // fire on CHANGE only
      hoveredIdx = idx;
      applyHover(idx);
      if (typeof onHover === "function") {
        try {
          if (idx < 0) onHover(null);
          // Anchor in SCREEN px (yaw-projected world × view) — the hover card
          // must land beside the orb at any zoom/pan AND any rotation angle.
          else onHover(nodes[idx], { x: projX[idx] * zoom + panX, y: nodes[idx].y * zoom + panY });
        } catch (_) {} // a broken callback must never take down the render loop
      }
    };

    // Screen→world helpers (canvas CSS px → sim/layout coords through the view).
    const clientXY = (ev) => {
      const rect = canvas.getBoundingClientRect();
      return [ev.clientX - rect.left, ev.clientY - rect.top];
    };
    const toWorld = (ev) => {
      const [sx, sy] = clientXY(ev);
      return [(sx - panX) / zoom, (sy - panY) / zoom];
    };

    // Hit-test in WORLD space against the YAW-PROJECTED node positions (projX
    // — what the eye actually sees under rotation; y is yaw-invariant):
    // reference uses a fixed 30px circle; we add a zoom-corrected pad so the
    // finger target stays sane zoomed out. Nearest wins.
    const nodeAtClient = (ev) => {
      const [wx, wy] = toWorld(ev);
      const hitR = Math.max(30, NODE_R_BASE + NODE_R_SEL_BONUS + NODE_PULSE_PX) + HOVER_HIT_PAD_PX / zoom;
      let best = -1;
      let bestD = Infinity;
      for (let i = 0; i < nodeCount; i++) {
        const d = Math.hypot(projX[i] - wx, nodes[i].y - wy);
        if (d <= hitR && d < bestD) {
          bestD = d;
          best = i;
        }
      }
      return best;
    };

    // ---- drag / pan / zoom ---------------------------------------------------
    // pointerdown on an orb starts a NODE DRAG (pins to the pointer — the sim
    // keeps reacting around it; release un-pins, matching the reference). On
    // empty canvas the same gesture PANS. Motion past the slop threshold
    // suppresses the click that follows. Node drag is disabled under reduced
    // motion (no loop to repaint the sim); pan/zoom repaint themselves.
    const DRAG_SLOP_PX = 4;
    let dragIdx = -1;
    let dragMoved = false;
    let panning = false;
    let downX = 0;
    let downY = 0;
    let lastSX = 0;
    let lastSY = 0;
    let clickSuppressed = false;

    const onPointerDown = (ev) => {
      const idx = reducedMotion ? -1 : nodeAtClient(ev);
      [downX, downY] = clientXY(ev);
      lastSX = downX;
      lastSY = downY;
      dragMoved = false;
      if (idx >= 0) {
        dragIdx = idx;
      } else {
        panning = true;
      }
      try { canvas.setPointerCapture(ev.pointerId); } catch (_) {}
    };
    const onPointerMove = (ev) => {
      if (dragIdx >= 0) {
        const [sx, sy] = clientXY(ev);
        if (!dragMoved && Math.hypot(sx - downX, sy - downY) > DRAG_SLOP_PX) {
          dragMoved = true;
          canvas.style.cursor = "grabbing";
        }
        if (dragMoved) {
          const [wx, wy] = toWorld(ev);
          // Yaw-INVERSE the pointer's world-x into sim space at the node's own
          // depth (rotation PAUSES during a drag, so θ is stable). Near edge-on
          // (|cosθ| small) the x equation is ill-conditioned — hold the node's
          // current sim-x and track y only; y is yaw-invariant, always exact.
          const sx = Math.abs(rotCos) < 0.15
            ? nodes[dragIdx].x
            : rotCx + (wx - rotCx - zOff[dragIdx] * rotSin) / rotCos;
          pinNode(sim, dragIdx, sx, wy);
        }
        return; // hover state stays parked on the dragged node
      }
      if (panning) {
        const [sx, sy] = clientXY(ev);
        if (!dragMoved && Math.hypot(sx - downX, sy - downY) > DRAG_SLOP_PX) {
          dragMoved = true;
          canvas.style.cursor = "grabbing";
        }
        if (dragMoved) {
          panX += sx - lastSX;
          panY += sy - lastSY;
          applyView();
        }
        lastSX = sx;
        lastSY = sy;
        return;
      }
      const idx = nodeAtClient(ev);
      canvas.style.cursor = idx >= 0 ? "grab" : "";
      setHovered(idx);
    };
    const onPointerUp = (ev) => {
      if (dragIdx < 0 && !panning) return;
      try { canvas.releasePointerCapture(ev.pointerId); } catch (_) {}
      if (dragIdx >= 0) releaseNode(sim, dragIdx);
      if (dragMoved) clickSuppressed = true; // the click after a drag/pan is not a select
      dragIdx = -1;
      panning = false;
      dragMoved = false;
      canvas.style.cursor = "";
    };
    const onPointerLeave = () => { if (dragIdx < 0 && !panning) setHovered(-1); };

    // Wheel-zoom about the cursor (reference: ×1.08/×0.92, clamp [0.2, 3]).
    const onWheel = (ev) => {
      ev.preventDefault();
      const [sx, sy] = clientXY(ev);
      const wx = (sx - panX) / zoom;
      const wy = (sy - panY) / zoom;
      zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, zoom * (ev.deltaY > 0 ? 0.92 : 1.08)));
      panX = sx - wx * zoom;
      panY = sy - wy * zoom;
      applyView();
    };

    // ---- selection ----------------------------------------------------------
    // Click a node → select (ring + bigger radius + bright bolts) and fire
    // onSelect(node). Click empty space → clear. setSelected(id) drives it from
    // the edit panel. selEdges caches the selected node's link-edge list for the
    // per-frame overlay copy.
    let selectedIdx = -1;
    let selEdges = [];
    const setSelectedIdx = (idx) => {
      if (idx === selectedIdx) return;
      if (selectedIdx >= 0) nodeSel[selectedIdx] = 0;
      selectedIdx = idx;
      if (selectedIdx >= 0) nodeSel[selectedIdx] = 1;
      nodeSelAttr.needsUpdate = true;
      selEdges = selectedIdx >= 0 ? linkAdj[selectedIdx].slice(0, SEL_MAX) : [];
      selMesh.visible = selEdges.length > 0;
      // Stale slots must go FULLY dark AND degenerate — a black line under
      // alpha-over (or a future blending change) must never resurrect the
      // phantom-connection bug. instanceCount clips the draw to live slots.
      selColors.fill(0);
      selPositions.fill(0);
      selGeometry.instanceCount = selEdges.length; // ONE full-chord segment per selected edge
      selColorBuffer.needsUpdate = true;
      selBuffer.needsUpdate = true;
      if (reducedMotion) {
        updateSelOverlay();
        renderer.render(scene, camera);
      }
    };
    // Write the selected node's links into the overlay at full base color —
    // one straight A→B segment per edge (joint-free = bead-free under the
    // additive material; see the overlay note above). Unused slots were zeroed
    // + clipped out (instanceCount) on selection change. Zero allocation.
    const updateSelOverlay = () => {
      if (selEdges.length === 0) return;
      for (let s = 0; s < selEdges.length; s++) {
        const e = selEdges[s];
        const A = nodes[linkA[e]];
        const B = nodes[linkB[e]];
        const w = s * 6;
        if (!A || !B) {
          selPositions.fill(0, w, w + 6);
          selColors.fill(0, w, w + 6);
          continue;
        }
        selPositions[w] = A.x; selPositions[w + 1] = A.y; selPositions[w + 2] = zOff[linkA[e]];
        selPositions[w + 3] = B.x; selPositions[w + 4] = B.y; selPositions[w + 5] = zOff[linkB[e]];
        const bc = linkSys.baseColorSlots[e];
        selColors[w] = bc[0]; selColors[w + 1] = bc[1]; selColors[w + 2] = bc[2];
        selColors[w + 3] = bc[0]; selColors[w + 4] = bc[1]; selColors[w + 5] = bc[2];
      }
      selBuffer.needsUpdate = true;
      selColorBuffer.needsUpdate = true;
    };
    const onClick = (ev) => {
      if (clickSuppressed) { clickSuppressed = false; return; }
      const idx = nodeAtClient(ev);
      setSelectedIdx(idx);
      if (typeof onSelect === "function") {
        try { onSelect(idx >= 0 ? nodes[idx] : null); } catch (_) {}
      }
    };
    // Double-click on EMPTY canvas → "plant a thought here" (create flow).
    // SIM coords — that's the persistence space the integration works in, so
    // the pointer's world-x is yaw-inverted at the z=0 plane (new thoughts are
    // born on the cloud's mid-plane; same edge-on guard as node drag).
    const onDblClick = (ev) => {
      if (nodeAtClient(ev) >= 0) return;
      if (typeof onCreateAt !== "function") return;
      const [wx, wy] = toWorld(ev);
      const sx = Math.abs(rotCos) < 0.15 ? wx : rotCx + (wx - rotCx) / rotCos;
      try { onCreateAt({ x: sx, y: wy }); } catch (_) {}
    };
    canvas.addEventListener("pointerdown", onPointerDown);
    canvas.addEventListener("pointermove", onPointerMove);
    canvas.addEventListener("pointerup", onPointerUp);
    canvas.addEventListener("pointerleave", onPointerLeave);
    canvas.addEventListener("click", onClick);
    canvas.addEventListener("dblclick", onDblClick);
    canvas.addEventListener("wheel", onWheel, { passive: false }); // preventDefault must work

    // ---- frame loop -----------------------------------------------------------
    // Physics once per frame (dt-capped inside), 150ms all-bolt regeneration,
    // synced alpha breathing, particle spawn/advance, per-frame endpoint pin.
    let lastT = -1;
    function tick(t) {
      const dt = lastT < 0 ? 50 / 3 : t - lastT;
      lastT = t;
      const ts = t / 1000; // seconds — the reference's time unit
      // Planetary yaw: accumulate (not derive from ts) so pausing during a
      // node drag resumes seamlessly. One rotation write + one cos/sin pair
      // per frame — the GPU turns the buffers; syncNodesAndLabels projects
      // the DOM/CPU side with the same pair.
      if (ROTATE_ON && dragIdx < 0) {
        rotTheta += (dt / 1000) * (Math.PI * 2 / ROTATE_SECS_PER_REV);
        if (rotTheta > Math.PI * 2) rotTheta -= Math.PI * 2;
        rotCos = Math.cos(rotTheta);
        rotSin = Math.sin(rotTheta);
        rotPivot.rotation.y = rotTheta;
      }
      stepForceSim(sim, t, dt);
      syncNodesAndLabels();

      linkSys.step(); // one shared position buffer — the halo mesh reads the same geometry
      suggSys.step();
      flowTime.value = ts; // GPU flowing current: its whole per-frame cost is this write
      updateSelOverlay();
      updateStrikes(t); // thunder: snap → flicker → afterglow
      updateParticles(dt);

      // Synchronized breathing (reference: 0.25 + sin(time*3)*0.1, global).
      const breathe = Math.sin(ts * BOLT_ALPHA_SPEED);
      linkSys.material.opacity = BOLT_ALPHA_BASE + breathe * BOLT_ALPHA_AMP;
      suggSys.material.opacity = SUGGESTED_OPACITY_BASE + breathe * SUGGESTED_OPACITY_AMP;
      haloMaterial.opacity = HALO_OPACITY * (linkSys.material.opacity / BOLT_ALPHA_BASE);
      nodeMaterial.uniforms.uTime.value = ts;

      renderer.render(scene, camera);
    }

    // prefers-reduced-motion → ONE static frame: settled layout, one bolt roll,
    // no loop, no breathing, no particles. Hover/selection still repaint
    // manually. Otherwise setAnimationLoop, paused while the page is hidden.
    const reducedMotion = (() => {
      try { return !!(window.matchMedia && window.matchMedia("(prefers-reduced-motion: reduce)").matches); } catch (_) { return false; }
    })();

    let disposed = false;
    if (reducedMotion) {
      // NO rotation — but a small FIXED yaw tilt so the depth still reads in
      // the static frame (gated on ROTATE_ON: switching rotation off means a
      // flat classic view everywhere). Set BEFORE the sync so labels/hover/
      // aDepth project through the same angle the GPU renders.
      if (ROTATE_ON) {
        rotTheta = REDUCED_MOTION_YAW;
        rotCos = Math.cos(rotTheta);
        rotSin = Math.sin(rotTheta);
        rotPivot.rotation.y = rotTheta;
      }
      settleForceSim(sim, 240);
      syncNodesAndLabels();
      linkSys.step();
      suggSys.step();
      flowOn.value = 0; // shader collapses to static full-hue wires (hover dim still applies)
      renderer.render(scene, camera);
    } else {
      renderer.setAnimationLoop(tick);
      onVisibility = () => {
        if (disposed) return;
        if (document.hidden) {
          renderer.setAnimationLoop(null);
        } else {
          lastT = -1; // resume with a fresh dt — no catch-up integration
          renderer.setAnimationLoop(tick);
        }
      };
      document.addEventListener("visibilitychange", onVisibility);
    }

    // MUST actually release the GL context — WKWebView caps live contexts (~8,
    // shared with the terminal panes). forceContextLoss after dispose is the
    // belt+braces the terminal renderer policy in main.js relies on.
    function dispose() {
      if (disposed) return;
      disposed = true;
      renderer.setAnimationLoop(null);
      if (onVisibility) document.removeEventListener("visibilitychange", onVisibility);
      canvas.removeEventListener("pointerdown", onPointerDown);
      canvas.removeEventListener("pointermove", onPointerMove);
      canvas.removeEventListener("pointerup", onPointerUp);
      canvas.removeEventListener("pointerleave", onPointerLeave);
      canvas.removeEventListener("click", onClick);
      canvas.removeEventListener("dblclick", onDblClick);
      canvas.removeEventListener("wheel", onWheel);
      linkSys.geometry.dispose(); // shared by the halo mesh — dispose ONCE
      linkSys.material.dispose();
      suggSys.geometry.dispose();
      suggSys.material.dispose();
      haloMaterial.dispose();
      selGeometry.dispose();
      selMaterial.dispose();
      strikeGeometry.dispose();
      strikeMaterial.dispose();
      strikeGlowGeometry.dispose();
      strikeGlowMaterial.dispose();
      strikeAuraGeometry.dispose();
      strikeAuraMaterial.dispose();
      branchGeometry.dispose();
      branchMaterial.dispose();
      partGeometry.dispose();
      partMaterial.dispose();
      gridGeometry.dispose();
      gridMaterial.dispose();
      atmoGeometry.dispose();
      atmoMaterial.dispose();
      nodeGeometry.dispose();
      nodeMaterial.dispose();
      renderer.dispose();
      try { renderer.forceContextLoss(); } catch (_) {}
      canvas.remove();
      labelLayer.remove();
    }

    // Drive selection from outside (edit panel ↔ graph sync). Maps note id → index.
    function setSelected(id) {
      let idx = -1;
      if (id != null) {
        for (let i = 0; i < nodeCount; i++) { if (nodes[i].id === id) { idx = i; break; } }
      }
      setSelectedIdx(idx);
    }

    // Snapshot the live sim positions by note id — the integration caches this
    // across re-renders (save/reload) and passes it back as `seedPositions`, so
    // the map keeps its shape instead of re-scrambling.
    function getPositions() {
      const out = {};
      for (let i = 0; i < nodeCount; i++) {
        if (nodes[i].id != null) out[nodes[i].id] = { x: nodes[i].x, y: nodes[i].y };
      }
      return out;
    }

    return { dispose, setSelected, getPositions };
  } catch (_) {
    failBuild();
    return null;
  }
}
