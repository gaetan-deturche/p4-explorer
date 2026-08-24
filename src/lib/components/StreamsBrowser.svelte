<script lang="ts">
  import type { P4Record } from "$lib/p4";

  let {
    rows,
    loading,
    currentStream,
    depotOnly,
    onDepotOnly,
    onContext,
  }: {
    rows: P4Record[];
    loading: boolean;
    currentStream: string;
    /** List only the current workspace's depot. */
    depotOnly: boolean;
    onDepotOnly: (v: boolean) => void;
    onContext: (stream: string, e: MouseEvent) => void;
  } = $props();

  /** The depot a stream path lives in: `//Curiosity/main` -> `//Curiosity`. */
  function depotOf(stream: string): string {
    const m = /^\/\/([^/]+)\//.exec(stream ?? "");
    return m ? "//" + m[1] : "";
  }
  const currentDepot = $derived(depotOf(currentStream));
  /** The rows actually listed. Scoping to the depot is the default because the
   *  rest cannot be switched to from here anyway: measured on this server, 214
   *  streams across 26 depots, of which the project at hand owns 7 — and 21 of
   *  those 214 are named "main". */
  const shown = $derived(
    depotOnly && currentDepot ? rows.filter((s) => depotOf(s.Stream) === currentDepot) : rows,
  );

  type SNode = { s: P4Record; children: SNode[] };

  // Build the stream hierarchy by Parent (mainlines are roots).
  const roots = $derived.by<SNode[]>(() => {
    const byPath = new Map<string, SNode>();
    for (const s of shown) if (s.Stream) byPath.set(s.Stream, { s, children: [] });
    const out: SNode[] = [];
    for (const node of byPath.values()) {
      const parent = node.s.Parent;
      const p = parent && parent !== "none" ? byPath.get(parent) : undefined;
      if (p) p.children.push(node);
      else out.push(node);
    }
    const cmp = (a: SNode, b: SNode) => (a.s.Stream ?? "").localeCompare(b.s.Stream ?? "");
    const sortRec = (n: SNode) => {
      n.children.sort(cmp);
      n.children.forEach(sortRec);
    };
    out.sort(cmp);
    out.forEach(sortRec);
    return out;
  });

  let collapsed = $state<Record<string, boolean>>({});
  const toggle = (p: string) => (collapsed[p] = !collapsed[p]);
</script>

<div class="panel">
  <div class="bar">
    <label title={currentDepot
      ? `List only ${currentDepot}. Off, every stream on the server is listed — including other projects', which this workspace cannot switch to.`
      : "No workspace stream to scope to"}>
      <input
        type="checkbox"
        checked={depotOnly}
        disabled={!currentDepot}
        onchange={(e) => onDepotOnly(e.currentTarget.checked)}
      />
      this depot{currentDepot ? ` (${currentDepot})` : ""}
    </label>
    <span class="grow"></span>
    <span class="dim">
      {shown.length === rows.length
        ? `${rows.length} stream${rows.length === 1 ? "" : "s"}`
        : `${shown.length} of ${rows.length}`}
    </span>
  </div>
  <div class="scroll body">
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if roots.length === 0}
      <div class="msg dim">No streams.</div>
    {:else}
      {#each roots as n (n.s.Stream)}
        {@render row(n, 0)}
      {/each}
    {/if}
  </div>
</div>

{#snippet row(n: SNode, depth: number)}
  {@const open = !collapsed[n.s.Stream]}
  {@const kids = n.children.length > 0}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="srow mono"
    class:current={n.s.Stream === currentStream}
    style="padding-left:{depth * 14 + 4}px"
    title={n.s.desc}
    oncontextmenu={(e) => onContext(n.s.Stream, e)}
  >
    {#if kids}
      <button class="tw" title="Expand / collapse" onclick={() => toggle(n.s.Stream)}>
        {open ? "▾" : "▸"}
      </button>
    {:else}
      <span class="tw-sp"></span>
    {/if}
    {#if n.s.Stream === currentStream}<span class="you">▸</span>{/if}
    <span class="sname">{n.s.Name ?? n.s.Stream}</span>
    <span class="spath dim" title={n.s.Stream}>{n.s.Stream}</span>
    <span class="stype t-{n.s.Type}">{n.s.Type ?? ""}</span>
    <span class="sowner dim">{n.s.Owner ?? ""}</span>
  </div>
  {#if kids && open}
    {#each n.children as c (c.s.Stream)}
      {@render row(c, depth + 1)}
    {/each}
  {/if}
{/snippet}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
  }
  .body {
    flex: 1;
    padding: 4px 0;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 10px;
    font-size: 11px;
    border-bottom: 1px solid var(--border);
  }
  .bar label {
    display: flex;
    align-items: center;
    gap: 5px;
    cursor: pointer;
  }
  .grow {
    flex: 1;
  }
  /* The stream path carries the depot, so two streams both called "main" are
     told apart without hovering. Dim and shrinkable: the name leads. */
  .spath {
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: 11px;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px 2px 4px;
    font-size: 12px;
    white-space: nowrap;
  }
  .srow:hover {
    background: var(--bg-hover);
  }
  .srow.current {
    background: var(--have-bg);
  }
  .srow.current .sname {
    font-weight: 700;
  }
  .you {
    flex: none;
    color: var(--have);
    margin-left: -2px;
  }
  .tw,
  .tw-sp {
    flex: none;
    width: 16px;
  }
  .tw {
    border: none;
    background: none;
    border-radius: 0;
    padding: 0;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 11px;
    line-height: 1;
  }
  .tw:hover {
    color: var(--text);
  }
  .sname {
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 22rem;
  }
  .stype {
    flex: none;
    font-size: 10px;
    padding: 0 6px;
    border-radius: 8px;
    border: 1px solid var(--border);
    color: var(--text-dim);
  }
  .t-mainline {
    color: var(--accent);
    border-color: var(--accent);
  }
  .t-release {
    color: var(--have);
    border-color: var(--have);
  }
  .t-development {
    color: #d08a1d;
    border-color: #d08a1d;
  }
  .sowner {
    flex: 1;
    text-align: right;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
</style>
