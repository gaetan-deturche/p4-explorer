<script lang="ts">
  import type { P4Conn, P4Record } from "$lib/p4";

  let {
    conn = $bindable(),
    clients,
    connected,
    refreshing,
    syncing,
    onClientChange,
    onRefresh,
    onSync,
  }: {
    conn: P4Conn;
    clients: P4Record[];
    connected: boolean;
    refreshing: boolean;
    syncing: boolean;
    onClientChange: () => void;
    onRefresh: () => void;
    onSync: () => void;
  } = $props();

  const busy = $derived(!connected || refreshing || syncing);
</script>

<div class="toolbar">
  <label class="ws">
    Workspace
    <select class="mono" bind:value={conn.client} onchange={onClientChange} disabled={!connected}>
      <option value="">— select —</option>
      {#each clients as c (c.client)}
        <option value={c.client}>{c.client}{c.Stream ? "  ·  " + c.Stream : ""}</option>
      {/each}
    </select>
  </label>
  <div class="btns">
    <button onclick={onRefresh} disabled={busy} title="Re-fetch the current view">
      ↻ {refreshing ? "Refreshing…" : "Refresh"}
    </button>
    <button class="sync" onclick={onSync} disabled={busy} title="Sync the entire workspace to latest">
      ⤓ {syncing ? "Syncing…" : "Global sync"}
    </button>
  </div>
</div>

<style>
  .toolbar {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .ws {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .ws select {
    min-width: 18rem;
  }
  .btns {
    display: flex;
    gap: 8px;
  }
  .btns button {
    font-size: 12px;
  }
  .sync:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
