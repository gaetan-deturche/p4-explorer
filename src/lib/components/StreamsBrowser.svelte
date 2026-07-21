<script lang="ts">
  import type { P4Record } from "$lib/p4";

  let {
    rows,
    loading,
    currentStream,
    onContext,
  }: {
    rows: P4Record[];
    loading: boolean;
    currentStream: string;
    onContext: (stream: string, e: MouseEvent) => void;
  } = $props();

  type SNode = { s: P4Record; children: SNode[] };

  // Build the stream hierarchy by Parent (mainlines are roots).
  const roots = $derived.by<SNode[]>(() => {
    const byPath = new Map<string, SNode>();
    for (const s of rows) if (s.Stream) byPath.set(s.Stream, { s, children: [] });
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
