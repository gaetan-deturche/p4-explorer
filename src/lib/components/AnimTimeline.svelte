<script lang="ts">
  //! One animation of both revisions on a shared time line, Sequencer-style:
  //! a row per track channel, the old revision on top and the new one below,
  //! sections as bars and keys as diamonds on the value's curve. Rows and keys
  //! that exist on one side only take that side's colour.
  import { STATUS_COLOR } from "$lib/assetgraph";
  import type { AnimKey, AnimLaneSide, AnimPair } from "$lib/p4";

  interface Props {
    anim: AnimPair;
    leftLabel: string;
    rightLabel: string;
    /** A track's label was clicked: show it in the tree. */
    onselect: (track: string) => void;
  }
  let { anim, leftLabel, rightLabel, onselect }: Props = $props();

  const LABEL_W = 260;
  const SUB = 20; // one revision's strip
  const PAD = 14;
  let width = $state(600);
  const plotW = $derived(Math.max(100, width - LABEL_W));

  /** The span every side, section and key fits in. */
  const span = $derived.by(() => {
    const ts: number[] = [0];
    const add = (t: number | null | undefined) => t != null && Number.isFinite(t) && ts.push(t);
    for (const r of [anim.left, anim.right]) r?.forEach(add);
    for (const lane of anim.lanes) {
      for (const s of [lane.left, lane.right]) {
        s?.sections.forEach(([a, b]) => (add(a), add(b)));
        s?.keys.forEach((k) => add(k.t));
      }
    }
    const t0 = Math.min(...ts);
    const t1 = Math.max(...ts);
    return { t0, t1: t1 > t0 ? t1 : t0 + 1 };
  });
  const x = (t: number) => PAD + ((t - span.t0) / (span.t1 - span.t0)) * (plotW - 2 * PAD);

  /** Ruler ticks at a round step, about eight across. */
  const ticks = $derived.by(() => {
    const range = span.t1 - span.t0;
    const raw = range / 8;
    const mag = Math.pow(10, Math.floor(Math.log10(raw)));
    const step = [1, 2, 5, 10].map((m) => m * mag).find((s) => s >= raw) ?? raw;
    const out: number[] = [];
    for (let t = Math.ceil(span.t0 / step) * step; t <= span.t1 + 1e-9; t += step) out.push(Math.round(t * 1e6) / 1e6);
    return out;
  });

  const secs = (t: number) => `${Math.round(t * 1000) / 1000}`;
  const num = (v: number) => `${Math.round(v * 1e4) / 1e4}`;

  function sameKey(a: AnimKey, b: AnimKey): boolean {
    return Math.abs(a.t - b.t) < 1e-6 && Math.abs(a.v - b.v) < 1e-6 && a.interp === b.interp;
  }

  /** Each track's rows together, in order. */
  const groups = $derived.by(() => {
    const out: { track: string; lanes: typeof anim.lanes }[] = [];
    for (const lane of anim.lanes) {
      const g = out.find((o) => o.track === lane.track);
      if (g) g.lanes.push(lane);
      else out.push({ track: lane.track, lanes: [lane] });
    }
    return out;
  });

  function trackStatus(lanes: typeof anim.lanes): string {
    const s = new Set(lanes.map((l) => l.status));
    return s.size === 1 ? [...s][0] : "changed";
  }

  /** The value range a lane's curve is drawn in, shared by both sides. */
  function valueRange(a: AnimLaneSide | null, b: AnimLaneSide | null): [number, number] {
    const vs = [...(a?.keys ?? []), ...(b?.keys ?? [])].map((k) => k.v);
    if (!vs.length) return [0, 1];
    const lo = Math.min(...vs);
    const hi = Math.max(...vs);
    return hi > lo ? [lo, hi] : [lo - 0.5, hi + 0.5];
  }

  function y(v: number, range: [number, number]): number {
    return SUB - 4 - ((v - range[0]) / (range[1] - range[0])) * (SUB - 8);
  }

  /** The curve through the keys: held flat on a constant key, straight otherwise. */
  function curve(keys: AnimKey[], range: [number, number]): string {
    if (!keys.length) return "";
    const ks = [...keys].sort((a, b) => a.t - b.t);
    let d = `M${PAD},${y(ks[0].v, range)} L${x(ks[0].t)},${y(ks[0].v, range)}`;
    for (let i = 1; i < ks.length; i++) {
      if (ks[i - 1].interp === "Constant") d += ` L${x(ks[i].t)},${y(ks[i - 1].v, range)}`;
      d += ` L${x(ks[i].t)},${y(ks[i].v, range)}`;
    }
    return d + ` L${plotW - PAD},${y(ks[ks.length - 1].v, range)}`;
  }

  /** A section bar's extent; an open bound runs to the edge. */
  function bar([a, b]: [number | null, number | null]): [number, number] {
    return [a == null ? 0 : x(a), b == null ? plotW : x(b)];
  }

  function keyColor(k: AnimKey, other: AnimLaneSide | null, mine: Side, laneStatus: string): string {
    if (laneStatus === "removed") return STATUS_COLOR.removed;
    if (laneStatus === "added") return STATUS_COLOR.added;
    if (other?.keys.some((o) => sameKey(o, k))) return "#c8c8c8";
    return mine === "left" ? STATUS_COLOR.removed : STATUS_COLOR.added;
  }

  type Side = "left" | "right";
  const playRanges = $derived(
    ([["left", anim.left], ["right", anim.right]] as [Side, [number | null, number | null] | null][]).filter(
      (p): p is [Side, [number | null, number | null]] => p[1] !== null,
    ),
  );
  function sides(lane: AnimPair["lanes"][number]): [Side, AnimLaneSide | null, AnimLaneSide | null][] {
    return [
      ["left", lane.left, lane.right],
      ["right", lane.right, lane.left],
    ];
  }

  const MARK: Record<string, string> = { same: "", changed: "~", added: "+", removed: "−" };
</script>

<div class="tl" bind:clientWidth={width}>
  <div class="ruler">
    <div class="lab dim">
      <span class="sw top"></span>{leftLabel}<br /><span class="sw bot"></span>{rightLabel}
    </div>
    <svg width={plotW} height="34">
      {#each ticks as t (t)}
        <line x1={x(t)} x2={x(t)} y1="22" y2="34" class="tick" />
        <text x={x(t)} y="16" class="tt" text-anchor="middle">{secs(t)} s</text>
      {/each}
      {#each playRanges as [side, r] (side)}
        {@const [a, b] = bar(r)}
        <rect x={a} width={Math.max(1, b - a)} y={side === "left" ? 22 : 28} height="4" class="play" />
      {/each}
    </svg>
  </div>
  <div class="lanes">
    {#if !anim.lanes.length}<div class="pad dim">No tracks.</div>{/if}
    {#each groups as g (g.track)}
      {@const ts = trackStatus(g.lanes)}
      {#each g.lanes as lane, li (lane.channel)}
        {@const range = valueRange(lane.left, lane.right)}
        <div class="lane s-{lane.status}">
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div class="lab" onclick={() => onselect(lane.track)} title="Show {lane.track} in the tree">
            {#if li === 0}<div class="track"><span class="mk">{MARK[ts]}</span>{lane.track}</div>{/if}
            <div class="chan">
              {#if lane.channel}<span class="mk">{g.lanes.length > 1 ? MARK[lane.status] : ""}</span>{lane.channel}{/if}
              {#if lane.left?.keys.length || lane.right?.keys.length}<span class="dim range"
                  >{num(range[0])} … {num(range[1])}</span
                >{/if}
            </div>
          </div>
          <svg width={plotW} height={SUB * 2 + 2}>
            {#each sides(lane) as [side, mine, other] (side)}
              <g transform="translate(0,{side === 'left' ? 0 : SUB + 2})">
                <rect x="0" y="0" width={plotW} height={SUB} class="strip" />
                {#if !mine}
                  <text x={PAD} y={SUB / 2 + 4} class="none">not in {side === "left" ? leftLabel : rightLabel}</text>
                {:else}
                  {#each mine.sections as s, si (si)}
                    {@const [a, b] = bar(s)}
                    <rect
                      x={a}
                      y="1"
                      width={Math.max(1, b - a)}
                      height={SUB - 2}
                      rx="3"
                      class="sect"
                      fill={lane.status === "added" || lane.status === "removed" ? STATUS_COLOR[lane.status] : "#3a3d44"}
                      fill-opacity={lane.status === "added" || lane.status === "removed" ? 0.25 : 0.6}
                    />
                  {/each}
                  {#if mine.keys.length}
                    <path d={curve(mine.keys, range)} class="curve" />
                    {#each mine.keys as k, ki (ki)}
                      <rect
                        x={x(k.t) - 4}
                        y={y(k.v, range) - 4}
                        width="8"
                        height="8"
                        transform="rotate(45 {x(k.t)} {y(k.v, range)})"
                        fill={keyColor(k, other, side, lane.status)}
                        class="key"
                      >
                        <title>{secs(k.t)} s = {num(k.v)} ({k.interp})</title>
                      </rect>
                    {/each}
                  {/if}
                {/if}
              </g>
            {/each}
          </svg>
        </div>
      {/each}
    {/each}
  </div>
</div>

<style>
  .tl {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    font-size: 12px;
  }
  .ruler {
    flex: none;
    display: flex;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
  }
  .lanes {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overflow-x: hidden;
  }
  .lane {
    display: flex;
    border-bottom: 1px solid var(--border);
    border-left: 3px solid transparent;
  }
  .lane.s-changed {
    border-left-color: #5a96e6;
  }
  .lane.s-added {
    border-left-color: #5faf5f;
  }
  .lane.s-removed {
    border-left-color: #d9873a;
  }
  .lab {
    flex: none;
    width: 257px;
    padding: 2px 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
    border-right: 1px solid var(--border);
    cursor: pointer;
    font-size: 11px;
    line-height: 16px;
  }
  .ruler .lab {
    cursor: default;
    width: 260px;
  }
  .track {
    font-weight: 600;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .chan {
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .range {
    margin-left: 6px;
    font-family: var(--mono);
  }
  .mk {
    display: inline-block;
    width: 12px;
    font-weight: 700;
  }
  .s-changed .mk {
    color: #7aaef0;
  }
  .s-added .mk {
    color: #7cc47c;
  }
  .s-removed .mk {
    color: #d9873a;
  }
  .dim {
    opacity: 0.65;
  }
  .pad {
    padding: 8px;
  }
  .sw {
    display: inline-block;
    width: 10px;
    height: 6px;
    margin-right: 5px;
    vertical-align: middle;
    background: var(--text-dim);
  }
  .sw.bot {
    background: var(--text);
  }
  .tick {
    stroke: var(--text-dim);
  }
  .tt {
    font: 10px var(--mono, monospace);
    fill: var(--text-dim);
  }
  .play {
    fill: var(--accent);
    opacity: 0.6;
  }
  .strip {
    fill: var(--bg-alt, transparent);
    opacity: 0.5;
  }
  .none {
    font: italic 10px var(--sans, sans-serif);
    fill: var(--text-dim);
  }
  .curve {
    fill: none;
    stroke: var(--text-dim);
    stroke-width: 1.2;
  }
  .key {
    stroke: #111;
    stroke-width: 0.8;
  }
</style>
