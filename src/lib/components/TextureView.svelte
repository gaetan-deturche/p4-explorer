<script lang="ts">
  //! Two revisions of a texture, compared: side by side, a swipe between them,
  //! their difference (boosted), or flicking from one to the other — any of
  //! them per channel. Pan and zoom are shared, so both sides stay aligned.
  //! Every UDIM tile (or all of them as a mosaic), source mip, slice and layer
  //! can be shown; the two sides are matched by tile coordinates.
  import { onDestroy } from "svelte";

  export interface TexBlock {
    x: number;
    y: number;
    width: number;
    height: number;
    slices: number;
    mips: number;
  }
  export interface TexLayout {
    blocks: TexBlock[];
    layers: string[];
    compression: string;
    volume: boolean;
  }

  interface Props {
    layouts: [TexLayout | null, TexLayout | null];
    /** One side's view: block index, layer, source mip, slice, max texels a side (0 = full). */
    load: (side: 0 | 1, block: number, layer: number, mip: number, slice: number, max: number) => Promise<ImageData | null>;
    leftLabel: string;
    rightLabel: string;
  }
  let { layouts, load, leftLabel, rightLabel }: Props = $props();

  const MOSAIC_CELL = 512;
  const udim = (b: { x: number; y: number }) => 1001 + b.x + 10 * b.y;
  const tileKey = (b: { x: number; y: number }) => `${b.x},${b.y}`;

  /** Every tile either side has, in UDIM order. */
  const tiles = $derived.by(() => {
    const seen = new Map<string, { x: number; y: number }>();
    for (const l of layouts) for (const b of l?.blocks ?? []) seen.set(tileKey(b), { x: b.x, y: b.y });
    return [...seen.values()].sort((a, b) => udim(a) - udim(b));
  });
  const udimSet = $derived(tiles.length > 1);
  /** "all" = the mosaic of every tile; else a tile key. */
  let tile = $state("");
  let mip = $state(0);
  let slice = $state(0);
  let layer = $state(0);
  $effect(() => {
    if (!tile && tiles.length) tile = udimSet ? "all" : tileKey(tiles[0]);
  });

  const blockOf = (side: 0 | 1, key: string) => layouts[side]?.blocks.findIndex((b) => tileKey(b) === key) ?? -1;
  /** The selected tile's block on either side (for its mip/slice counts). */
  const shownBlock = $derived.by(() => {
    if (tile === "all") return null;
    for (const s of [1, 0] as const) {
      const i = blockOf(s, tile);
      if (i >= 0) return layouts[s]!.blocks[i];
    }
    return null;
  });
  const mipCount = $derived(Math.max(1, ...layouts.flatMap((l) => (l?.blocks ?? []).filter((b) => tile === "all" || tileKey(b) === tile).map((b) => b.mips))));
  const sliceCount = $derived(shownBlock ? (layouts.some((l) => l?.volume) ? Math.max(1, shownBlock.slices >> mip) : shownBlock.slices) : 1);
  const layerCount = $derived(Math.max(1, ...layouts.map((l) => l?.layers.length ?? 1)));

  let left = $state<ImageData | null>(null);
  let right = $state<ImageData | null>(null);
  let loading = $state(false);
  let loadError = $state("");
  let request = 0;

  /** A mosaic of every tile at mip 0, `MOSAIC_CELL` texels a tile, labelled. */
  async function mosaic(side: 0 | 1): Promise<ImageData | null> {
    const l = layouts[side];
    if (!l || !tiles.length) return null;
    const minX = Math.min(...tiles.map((t) => t.x));
    const minY = Math.min(...tiles.map((t) => t.y));
    const cols = Math.max(...tiles.map((t) => t.x)) - minX + 1;
    const rows = Math.max(...tiles.map((t) => t.y)) - minY + 1;
    const canvas = new OffscreenCanvas(cols * MOSAIC_CELL, rows * MOSAIC_CELL);
    const ctx = canvas.getContext("2d")!;
    for (const [i, b] of l.blocks.entries()) {
      const img = await load(side, i, Math.min(layer, l.layers.length - 1), 0, 0, MOSAIC_CELL);
      if (!img) continue;
      const bmp = await createImageBitmap(img);
      const x = (b.x - minX) * MOSAIC_CELL;
      const y = (b.y - minY) * MOSAIC_CELL;
      ctx.drawImage(bmp, x, y, MOSAIC_CELL, MOSAIC_CELL);
      ctx.font = "bold 28px sans-serif";
      ctx.fillStyle = "rgba(0,0,0,0.6)";
      ctx.fillRect(x, y, 90, 38);
      ctx.fillStyle = "#fff";
      ctx.fillText(String(udim(b)), x + 10, y + 29);
    }
    return ctx.getImageData(0, 0, canvas.width, canvas.height);
  }

  async function viewImage(side: 0 | 1): Promise<ImageData | null> {
    if (tile === "all") return mosaic(side);
    const l = layouts[side];
    const i = blockOf(side, tile);
    if (!l || i < 0) return null;
    const b = l.blocks[i];
    if (mip >= b.mips || layer >= l.layers.length) return null;
    const slices = l.volume ? Math.max(1, b.slices >> mip) : b.slices;
    if (slice >= slices) return null;
    return load(side, i, layer, mip, slice, 0);
  }

  $effect(() => {
    // Reload both sides when the view changes.
    void tile, mip, slice, layer, layouts;
    if (!tile) return;
    const n = ++request;
    loading = true;
    loadError = "";
    Promise.all([viewImage(0), viewImage(1)])
      .then(([a, b]) => {
        if (n !== request) return;
        left = a;
        right = b;
      })
      .catch((e) => n === request && (loadError = String(e)))
      .finally(() => n === request && (loading = false));
  });
  $effect(() => {
    // Keep the pickers in range when the tile changes.
    if (mip >= mipCount) mip = 0;
    if (slice >= sliceCount) slice = 0;
  });

  type Mode = "side" | "swipe" | "diff" | "flicker";
  type Channel = "rgb" | "r" | "g" | "b" | "a";
  const both = $derived(!!left && !!right);
  const sameSize = $derived(!!left && !!right && left.width === right.width && left.height === right.height);
  let mode = $state<Mode>("side");
  let channel = $state<Channel>("rgb");
  let gain = $state(8);
  let view = $state({ x: 0, y: 0, k: 1 });
  let swipe = $state(0.5);
  let showRight = $state(true);
  let flickTimer: number | null = null;
  let areaW = $state(0);
  let areaH = $state(0);
  let fitted = false;

  const single = $derived(!both || mode !== "side");
  /** The pane size one image is shown in. */
  const paneW = $derived(single ? areaW : areaW / 2);

  /** One channel as grey, or RGB with alpha dropped (alpha has its own view). */
  function filtered(src: ImageData, ch: Channel): ImageData {
    const out = new ImageData(src.width, src.height);
    const s = src.data;
    const d = out.data;
    const pick = { r: 0, g: 1, b: 2, a: 3 } as const;
    for (let i = 0; i < s.length; i += 4) {
      if (ch === "rgb") {
        d[i] = s[i];
        d[i + 1] = s[i + 1];
        d[i + 2] = s[i + 2];
      } else {
        const v = s[i + pick[ch]];
        d[i] = d[i + 1] = d[i + 2] = v;
      }
      d[i + 3] = 255;
    }
    return out;
  }

  /** |left − right| per channel, times `gain`. */
  function difference(a: ImageData, b: ImageData, ch: Channel, k: number): ImageData {
    const out = new ImageData(a.width, a.height);
    const x = a.data;
    const y = b.data;
    const d = out.data;
    const pick = { r: 0, g: 1, b: 2, a: 3 } as const;
    for (let i = 0; i < x.length; i += 4) {
      if (ch === "rgb") {
        d[i] = Math.min(255, Math.abs(x[i] - y[i]) * k);
        d[i + 1] = Math.min(255, Math.abs(x[i + 1] - y[i + 1]) * k);
        d[i + 2] = Math.min(255, Math.abs(x[i + 2] - y[i + 2]) * k);
      } else {
        const c = pick[ch];
        d[i] = d[i + 1] = d[i + 2] = Math.min(255, Math.abs(x[i + c] - y[i + c]) * k);
      }
      d[i + 3] = 255;
    }
    return out;
  }

  /** How much differs, over all four channels. */
  const stats = $derived.by(() => {
    if (!sameSize || !left || !right) return null;
    const a = left.data;
    const b = right.data;
    let changed = 0;
    let max = 0;
    for (let i = 0; i < a.length; i += 4) {
      const m = Math.max(
        Math.abs(a[i] - b[i]),
        Math.abs(a[i + 1] - b[i + 1]),
        Math.abs(a[i + 2] - b[i + 2]),
        Math.abs(a[i + 3] - b[i + 3]),
      );
      if (m) changed++;
      if (m > max) max = m;
    }
    return { changed, total: a.length / 4, max };
  });

  const leftShown = $derived(left ? filtered(left, channel) : null);
  const rightShown = $derived(right ? filtered(right, channel) : null);
  const diffShown = $derived(sameSize && mode === "diff" ? difference(left!, right!, channel, gain) : null);

  /** Paint an ImageData into a canvas (a Svelte action, re-run on change). */
  function paint(canvas: HTMLCanvasElement, img: ImageData | null) {
    const draw = (i: ImageData | null) => {
      if (!i) return;
      canvas.width = i.width;
      canvas.height = i.height;
      canvas.getContext("2d")!.putImageData(i, 0, 0);
    };
    draw(img);
    return { update: draw };
  }

  function fit() {
    const img = right ?? left;
    if (!img || paneW <= 0 || areaH <= 0) return;
    const k = Math.min(paneW / img.width, (areaH - 22) / img.height);
    view = { k, x: (paneW - img.width * k) / 2, y: (areaH - 22 - img.height * k) / 2 };
  }
  function actual() {
    const img = right ?? left;
    if (!img) return;
    view = { k: 1, x: (paneW - img.width) / 2, y: (areaH - 22 - img.height) / 2 };
  }
  $effect(() => {
    void tile, mip;
    fitted = false;
  });
  $effect(() => {
    if (!fitted && paneW > 0 && (left || right)) {
      fitted = true;
      fit();
    }
  });

  function setMode(m: Mode) {
    mode = m;
    if (flickTimer !== null) clearInterval(flickTimer);
    flickTimer = null;
    if (m === "flicker") flickTimer = window.setInterval(() => (showRight = !showRight), 600);
    fitted = false; // the pane width changes between one and two panes
  }
  onDestroy(() => flickTimer !== null && clearInterval(flickTimer));

  // --- pan, zoom and the swipe handle ----------------------------------------
  let drag: { x: number; y: number; vx: number; vy: number; held: boolean } | null = null;
  function down(e: PointerEvent) {
    if (e.button !== 0 && e.button !== 1) return;
    drag = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y, held: false };
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    if (!drag.held) {
      if (Math.hypot(dx, dy) < 3) return;
      drag.held = true;
      (e.currentTarget as Element).setPointerCapture(e.pointerId);
    }
    view = { ...view, x: drag.vx + dx, y: drag.vy + dy };
  }
  function up() {
    drag = null;
  }
  function wheel(e: WheelEvent) {
    e.preventDefault();
    const box = (e.currentTarget as Element).getBoundingClientRect();
    const cx = e.clientX - box.left;
    const cy = e.clientY - box.top;
    const k = Math.min(64, Math.max(0.01, view.k * Math.exp(-e.deltaY * 0.0015)));
    view = { k, x: cx - ((cx - view.x) * k) / view.k, y: cy - ((cy - view.y) * k) / view.k };
  }
  function swipeDown(e: PointerEvent) {
    e.stopPropagation();
    const stage = (e.currentTarget as HTMLElement).parentElement!;
    const box = stage.getBoundingClientRect();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const at = (m: PointerEvent) => {
      swipe = Math.min(1, Math.max(0, (m.clientX - box.left) / box.width));
    };
    el.addEventListener("pointermove", at);
    el.addEventListener("pointerup", () => el.removeEventListener("pointermove", at), { once: true });
  }

  const transform = $derived(`translate(${view.x}px, ${view.y}px) scale(${view.k})`);
  const pixelated = $derived(view.k >= 2);
</script>

<div class="tv">
  {#if udimSet || mipCount > 1 || sliceCount > 1 || layerCount > 1}
    <div class="bar">
      {#if udimSet}
        <label
          >Tile <select bind:value={tile}>
            <option value="all">All tiles ({tiles.length})</option>
            {#each tiles as tl (tileKey(tl))}
              {@const onLeft = blockOf(0, tileKey(tl)) >= 0}
              {@const onRight = blockOf(1, tileKey(tl)) >= 0}
              <option value={tileKey(tl)}
                >{udim(tl)}{onLeft && !onRight ? " (removed)" : !onLeft && onRight ? " (added)" : ""}</option
              >
            {/each}
          </select></label
        >
      {/if}
      {#if mipCount > 1 && tile !== "all"}
        <label
          >Mip <select bind:value={mip}>
            {#each Array.from({ length: mipCount }, (_, i) => i) as m (m)}
              <option value={m}
                >{m}{shownBlock ? ` (${Math.max(1, shownBlock.width >> m)}×${Math.max(1, shownBlock.height >> m)})` : ""}</option
              >
            {/each}
          </select></label
        >
      {/if}
      {#if sliceCount > 1 && tile !== "all"}
        <label
          >Slice <select bind:value={slice}>
            {#each Array.from({ length: sliceCount }, (_, i) => i) as s (s)}
              <option value={s}>{s}</option>
            {/each}
          </select></label
        >
      {/if}
      {#if layerCount > 1}
        <label
          >Layer <select bind:value={layer}>
            {#each Array.from({ length: layerCount }, (_, i) => i) as s (s)}
              <option value={s}>{s} {layouts[1]?.layers[s] ?? layouts[0]?.layers[s] ?? ""}</option>
            {/each}
          </select></label
        >
      {/if}
      {#if loading}<span class="dim">decoding…</span>{/if}
      {#if loadError}<span class="err">{loadError}</span>{/if}
    </div>
  {/if}
  <div class="bar">
    {#if both}
      <span class="group">
        {#each [["side", "Side by side"], ["swipe", "Swipe"], ["diff", "Difference"], ["flicker", "Flicker"]] as [m, label] (m)}
          <button
            class:on={mode === m}
            disabled={!sameSize && m === "diff"}
            title={!sameSize && m === "diff" ? "The two revisions differ in size" : ""}
            onclick={() => setMode(m as Mode)}>{label}</button
          >
        {/each}
      </span>
    {/if}
    <span class="group">
      {#each [["rgb", "RGB"], ["r", "R"], ["g", "G"], ["b", "B"], ["a", "A"]] as [c, label] (c)}
        <button class:on={channel === c} onclick={() => (channel = c as Channel)}>{label}</button>
      {/each}
    </span>
    {#if mode === "diff" && sameSize}
      <label class="gain">×{gain}<input type="range" min="1" max="64" bind:value={gain} /></label>
    {/if}
    {#if stats}
      <span class="dim">
        {stats.changed === 0
          ? "identical pixels"
          : `${stats.changed.toLocaleString()} px differ (${((stats.changed / stats.total) * 100).toFixed(2)}%), max Δ ${stats.max}`}
      </span>
    {:else if left && right}
      <span class="dim">{left.width}×{left.height} → {right.width}×{right.height}</span>
    {/if}
    <span class="grow"></span>
    <button onclick={fit} title="Show the whole texture">Fit</button>
    <button onclick={actual} title="One texel per screen pixel">1:1</button>
  </div>
  <div class="area" bind:clientWidth={areaW} bind:clientHeight={areaH}>
    {#snippet pane(title: string, img: ImageData | null, extra = "")}
      <div class="pane">
        <div class="title">{title}{extra}</div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="stage" onpointerdown={down} onpointermove={move} onpointerup={up} onwheel={wheel}>
          {#if img}
            <canvas use:paint={img} class:pixelated style:transform></canvas>
          {:else}
            <div class="none dim">Not in this revision.</div>
          {/if}
        </div>
      </div>
    {/snippet}
    {#if !both}
      {@render pane(left ? leftLabel : rightLabel, leftShown ?? rightShown)}
    {:else if mode === "side"}
      {@render pane(leftLabel, leftShown)}
      {@render pane(rightLabel, rightShown)}
    {:else if mode === "diff"}
      {@render pane(`${leftLabel} − ${rightLabel}`, diffShown, ` (×${gain})`)}
    {:else if mode === "flicker"}
      {@render pane(showRight ? rightLabel : leftLabel, showRight ? rightShown : leftShown)}
    {:else}
      <div class="pane">
        <div class="title">{leftLabel} ◂ ▸ {rightLabel}</div>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="stage" onpointerdown={down} onpointermove={move} onpointerup={up} onwheel={wheel}>
          <canvas use:paint={leftShown} class:pixelated style:transform></canvas>
          <div class="clip" style:clip-path={`inset(0 0 0 ${swipe * 100}%)`}>
            <canvas use:paint={rightShown} class:pixelated style:transform></canvas>
          </div>
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="handle" style:left={`${swipe * 100}%`} onpointerdown={swipeDown}></div>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .tv {
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
  .gain {
    display: flex;
    align-items: center;
    gap: 4px;
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
  .area {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .pane {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
  }
  .title {
    flex: none;
    height: 22px;
    line-height: 22px;
    padding: 0 8px;
    font-size: 12px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .stage {
    position: relative;
    flex: 1;
    overflow: hidden;
    cursor: grab;
    /* a checkerboard behind transparent or missing texels */
    background: repeating-conic-gradient(#2a2b2f 0% 25%, #222326 0% 50%) 0 0 / 16px 16px;
  }
  .stage:active {
    cursor: grabbing;
  }
  canvas {
    position: absolute;
    left: 0;
    top: 0;
    transform-origin: 0 0;
  }
  canvas.pixelated {
    image-rendering: pixelated;
  }
  .clip {
    position: absolute;
    inset: 0;
  }
  .handle {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 3px;
    margin-left: -1px;
    background: #f0d050;
    cursor: ew-resize;
  }
  .none {
    padding: 12px;
    font-size: 12px;
  }
</style>
