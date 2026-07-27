<script lang="ts">
  import type { NoteEntry } from "$lib/notifications.svelte";

  let { entries, onClear }: { entries: NoteEntry[]; onClear: () => void } = $props();

  // Auto-scroll to the newest entry unless the user has scrolled up.
  let body: HTMLDivElement | undefined = $state();
  let stick = $state(true);
  function onScroll() {
    if (!body) return;
    stick = body.scrollTop + body.clientHeight >= body.scrollHeight - 20;
  }
  $effect(() => {
    entries.length; // track
    if (stick && body) body.scrollTop = body.scrollHeight;
  });
</script>

<div class="panel">
  <div class="hdr">
    <span class="dim">{entries.length} notification{entries.length === 1 ? "" : "s"}</span>
    <button onclick={onClear} disabled={entries.length === 0}>Clear</button>
  </div>
  <div class="scroll body" bind:this={body} onscroll={onScroll}>
    {#if entries.length === 0}
      <div class="msg dim">No notifications yet.</div>
    {:else}
      {#each entries as e (e.n)}
        <div class="row" class:err={e.type === "error"}>
          <span class="time dim">{e.time}</span>
          <span class="dot" title={e.type}>{e.type === "error" ? "✕" : "●"}</span>
          <span class="msg-txt">{e.message}</span>
        </div>
      {/each}
    {/if}
  </div>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
  }
  .hdr {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 10px;
    border-bottom: 1px solid var(--border);
    font-size: 11px;
  }
  .hdr button {
    font-size: 11px;
    padding: 2px 8px;
  }
  .body {
    flex: 1;
    padding: 4px 0;
    overflow-y: auto;
  }
  .row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 2px 10px;
    font-size: 12px;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .time {
    flex: none;
    font-size: 11px;
  }
  .dot {
    flex: none;
    color: var(--have);
    font-size: 10px;
  }
  .row.err .dot {
    color: var(--warn);
  }
  .msg-txt {
    flex: 1;
    min-width: 0;
    white-space: pre-wrap;
    word-break: break-word;
  }
  .row.err .msg-txt {
    color: var(--warn);
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
</style>
