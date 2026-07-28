<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { diffLines, changeBlocks, type DiffRow } from "$lib/linediff";

  // Opened by the Rust `open_diff_window` command with the two (already
  // materialized) sides as query params: left/right = file paths on disk,
  // ll/rl = display labels, title = window subject.
  const params = new URLSearchParams(window.location.search);
  const leftPath = params.get("left") ?? "";
  const rightPath = params.get("right") ?? "";
  const leftLabel = params.get("ll") ?? "left";
  const rightLabel = params.get("rl") ?? "right";

  let rows = $state<DiffRow[]>([]);
  let blocks = $state<number[]>([]);
  let error = $state("");
  let loading = $state(true);
  let current = $state(-1); // index into blocks (prev/next navigation)
  let body = $state<HTMLDivElement>();

  onMount(async () => {
    try {
      const [l, r] = await Promise.all([
        invoke<string>("read_text_file", { path: leftPath }),
        invoke<string>("read_text_file", { path: rightPath }),
      ]);
      rows = diffLines(l, r);
      blocks = changeBlocks(rows);
      loading = false;
      if (blocks.length) goTo(0); // land on the first change, like P4Merge
    } catch (e) {
      error = String(e);
      loading = false;
    }
  });

  function goTo(i: number) {
    if (!blocks.length) return;
    current = Math.max(0, Math.min(blocks.length - 1, i));
    const el = body?.querySelector(`[data-row="${blocks[current]}"]`);
    el?.scrollIntoView({ block: "center" });
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === "F7" && e.shiftKey) {
      e.preventDefault();
      goTo(current - 1);
    } else if (e.key === "F7") {
      e.preventDefault();
      goTo(current + 1);
    } else if (e.key === "Escape") {
      window.close();
    }
  }

  const changed = $derived(rows.filter((r) => r.type !== "same").length);

  // Render a mod row's text as [same][changed][same] using the intra-line range.
  function seg(text: string, h?: [number, number]): { pre: string; mid: string; post: string } {
    if (!h) return { pre: text, mid: "", post: "" };
    return { pre: text.slice(0, h[0]), mid: text.slice(h[0], h[1]), post: text.slice(h[1]) };
  }
</script>

<svelte:window onkeydown={onKey} />

<div class="app">
  <div class="head">
    <span class="label mono" title={leftPath}>{leftLabel}</span>
    <div class="nav">
      <button onclick={() => goTo(current - 1)} disabled={current <= 0} title="Previous change (Shift+F7)">
        ▲
      </button>
      <span class="count dim">
        {#if loading}…{:else if blocks.length}{current + 1} / {blocks.length} changes{:else}no changes{/if}
      </span>
      <button
        onclick={() => goTo(current + 1)}
        disabled={current >= blocks.length - 1}
        title="Next change (F7)"
      >
        ▼
      </button>
    </div>
    <span class="label right mono" title={rightPath}>{rightLabel}</span>
  </div>

  <div class="scroll body" bind:this={body}>
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if error}
      <div class="msg err">{error}</div>
    {:else if rows.length === 0}
      <div class="msg dim">Both files are empty.</div>
    {:else}
      <div class="grid mono">
        {#each rows as row, i (i)}
          {@const l = seg(row.l?.text ?? "", row.lh)}
          {@const r = seg(row.r?.text ?? "", row.rh)}
          <span class="num" class:hl={row.type === "del" || row.type === "mod"} data-row={i}>
            {row.l?.no ?? ""}
          </span>
          <span
            class="code left"
            class:del={row.type === "del" || row.type === "mod"}
            class:void={!row.l}
          >
            {#if row.type === "mod"}{l.pre}<span class="chg chg-del">{l.mid}</span>{l.post}
            {:else}{row.l?.text ?? ""}{/if}
          </span>
          <span class="num sep" class:hl={row.type === "add" || row.type === "mod"}>
            {row.r?.no ?? ""}
          </span>
          <span
            class="code"
            class:add={row.type === "add" || row.type === "mod"}
            class:void={!row.r}
          >
            {#if row.type === "mod"}{r.pre}<span class="chg chg-add">{r.mid}</span>{r.post}
            {:else}{row.r?.text ?? ""}{/if}
          </span>
        {/each}
      </div>
    {/if}
  </div>
</div>

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100%;
    overflow: hidden;
    background: var(--bg);
  }
  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-alt);
    flex: none;
  }
  .label {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .label.right {
    text-align: right;
  }
  .nav {
    flex: none;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .nav button {
    font-size: 10px;
    padding: 2px 8px;
  }
  .count {
    font-size: 11px;
    white-space: nowrap;
  }
  .body {
    flex: 1;
    min-height: 0;
  }
  .msg {
    padding: 20px;
    font-size: 12px;
  }
  .err {
    color: var(--warn);
  }
  .grid {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto minmax(0, 1fr);
    font-size: 12px;
    line-height: 1.5;
    min-height: 100%;
  }
  .num {
    text-align: right;
    padding: 0 8px;
    color: var(--text-dim);
    -webkit-user-select: none;
    user-select: none;
    white-space: nowrap;
    background: var(--bg-alt);
  }
  .num.sep {
    border-left: 1px solid var(--border);
  }
  .num.hl {
    color: var(--text);
  }
  .code {
    padding: 0 8px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    word-break: break-word;
  }
  .code.left {
    border-right: 1px solid var(--border);
  }
  /* Line-level tints (same language as the inline DiffView). */
  .del {
    background: rgba(192, 57, 43, 0.14);
  }
  .add {
    background: rgba(31, 157, 85, 0.14);
  }
  /* A side that has no line at this row (pure add/del alignment gap). */
  .void {
    background:
      repeating-linear-gradient(
        -45deg,
        rgba(128, 128, 128, 0.06),
        rgba(128, 128, 128, 0.06) 4px,
        transparent 4px,
        transparent 8px
      );
  }
  /* Intra-line changed range (mod rows) — stronger than the line tint. */
  .chg {
    border-radius: 2px;
  }
  .chg-del {
    background: rgba(192, 57, 43, 0.35);
  }
  .chg-add {
    background: rgba(31, 157, 85, 0.35);
  }
</style>
