<script lang="ts">
  import type { P4Conn } from "$lib/p4";

  let {
    conn = $bindable(),
    busy,
    servers,
    onConnect,
    onSelectServer,
    onRelogin,
    onForget,
    onAdd,
    onClose,
  }: {
    conn: P4Conn;
    busy: boolean;
    servers: string[];
    onConnect: () => void;
    onSelectServer: (port: string) => void;
    onRelogin: (port: string) => void;
    onForget: (port: string) => void;
    onAdd: (port: string) => void;
    onClose: () => void;
  } = $props();

  let newServer = $state("");
  function add() {
    const v = newServer.trim();
    if (!v) return;
    newServer = "";
    onAdd(v);
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<div class="overlay">
  <button class="backdrop" aria-label="Close options" onclick={onClose}></button>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle">Connection options</div>

    <div class="section">Servers</div>
    {#if servers.length}
      <div class="srvlist">
        {#each servers as s (s)}
          <div class="srow" class:current={s === conn.port}>
            <span class="sname mono" title={s}>{s}</span>
            <div class="sbtns">
              <button onclick={() => onSelectServer(s)} disabled={busy || s === conn.port}>
                Connect
              </button>
              <button onclick={() => onRelogin(s)} disabled={busy} title="Log in again to this server">
                Re-login
              </button>
              <button class="danger" onclick={() => onForget(s)} title="Remove from the list">
                Forget
              </button>
            </div>
          </div>
        {/each}
      </div>
    {:else}
      <p class="hint dim">No servers remembered yet.</p>
    {/if}
    <div class="addrow">
      <input
        class="mono"
        bind:value={newServer}
        placeholder="ssl:host:1666"
        onkeydown={(e) => e.key === "Enter" && add()}
      />
      <button onclick={add} disabled={!newServer.trim()}>Add</button>
    </div>

    <div class="section">Current connection</div>
    <p class="hint dim">Leave blank to use the ambient p4 environment (P4PORT / P4USER / ticket).</p>
    <label>
      <span>Server (P4PORT)</span>
      <input class="mono" bind:value={conn.port} placeholder="ssl:host:1666" />
    </label>
    <label>
      <span>User (P4USER)</span>
      <input class="mono" bind:value={conn.user} placeholder="username" />
    </label>
    <div class="actions">
      <button onclick={onClose}>Close</button>
      <button class="primary" onclick={onConnect} disabled={busy}>
        {busy ? "Connecting…" : "Connect"}
      </button>
    </div>
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 50;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    border: none;
    border-radius: 0;
    padding: 0;
    background: rgba(0, 0, 0, 0.4);
    cursor: default;
  }
  .dialog {
    position: relative;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.4);
    padding: 16px 18px;
    width: 30rem;
    max-width: 92vw;
    max-height: 88vh;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .section {
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    border-bottom: 1px solid var(--border);
    padding-bottom: 3px;
    margin-top: 4px;
  }
  .hint {
    margin: 0;
    font-size: 11px;
  }
  .srvlist {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  .srow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 6px;
    border-radius: 5px;
  }
  .srow.current {
    background: var(--bg-sel);
  }
  .sname {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }
  .sbtns {
    display: flex;
    gap: 4px;
    flex: none;
  }
  .sbtns button {
    font-size: 11px;
    padding: 2px 8px;
  }
  .danger:not(:disabled) {
    color: var(--warn);
  }
  .addrow {
    display: flex;
    gap: 6px;
  }
  .addrow input {
    flex: 1;
    min-width: 0;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 12px;
    color: var(--text-dim);
  }
  label input {
    width: 100%;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .primary:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
