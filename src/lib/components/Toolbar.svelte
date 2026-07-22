<script lang="ts">
  import type { P4Conn, P4Record } from "$lib/p4";

  let {
    conn = $bindable(),
    clients,
    servers,
    connected,
    refreshing,
    syncing,
    reconciling,
    onClientChange,
    onServerChange,
    onAddServer,
    onServerContext,
    onRefresh,
    onSync,
    onReconcile,
  }: {
    conn: P4Conn;
    clients: P4Record[];
    servers: string[];
    connected: boolean;
    refreshing: boolean;
    syncing: boolean;
    reconciling: boolean;
    onClientChange: () => void;
    onServerChange: (port: string) => void;
    onAddServer: () => void;
    onServerContext: (e: MouseEvent) => void;
    onRefresh: () => void;
    onSync: () => void;
    onReconcile: () => void;
  } = $props();

  const busy = $derived(!connected || refreshing || syncing || reconciling);

  const ADD = "__add__";
  function onServerPick(e: Event) {
    const sel = e.currentTarget as HTMLSelectElement;
    const v = sel.value;
    if (v === ADD) {
      sel.value = conn.port; // don't leave "Add…" selected
      onAddServer();
      return;
    }
    if (v !== conn.port) onServerChange(v);
  }
</script>

<div class="toolbar">
  <div class="row">
    <label class="srv">
      Server
      <select
        class="mono"
        value={conn.port}
        onchange={onServerPick}
        oncontextmenu={(e) => {
          e.preventDefault();
          onServerContext(e);
        }}
        disabled={syncing}
        title={conn.port ? "Right-click to forget this server" : ""}
      >
        {#if conn.port && !servers.includes(conn.port)}
          <option value={conn.port}>{conn.port}</option>
        {/if}
        {#if !conn.port}
          <option value="">— ambient default —</option>
        {/if}
        {#each servers as s (s)}
          <option value={s}>{s}</option>
        {/each}
        <option value={ADD}>＋ Add server…</option>
      </select>
    </label>

    <label class="ws">
      Workspace
      <select class="mono" bind:value={conn.client} onchange={onClientChange} disabled={!connected}>
        <option value="">— select —</option>
        {#each clients as c (c.client)}
          <option value={c.client}>
            {c.client}{c.Root ? "  —  " + c.Root : ""}{c.Stream ? "  ·  " + c.Stream : ""}
          </option>
        {/each}
      </select>
    </label>
  </div>

  <div class="btns">
    <button onclick={onRefresh} disabled={busy} title="Re-fetch the current view">
      ↻ {refreshing ? "Refreshing…" : "Refresh"}
    </button>
    <button class="sync" onclick={onSync} disabled={busy} title="Sync the entire workspace to latest">
      ⤓ {syncing ? "Syncing…" : "Global sync"}
    </button>
    <button
      onclick={onReconcile}
      disabled={busy || !conn.client}
      title="Open files changed, added, or deleted outside Perforce (offline work)"
    >
      ⟲ {reconciling ? "Reconciling…" : "Reconcile"}
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
  .row {
    display: flex;
    align-items: center;
    gap: 14px;
    flex-wrap: wrap;
  }
  .srv,
  .ws {
    display: flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    font-size: 12px;
  }
  .srv select {
    min-width: 16rem;
  }
  .ws select {
    min-width: 18rem;
    max-width: 40rem;
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
