<script lang="ts">
  //! One revision of one asset graph, drawn: comment boxes, wires, then nodes,
  //! each where the user placed it, with what changed coloured. Pan and zoom
  //! live in `view`, which the two sides of the diff share.
  import {
    geom,
    kindColor,
    STATUS_COLOR,
    HEADER,
    ROW,
    type GNode,
    type GSide,
    type Geom,
  } from "$lib/assetgraph";

  interface Props {
    side: GSide | null;
    title: string;
    view: { x: number; y: number; k: number };
    selected: string;
    /** Draw what did not change faintly, so changes stand out. */
    dim: boolean;
    onselect: (id: string) => void;
  }
  let { side, title, view = $bindable(), selected, dim, onselect }: Props = $props();

  const geoms = $derived(new Map((side?.nodes ?? []).map((n) => [n.id, geom(n)] as [string, Geom])));
  const comments = $derived((side?.nodes ?? []).filter((n) => n.shape === "comment"));
  const nodes = $derived((side?.nodes ?? []).filter((n) => n.shape !== "comment"));

  function wirePath(x1: number, y1: number, x2: number, y2: number): string {
    const d = Math.max(40, Math.abs(x2 - x1) / 2);
    return `M${x1},${y1} C${x1 + d},${y1} ${x2 - d},${y2} ${x2},${y2}`;
  }

  const wires = $derived(
    (side?.wires ?? []).flatMap((w) => {
      const a = geoms.get(w.from)?.outs.get(w.fromPin);
      const b = geoms.get(w.to)?.ins.get(w.toPin);
      if (!a || !b) return [];
      return [{ w, d: wirePath(a[0], a[1], b[0], b[1]) }];
    }),
  );

  function faded(status: string, moved = false): boolean {
    return dim && status === "same" && !moved;
  }

  function pinLabel(name: string, kind: string): string {
    return kind === "exec" && /^(execute|then)$/i.test(name) ? "" : name;
  }

  function rgba(c: [number, number, number, number] | undefined, a: number): string {
    const [r, g, b] = c ?? [1, 1, 1, 1];
    // Comment colours are linear; lift them a little for display.
    const s = (v: number) => Math.round(Math.min(1, Math.pow(Math.max(0, v), 1 / 2.2)) * 255);
    return `rgba(${s(r)},${s(g)},${s(b)},${a})`;
  }

  // --- pan and zoom ---------------------------------------------------------
  let drag: { x: number; y: number; vx: number; vy: number; held: boolean } | null = null;
  function down(e: PointerEvent) {
    if (e.button !== 0 && e.button !== 1) return;
    drag = { x: e.clientX, y: e.clientY, vx: view.x, vy: view.y, held: false };
  }
  function move(e: PointerEvent) {
    if (!drag) return;
    const dx = e.clientX - drag.x;
    const dy = e.clientY - drag.y;
    // Captured only once it is a drag: capturing on press would take the
    // click away from the node under the pointer.
    if (!drag.held) {
      if (Math.hypot(dx, dy) < 4) return;
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
    const k = Math.min(3, Math.max(0.03, view.k * Math.exp(-e.deltaY * 0.0015)));
    view = { k, x: cx - ((cx - view.x) * k) / view.k, y: cy - ((cy - view.y) * k) / view.k };
  }

  function nodeStroke(n: GNode): string {
    if (n.id === selected) return "#f0d050";
    return n.status === "same" && n.moved ? "#9aa0aa" : STATUS_COLOR[n.status];
  }
</script>

<div class="pane">
  <div class="title">{title}</div>
  {#if !side}
    <div class="empty dim">Not in this revision.</div>
  {:else}
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <svg onpointerdown={down} onpointermove={move} onpointerup={up} onwheel={wheel}>
      <g transform="translate({view.x},{view.y}) scale({view.k})">
        {#each comments as c (c.id)}
          {@const g = geoms.get(c.id)!}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <g class="comment" class:faded={faded(c.status, c.moved)} onclick={() => onselect(c.id)}>
            <rect
              x={g.x}
              y={g.y}
              width={g.w}
              height={g.h}
              rx="4"
              fill={rgba(c.color, 0.18)}
              stroke={c.status === "same" && c.id !== selected ? rgba(c.color, 0.5) : nodeStroke(c)}
              stroke-width={c.status === "same" && c.id !== selected ? 1 : 3}
              stroke-dasharray={c.status === "same" && c.moved ? "8 5" : ""}
            />
            <text x={g.x + 8} y={g.y + 18} class="ctext">{c.label}</text>
          </g>
        {/each}
        {#each wires as { w, d } (w.from + w.fromPin + w.to + w.toPin)}
          <path
            {d}
            class="wire"
            class:faded={faded(w.status)}
            stroke={w.status === "same" ? kindColor(w.kind) : STATUS_COLOR[w.status]}
            stroke-width={w.status === "same" ? (w.kind === "exec" ? 2.5 : 1.8) : 4}
          />
        {/each}
        {#each nodes as n (n.id)}
          {@const g = geoms.get(n.id)!}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <g class="node" class:faded={faded(n.status, n.moved)} onclick={(e) => (e.stopPropagation(), onselect(n.id))}>
            {#if n.shape === "knot"}
              <circle cx={g.x + g.w / 2} cy={g.y + g.h / 2} r="6" fill="#444" stroke={nodeStroke(n)} stroke-width="2" />
            {:else}
              <rect x={g.x} y={g.y} width={g.w} height={g.h} rx="6" class="body" />
              <path
                d="M{g.x},{g.y + HEADER} v{-(HEADER - 6)} a6,6 0 0 1 6,-6 h{g.w - 12} a6,6 0 0 1 6,6 v{HEADER - 6} z"
                fill={n.status === "same" ? "#3a3d44" : STATUS_COLOR[n.status]}
                fill-opacity={n.status === "same" ? 1 : 0.55}
              />
              <rect
                x={g.x}
                y={g.y}
                width={g.w}
                height={g.h}
                rx="6"
                fill="none"
                stroke={nodeStroke(n)}
                stroke-width={n.status === "same" && !n.moved && n.id !== selected ? 1 : 3}
                stroke-dasharray={n.status === "same" && n.moved && n.id !== selected ? "8 5" : ""}
              />
              <text x={g.x + 10} y={g.y + 17} class="label">{n.label}</text>
              {#each n.inputs as p, i (p.name + i)}
                {@const y = g.y + HEADER + i * ROW + ROW / 2}
                <circle cx={g.x} cy={y} r="4" fill={kindColor(p.kind)} />
                <text x={g.x + 10} y={y + 4} class="pin"
                  >{pinLabel(p.name, p.kind)}{#if p.value}<tspan class="val"> {p.value}</tspan>{/if}</text
                >
              {/each}
              {#each n.outputs as p, i (p.name + i)}
                {@const y = g.y + HEADER + i * ROW + ROW / 2}
                <circle cx={g.x + g.w} cy={y} r="4" fill={kindColor(p.kind)} />
                <text x={g.x + g.w - 10} y={y + 4} class="pin" text-anchor="end">{pinLabel(p.name, p.kind)}</text>
              {/each}
            {/if}
          </g>
        {/each}
      </g>
    </svg>
  {/if}
</div>

<style>
  .pane {
    position: relative;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    border-left: 1px solid var(--border);
    background: #1b1c1f;
  }
  .title {
    flex: none;
    padding: 3px 8px;
    font-size: 12px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel, #222);
  }
  .empty {
    padding: 12px;
    font-size: 12px;
  }
  .dim {
    opacity: 0.65;
  }
  svg {
    flex: 1;
    width: 100%;
    cursor: grab;
    user-select: none;
  }
  svg:active {
    cursor: grabbing;
  }
  .wire {
    fill: none;
    opacity: 0.9;
  }
  .faded {
    opacity: 0.32;
  }
  .wire.faded {
    opacity: 0.18;
  }
  .body {
    fill: #26282c;
    fill-opacity: 0.95;
  }
  .node,
  .comment {
    cursor: pointer;
  }
  .label {
    font: 600 12px var(--sans, sans-serif);
    fill: #f0f0f0;
  }
  .ctext {
    font: 600 16px var(--sans, sans-serif);
    fill: #e8e8e8;
  }
  .pin {
    font: 11px var(--sans, sans-serif);
    fill: #cfcfcf;
  }
  .val {
    fill: #9a9a9a;
  }
</style>
