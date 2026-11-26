<script lang="ts">
  //! Two revisions of a mesh LOD in 3D, under one camera: side by side, or the
  //! older one drawn as wireframe over the newer. Triangles only one side has are
  //! marked (removed on the left, added on the right) and moved ones too; or the
  //! triangles are colored by material section.
  import { onDestroy, onMount } from "svelte";
  import * as THREE from "three";
  import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";

  interface MeshSection {
    name: string;
    material: string | null;
  }
  interface SideHeader {
    vertices?: number;
    triangles?: number;
    sections?: MeshSection[];
    min?: number[];
    max?: number[];
    error?: string;
  }
  interface Header {
    lod: string;
    lods: string[];
    left: SideHeader | null;
    right: SideHeader | null;
    compare: {
      sameTopology: boolean;
      movedVertices: number;
      maxMove: number;
      removedTriangles: number;
      addedTriangles: number;
    } | null;
  }
  interface Side {
    pos: Float32Array;
    idx: Uint32Array;
    sec: Uint32Array;
    st: Uint8Array;
  }

  interface Props {
    /** Both revisions of one LOD ("" = the first), as `mesh_view` sends them. */
    load: (lod: string) => Promise<ArrayBuffer>;
    leftLabel: string;
    rightLabel: string;
  }
  let { load, leftLabel, rightLabel }: Props = $props();

  type Mode = "side" | "overlay";
  type Colors = "changes" | "sections";
  let mode = $state<Mode>("side");
  let colors = $state<Colors>("changes");
  let wire = $state(false);
  let lod = $state("");
  let header = $state<Header | null>(null);
  let loading = $state(false);
  let loadError = $state("");

  // mesh_view's per-triangle status
  const SAME = 0;
  const ONE_SIDE = 1;
  const MOVED = 2;
  const NEUTRAL = 0xa9adb4;
  const REMOVED = 0xe5534b;
  const ADDED = 0x46c35f;
  const MOVED_COLOR = 0xe3a12f;
  const PALETTE = [
    0x5b8cff, 0xe0a030, 0x4ecb8d, 0xd46ad8, 0xe86a5a, 0x3fc1c9, 0xb9a34a, 0x8f7cf0, 0xe58bb0, 0x7fb36a, 0xc98d5e,
    0x6aa4d8,
  ];
  const hex = (c: number) => `#${c.toString(16).padStart(6, "0")}`;

  /** The big arrays stay out of reactive state. */
  let sides: [Side | null, Side | null] = [null, null];

  function parse(buf: ArrayBuffer): [Header, [Side | null, Side | null]] {
    const length = new DataView(buf).getUint32(0, true);
    const h: Header = JSON.parse(new TextDecoder().decode(new Uint8Array(buf, 4, length)));
    let at = Math.ceil((4 + length) / 4) * 4;
    const read = (s: SideHeader | null): Side | null => {
      if (!s || s.vertices === undefined || s.triangles === undefined) return null;
      const pos = new Float32Array(buf, at, s.vertices * 3);
      at += s.vertices * 12;
      const idx = new Uint32Array(buf, at, s.triangles * 3);
      at += s.triangles * 12;
      const sec = new Uint32Array(buf, at, s.triangles);
      at += s.triangles * 4;
      const st = new Uint8Array(buf, at, s.triangles);
      at = Math.ceil((at + s.triangles) / 4) * 4;
      return { pos, idx, sec, st };
    };
    const l = read(h.left);
    const r = read(h.right);
    return [h, [l, r]];
  }

  /** Every section name either side has, for colors that match across sides. */
  const sectionNames = $derived.by(() => {
    const names: string[] = [];
    for (const s of [header?.right, header?.left])
      for (const sec of s?.sections ?? []) if (!names.includes(sec.name)) names.push(sec.name);
    return names;
  });
  const leaf = (path: string | null) => (path ? path.split("/").pop()!.split(".").pop()! : "no material");
  /** A section's material on each side (undefined where the side lacks the section). */
  const sectionMaterials = (name: string) =>
    [header?.left, header?.right].map((s) => s?.sections?.find((sec) => sec.name === name)?.material);
  const materialText = (name: string) => {
    const [l, r] = sectionMaterials(name);
    if (l !== undefined && r !== undefined && l !== r) return `${leaf(l)} → ${leaf(r)}`;
    return leaf((r === undefined ? l : r) ?? null);
  };

  // --- three.js -----------------------------------------------------------
  let host = $state<HTMLDivElement>();
  let canvas = $state<HTMLCanvasElement>();
  let renderer: THREE.WebGLRenderer | null = null;
  let controls: OrbitControls | null = null;
  let glError = $state("");
  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(35, 1, 1, 1e7);
  scene.add(new THREE.HemisphereLight(0xffffff, 0x50525a, 1.6));
  const headlight = new THREE.DirectionalLight(0xffffff, 1.4);
  headlight.position.set(0.3, 0.5, 1);
  camera.add(headlight);
  scene.add(camera);

  interface Drawn {
    geom: THREE.BufferGeometry;
    solid: THREE.Mesh;
    edges: THREE.Mesh;
  }
  let drawn: [Drawn | null, Drawn | null] = [null, null];

  function disposeDrawn() {
    for (const d of drawn) {
      if (!d) continue;
      scene.remove(d.solid, d.edges);
      d.geom.dispose();
      for (const m of [d.solid.material, d.edges.material].flat()) (m as THREE.Material).dispose();
    }
    drawn = [null, null];
  }

  /** UE is Z-up and left-handed: swapping Y and Z keeps the shape unmirrored. */
  function positions(p: Float32Array) {
    const out = new Float32Array(p.length);
    for (let i = 0; i < p.length; i += 3) {
      out[i] = p[i];
      out[i + 1] = p[i + 2];
      out[i + 2] = p[i + 1];
    }
    return out;
  }

  /** One side as an indexed mesh whose triangles are grouped by color. */
  function build(side: 0 | 1, s: Side): Drawn {
    const own = (side === 0 ? header?.left : header?.right)?.sections ?? [];
    const sectionKey = own.map((sec) => Math.max(0, sectionNames.indexOf(sec.name)));
    const statusColor = (st: number) => (st === MOVED ? MOVED_COLOR : st === ONE_SIDE ? (side === 0 ? REMOVED : ADDED) : NEUTRAL);
    // keys: 0..2 a status, or a section index
    const keys = colors === "changes" ? 3 : Math.max(1, sectionNames.length);
    const keyOf = colors === "changes" ? (t: number) => s.st[t] : (t: number) => sectionKey[s.sec[t]] ?? 0;
    const tris = s.st.length;
    const counts = new Uint32Array(keys);
    for (let t = 0; t < tris; t++) counts[keyOf(t)]++;
    const starts = new Uint32Array(keys);
    for (let k = 1; k < keys; k++) starts[k] = starts[k - 1] + counts[k - 1];
    const fill = starts.slice();
    const index = new Uint32Array(tris * 3);
    for (let t = 0; t < tris; t++) {
      const at = fill[keyOf(t)]++ * 3;
      index[at] = s.idx[t * 3];
      index[at + 1] = s.idx[t * 3 + 1];
      index[at + 2] = s.idx[t * 3 + 2];
    }
    const geom = new THREE.BufferGeometry();
    geom.setAttribute("position", new THREE.BufferAttribute(positions(s.pos), 3));
    geom.setIndex(new THREE.BufferAttribute(index, 1));
    for (let k = 0; k < keys; k++) if (counts[k]) geom.addGroup(starts[k] * 3, counts[k] * 3, k);
    const color = (k: number) => (colors === "changes" ? statusColor(k) : PALETTE[k % PALETTE.length]);
    const solids = Array.from(
      { length: keys },
      (_, k) =>
        new THREE.MeshPhongMaterial({
          color: color(k),
          flatShading: true,
          side: THREE.DoubleSide,
          shininess: 8,
          polygonOffset: true,
          polygonOffsetFactor: 1,
          polygonOffsetUnits: 1,
        }),
    );
    const lines = Array.from(
      { length: keys },
      (_, k) =>
        new THREE.MeshBasicMaterial({
          color:
            colors === "changes"
              ? k === SAME
                ? 0x3c4048
                : color(k)
              : new THREE.Color(color(k)).multiplyScalar(0.45),
          wireframe: true,
          transparent: true,
          opacity: 0.85,
        }),
    );
    const solid = new THREE.Mesh(geom, solids);
    const edges = new THREE.Mesh(geom, lines);
    scene.add(solid, edges);
    return { geom, solid, edges };
  }

  function rebuild() {
    disposeDrawn();
    drawn = [sides[0] ? build(0, sides[0]) : null, sides[1] ? build(1, sides[1]) : null];
    requestRender();
  }

  const both = $derived(!!header?.left?.triangles && !!header?.right?.triangles);

  /** Frame both sides' bounds from the front, a little from above. */
  function fit() {
    const box = new THREE.Box3();
    for (const s of [header?.left, header?.right]) {
      if (!s?.min || !s?.max) continue;
      box.expandByPoint(new THREE.Vector3(s.min[0], s.min[2], s.min[1]));
      box.expandByPoint(new THREE.Vector3(s.max[0], s.max[2], s.max[1]));
    }
    if (box.isEmpty()) return;
    const center = box.getCenter(new THREE.Vector3());
    const radius = Math.max(1, box.getSize(new THREE.Vector3()).length() / 2);
    const dist = (radius / Math.sin(THREE.MathUtils.degToRad(camera.fov / 2))) * 0.8;
    camera.near = dist / 500;
    camera.far = dist * 50;
    camera.position.copy(center).add(new THREE.Vector3(-0.55, 0.45, 0.7).normalize().multiplyScalar(dist));
    camera.updateProjectionMatrix();
    controls?.target.copy(center);
    controls?.update();
    requestRender();
  }

  let frame = 0;
  function requestRender() {
    if (!frame) frame = requestAnimationFrame(render);
  }

  function show(solid: [boolean, boolean], edges: [boolean, boolean]) {
    for (const i of [0, 1] as const) {
      if (!drawn[i]) continue;
      drawn[i]!.solid.visible = solid[i];
      drawn[i]!.edges.visible = edges[i];
    }
  }

  function render() {
    frame = 0;
    if (!renderer || !host) return;
    const w = host.clientWidth;
    const h = host.clientHeight;
    if (!w || !h) return;
    renderer.setScissorTest(true);
    const pass = (x: number, width: number) => {
      camera.aspect = width / h;
      camera.updateProjectionMatrix();
      renderer!.setViewport(x, 0, width, h);
      renderer!.setScissor(x, 0, width, h);
      renderer!.render(scene, camera);
    };
    if (mode === "side" && both) {
      const half = Math.floor(w / 2);
      show([true, false], [wire, false]);
      pass(0, half);
      show([false, true], [false, wire]);
      pass(half, w - half);
    } else if (both) {
      // overlay: the newer one solid, the older one as lines over it
      show([false, true], [true, wire]);
      pass(0, w);
    } else {
      show([true, true], [wire, wire]);
      pass(0, w);
    }
  }

  async function reload() {
    loading = true;
    loadError = "";
    try {
      const [h, s] = parse(await load(lod));
      header = h;
      sides = s;
      const first = !lod;
      lod = h.lod;
      rebuild();
      if (first) fit();
    } catch (e) {
      loadError = String(e);
    } finally {
      loading = false;
    }
  }

  let resize: ResizeObserver | null = null;
  onMount(() => {
    try {
      renderer = new THREE.WebGLRenderer({ canvas, antialias: true });
      renderer.setPixelRatio(window.devicePixelRatio);
      renderer.setClearColor(0x232428);
    } catch (e) {
      glError = `WebGL is not available: ${e}`;
      return;
    }
    controls = new OrbitControls(camera, canvas!);
    controls.zoomToCursor = true;
    controls.addEventListener("change", requestRender);
    resize = new ResizeObserver(() => {
      renderer?.setSize(host!.clientWidth, host!.clientHeight, false);
      requestRender();
    });
    resize.observe(host!);
    void reload();
  });
  onDestroy(() => {
    if (frame) cancelAnimationFrame(frame);
    resize?.disconnect();
    controls?.dispose();
    disposeDrawn();
    renderer?.dispose();
  });

  // Regroup when the coloring changes; redraw for the rest.
  let builtColors: Colors = "changes";
  $effect(() => {
    if (colors !== builtColors) {
      builtColors = colors;
      rebuild();
    }
  });
  $effect(() => {
    void mode;
    void wire;
    requestRender();
  });

  function pickLod(e: Event) {
    lod = (e.currentTarget as HTMLSelectElement).value;
    void reload();
  }

  const cm = (v: number) => (v >= 100 ? `${(v / 100).toFixed(2)} m` : `${v.toFixed(v < 1 ? 3 : 2)} cm`);
  const summary = $derived.by(() => {
    const c = header?.compare;
    if (!c) return "";
    if (c.sameTopology) return c.movedVertices ? `${c.movedVertices.toLocaleString()} vertices moved (up to ${cm(c.maxMove)})` : "identical geometry";
    return `${c.removedTriangles.toLocaleString()} triangles removed, ${c.addedTriangles.toLocaleString()} added`;
  });
  const counts = (s: SideHeader | null | undefined) =>
    s?.triangles !== undefined ? `${s.triangles.toLocaleString()} tris · ${s.vertices!.toLocaleString()} verts` : "";
  const sideNote = (s: SideHeader | null | undefined) => (!s ? "Not in this revision." : (s.error ?? ""));
</script>

<div class="mv">
  <div class="bar">
    {#if (header?.lods.length ?? 0) > 1}
      <label
        >LOD <select value={lod} onchange={pickLod}>
          {#each header!.lods as l (l)}<option value={l}>{l}</option>{/each}
        </select></label
      >
    {/if}
    {#if both}
      <span class="group">
        {#each [["side", "Side by side"], ["overlay", "Overlay"]] as [m, label] (m)}
          <button class:on={mode === m} onclick={() => (mode = m as Mode)}>{label}</button>
        {/each}
      </span>
    {/if}
    <span class="group">
      {#each [["changes", "Changes"], ["sections", "Sections"]] as [c, label] (c)}
        <button class:on={colors === c} onclick={() => (colors = c as Colors)}>{label}</button>
      {/each}
    </span>
    <label class="opt"><input type="checkbox" bind:checked={wire} />Wireframe</label>
    {#if summary}<span class="sum">{summary}</span>{/if}
    {#if loading}<span class="dim">decoding…</span>{/if}
    {#if loadError}<span class="err">{loadError}</span>{/if}
    <span class="grow"></span>
    <button onclick={fit} title="Frame the whole mesh">Fit</button>
  </div>
  <div class="legend">
    {#if colors === "changes"}
      <span><i style:background={hex(REMOVED)}></i>removed</span>
      <span><i style:background={hex(ADDED)}></i>added</span>
      <span><i style:background={hex(MOVED_COLOR)}></i>moved</span>
      {#if mode === "overlay" && both}<span class="dim">{leftLabel} as lines over {rightLabel}</span>{/if}
    {:else}
      {#each sectionNames as name, i (name)}
        {@const [l, r] = sectionMaterials(name)}
        <span title={[...new Set([l, r].filter((m) => m !== undefined))].join(" → ")} class:changed={l !== undefined && r !== undefined && l !== r}
          ><i style:background={hex(PALETTE[i % PALETTE.length])}></i>{name} <span class="dim">({materialText(name)})</span></span
        >
      {/each}
    {/if}
  </div>
  <div class="area" bind:this={host}>
    <canvas bind:this={canvas}></canvas>
    {#if glError}<div class="note err">{glError}</div>{/if}
    {#if header}
      {#if mode === "side" && both}
        <div class="title" style="left:0">{leftLabel} <span class="dim">{counts(header.left)}</span></div>
        <div class="title" style="left:50%">{rightLabel} <span class="dim">{counts(header.right)}</span></div>
        <div class="divider"></div>
      {:else if both}
        <div class="title" style="left:0">{leftLabel} → {rightLabel}</div>
      {:else}
        {@const shown = header.right?.triangles ? "right" : "left"}
        <div class="title" style="left:0">
          {shown === "right" ? rightLabel : leftLabel}
          <span class="dim">{counts(shown === "right" ? header.right : header.left)}</span>
        </div>
        {@const other = shown === "right" ? header.left : header.right}
        {#if sideNote(other)}
          <div class="note">{shown === "right" ? leftLabel : rightLabel}: {sideNote(other)}</div>
        {/if}
        {#if !header.left?.triangles && !header.right?.triangles}
          <div class="note">{sideNote(header.right) || sideNote(header.left)}</div>
        {/if}
      {/if}
    {/if}
  </div>
</div>

<style>
  .mv {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 4px 8px;
    font-size: 12px;
    border-bottom: 1px solid var(--border);
  }
  .group {
    display: flex;
    gap: 2px;
  }
  .group button.on {
    background: #5a96e6;
    color: #fff;
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 4px;
  }
  .sum {
    font-weight: 600;
  }
  .grow {
    flex: 1;
  }
  .err {
    color: var(--warn);
  }
  .dim {
    opacity: 0.7;
  }
  .legend {
    flex: none;
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
    padding: 3px 8px;
    font-size: 11px;
    border-bottom: 1px solid var(--border);
  }
  .legend .changed {
    color: var(--writable);
    font-weight: 600;
  }
  .legend i {
    display: inline-block;
    width: 10px;
    height: 10px;
    margin-right: 4px;
    border-radius: 2px;
    vertical-align: -1px;
  }
  .area {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }
  canvas {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
  }
  canvas:active {
    cursor: grabbing;
  }
  .title {
    position: absolute;
    top: 0;
    padding: 3px 8px;
    font-size: 12px;
    font-weight: 600;
    color: #e6e6ea;
    pointer-events: none;
  }
  .divider {
    position: absolute;
    top: 0;
    bottom: 0;
    left: 50%;
    width: 1px;
    background: var(--border);
    pointer-events: none;
  }
  .note {
    position: absolute;
    left: 8px;
    bottom: 8px;
    font-size: 12px;
    color: #e6e6ea;
    pointer-events: none;
  }
</style>
