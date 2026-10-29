<script lang="ts">
  //! The asset diff window: two revisions of a .uasset / .umap compared object by
  //! object and property by property, read straight from the files — hidden
  //! properties, graph links and references included, no editor needed.
  //!
  //! The rows come from Rust as one flat tree in display order (a row's children
  //! follow it with a greater depth). What is shown is decided here: changed
  //! branches open, unchanged ones closed, and "changes only" drops the rest.
  import { onMount } from "svelte";
  import { p4, assetDiffJob, setClipboard, type AssetDiff } from "$lib/p4";
  import { cacheGet, cacheSet } from "$lib/store.svelte";

  const params = new URLSearchParams(window.location.search);
  const leftPath = params.get("left") ?? "";
  const rightPath = params.get("right") ?? "";
  const leftLabel = params.get("ll") ?? "left";
  const rightLabel = params.get("rl") ?? "right";
  const title = params.get("title") ?? "";
  const job = params.get("job") ?? "";

  const RH = 22; // row height
  /** An added or removed branch this small opens by itself. */
  const SMALL = 8;

  let data = $state<AssetDiff | null>(null);
  let error = $state("");
  let notice = $state("");
  let loading = $state(true);
  let onlyChanges = $state(true);
  let filter = $state("");
  /** Branches the user opened or closed, overriding the default. */
  let toggled = $state(new Map<number, boolean>());
  let selected = $state(-1);
  let scrollEl = $state<HTMLDivElement | null>(null);
  let scrollTop = $state(0);
  let viewH = $state(600);

  function setNotice(m: string, ms = 5000) {
    notice = m;
    window.setTimeout(() => (notice = ""), ms);
  }

  onMount(async () => {
    onlyChanges = ((await cacheGet("nav", "assetdiff-changes")) ?? "1") === "1";
    try {
      data = await p4.assetDiff(leftPath, rightPath);
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  });

  const rows = $derived(data?.rows ?? []);
  /** Index just past each row's subtree, and each row's parent. */
  const shape = $derived.by(() => {
    const end = new Array<number>(rows.length);
    const parent = new Array<number>(rows.length).fill(-1);
    const stack: number[] = [];
    for (let i = 0; i < rows.length; i++) {
      while (stack.length && rows[stack[stack.length - 1]].depth >= rows[i].depth) end[stack.pop()!] = i;
      if (stack.length) parent[i] = stack[stack.length - 1];
      stack.push(i);
    }
    for (const i of stack) end[i] = rows.length;
    return { end, parent };
  });

  function isOpen(i: number): boolean {
    const t = toggled.get(i);
    if (t !== undefined) return t;
    const r = rows[i];
    // The raw package contents behind an asset-type summary: there when wanted.
    if (r.depth === 0 && r.key === "All data") return false;
    if (r.status === "changed") return true;
    if (r.status === "same") return false;
    return shape.end[i] - i - 1 <= SMALL;
  }

  function matches(i: number, term: string): boolean {
    const r = rows[i];
    return (
      r.key.toLowerCase().includes(term) ||
      (r.left ?? "").toLowerCase().includes(term) ||
      (r.right ?? "").toLowerCase().includes(term)
    );
  }

  /** The rows on screen, in order. */
  const visible = $derived.by(() => {
    const out: number[] = [];
    const term = filter.trim().toLowerCase();
    // With a filter: the matching rows, the branches leading to them, and what
    // an open match holds.
    let keep: Uint8Array | null = null;
    if (term) {
      keep = new Uint8Array(rows.length);
      for (let i = 0; i < rows.length; i++) {
        if (!matches(i, term)) continue;
        for (let p = i; p >= 0 && !keep[p]; p = shape.parent[p]) keep[p] = 1;
        for (let c = i + 1; c < shape.end[i]; c++) keep[c] = 1;
      }
    }
    let i = 0;
    while (i < rows.length) {
      const r = rows[i];
      if ((onlyChanges && r.status === "same") || (keep && !keep[i])) {
        i = shape.end[i];
        continue;
      }
      out.push(i);
      const open = keep && keep[i] && !matches(i, term) ? true : isOpen(i);
      i = r.kids && !open ? shape.end[i] : i + 1;
    }
    return out;
  });

  const first = $derived(Math.max(0, Math.floor(scrollTop / RH) - 10));
  const last = $derived(Math.min(visible.length, Math.ceil((scrollTop + viewH) / RH) + 10));

  function toggle(i: number, open = !isOpen(i)) {
    const next = new Map(toggled);
    next.set(i, open);
    toggled = next;
  }

  /** Open or close every branch at once. */
  function setAll(open: boolean) {
    const next = new Map<number, boolean>();
    rows.forEach((r, i) => r.kids && next.set(i, open));
    toggled = next;
  }

  function setOnlyChanges(v: boolean) {
    onlyChanges = v;
    cacheSet("nav", "assetdiff-changes", v ? "1" : "0");
  }

  /** "Objects › BP_C › Properties › Speed" for the clipboard. */
  function pathOf(i: number): string {
    const parts: string[] = [];
    for (let p = i; p >= 0; p = shape.parent[p]) parts.unshift(rows[p].key);
    return parts.join(" › ");
  }

  function copyRow(i: number) {
    const r = rows[i];
    const side = (v: string | null) => v ?? "(none)";
    const text =
      r.status === "same" ? `${pathOf(i)} = ${side(r.right)}` : `${pathOf(i)}: ${side(r.left)} → ${side(r.right)}`;
    setClipboard(text).then(() => setNotice("Copied."), (e) => setNotice(String(e)));
  }

  function scrollToRow(n: number) {
    if (!scrollEl) return;
    const y = n * RH;
    if (y < scrollEl.scrollTop) scrollEl.scrollTop = y;
    else if (y + RH > scrollEl.scrollTop + scrollEl.clientHeight) scrollEl.scrollTop = y + RH - scrollEl.clientHeight;
  }

  function onKey(e: KeyboardEvent) {
    if ((e.target as HTMLElement).tagName === "INPUT") return;
    const at = visible.indexOf(selected);
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = Math.max(0, Math.min(visible.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
      selected = visible[n] ?? -1;
      scrollToRow(n);
    } else if (e.key === "ArrowRight" && selected >= 0 && rows[selected].kids) {
      toggle(selected, true);
    } else if (e.key === "ArrowLeft" && selected >= 0) {
      if (rows[selected].kids && isOpen(selected)) toggle(selected, false);
      else if (shape.parent[selected] >= 0) {
        selected = shape.parent[selected];
        scrollToRow(visible.indexOf(selected));
      }
    } else if ((e.ctrlKey || e.metaKey) && e.key === "c" && selected >= 0 && !window.getSelection()?.toString()) {
      e.preventDefault();
      copyRow(selected);
    }
  }

  async function openInUnreal() {
    try {
      const j = await assetDiffJob(job);
      setNotice("Opening Unreal diff…", 15000);
      const mode = await p4.openUnrealDiff(j.conn, leftPath, rightPath, j.name, leftLabel, rightLabel);
      setNotice(
        mode === "remote"
          ? "Diff opened in the running Unreal Editor."
          : mode === "nocompare"
            ? "No earlier revision of this asset to compare."
            : mode.startsWith("single")
              ? "No earlier revision — opened the asset in Unreal instead."
              : "Launching Unreal Editor for the diff — this takes a moment…",
        8000,
      );
    } catch (e) {
      setNotice(String(e), 8000);
    }
  }

  const MARK = { same: "", changed: "~", added: "+", removed: "−" } as const;
</script>

<svelte:window onkeydown={onKey} />

<div class="wrap">
  <div class="bar">
    <span class="name mono" title={title}>{title}</span>
    {#if data?.class}<span class="cls">{data.class}</span>{/if}
    {#if data}
      <span class="dim"
        >{data.changes === 0 ? "no differences" : `${data.changes} change${data.changes === 1 ? "" : "s"}`}{data.rawChanges
          ? ` · ${data.rawChanges} in all data`
          : ""}</span
      >
    {/if}
    <span class="grow"></span>
    <input class="filter" placeholder="Filter properties…" bind:value={filter} spellcheck="false" />
    <label class="opt"
      ><input type="checkbox" checked={onlyChanges} onchange={(e) => setOnlyChanges(e.currentTarget.checked)} />Changes
      only</label
    >
    <button onclick={() => setAll(true)} title="Open every branch">Expand all</button>
    <button onclick={() => setAll(false)} title="Close every branch">Collapse all</button>
    <button onclick={openInUnreal} title="Diff the same two files in Unreal's own asset diff">Open in Unreal</button>
  </div>
  {#if notice}<div class="note">{notice}</div>{/if}
  {#if data?.leftError}<div class="err">{leftLabel}: {data.leftError}</div>{/if}
  {#if data?.rightError}<div class="err">{rightLabel}: {data.rightError}</div>{/if}

  {#if error}
    <div class="err mono">{error}</div>
  {:else if loading}
    <div class="pad dim">Reading both revisions…</div>
  {:else}
    <div class="head">
      <span>Property</span>
      <span class="side">
        {#if data?.leftThumb}<img src={data.leftThumb} alt="" />{/if}{leftLabel}
      </span>
      <span class="side">
        {#if data?.rightThumb}<img src={data.rightThumb} alt="" />{/if}{rightLabel}
      </span>
    </div>
    <div class="scroll" bind:this={scrollEl} bind:clientHeight={viewH} onscroll={() => (scrollTop = scrollEl?.scrollTop ?? 0)}>
      {#if !visible.length}
        <div class="pad dim">
          {filter ? "Nothing matches the filter." : onlyChanges ? "The two revisions hold the same data." : "Empty."}
        </div>
      {/if}
      <div class="rows" style="height:{visible.length * RH}px">
        {#each visible.slice(first, last) as i, k (i)}
          {@const r = rows[i]}
          <!-- svelte-ignore a11y_click_events_have_key_events -->
          <!-- svelte-ignore a11y_no_static_element_interactions -->
          <div
            class="row s-{r.status}"
            class:sel={i === selected}
            style="top:{(first + k) * RH}px"
            onclick={() => (selected = i)}
            ondblclick={() => r.kids && toggle(i)}
          >
            <span class="key" style="padding-left:{6 + r.depth * 16}px" title={pathOf(i)}>
              {#if r.kids}
                <button class="tw" onclick={(e) => (e.stopPropagation(), toggle(i))}>{isOpen(i) ? "▾" : "▸"}</button>
              {:else}<span class="tw"></span>{/if}<span class="mk">{MARK[r.status]}</span>{r.key}
            </span>
            <span class="val l" class:muted={r.kids} title={r.left ?? ""}>{r.left ?? ""}</span>
            <span class="val r" class:muted={r.kids} title={r.right ?? ""}>{r.right ?? ""}</span>
          </div>
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }
  .cls {
    flex: none;
    font-size: 11px;
    padding: 0 6px;
    border-radius: 8px;
    border: 1px solid var(--border);
  }
  .grow {
    flex: 1;
  }
  .filter {
    width: 200px;
    font-size: 12px;
  }
  .opt {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 11px;
    cursor: pointer;
    white-space: nowrap;
  }
  .dim {
    opacity: 0.65;
  }
  .mono {
    font-family: var(--mono);
  }
  .err,
  .note,
  .pad {
    flex: none;
    padding: 6px 10px;
    font-size: 12px;
  }
  .err {
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 12%, transparent);
  }
  .head,
  .row {
    display: grid;
    grid-template-columns: minmax(220px, 34%) 1fr 1fr;
  }
  .head {
    flex: none;
    font-size: 12px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .head > span {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .head img {
    height: 48px;
    width: 48px;
    object-fit: contain;
    border-radius: 3px;
  }
  .scroll {
    flex: 1;
    overflow: auto;
  }
  .rows {
    position: relative;
  }
  .row {
    position: absolute;
    left: 0;
    right: 0;
    height: 22px;
    line-height: 22px;
    font-family: var(--mono);
    font-size: 12px;
    border-left: 3px solid transparent;
    cursor: default;
  }
  .row.sel {
    outline: 1px solid color-mix(in srgb, var(--accent, #5a96e6) 70%, transparent);
    outline-offset: -1px;
  }
  .row > span {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
  .val {
    padding: 0 8px;
    border-left: 1px solid var(--border);
  }
  .muted {
    opacity: 0.5;
  }
  .tw {
    display: inline-block;
    width: 14px;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    font-size: 10px;
    cursor: pointer;
  }
  .mk {
    display: inline-block;
    width: 12px;
    font-weight: 700;
  }
  .s-changed {
    background: rgba(90, 150, 230, 0.1);
    border-left-color: #5a96e6;
  }
  .s-changed .mk {
    color: #7aaef0;
  }
  .s-changed:not(:has(.tw:is(button))) .l {
    background: rgba(217, 135, 58, 0.16);
  }
  .s-changed:not(:has(.tw:is(button))) .r {
    background: rgba(108, 195, 108, 0.17);
  }
  .s-added {
    background: rgba(108, 195, 108, 0.12);
    border-left-color: #5faf5f;
  }
  .s-added .mk {
    color: #7cc47c;
  }
  .s-removed {
    background: rgba(217, 135, 58, 0.12);
    border-left-color: #d9873a;
  }
  .s-removed .mk {
    color: #d9873a;
  }
</style>
