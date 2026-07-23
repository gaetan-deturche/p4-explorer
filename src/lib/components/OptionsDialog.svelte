<script lang="ts">
  import type { P4Conn } from "$lib/p4";
  import { safe } from "$lib/safe.svelte";
  import { P4_COMMAND_LIST } from "$lib/p4cmds";

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

  let tab = $state<"servers" | "connection" | "safe">("servers");
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
    <div class="dtitle">Options</div>
    <div class="tabs">
      <button class:active={tab === "servers"} onclick={() => (tab = "servers")}>Servers</button>
      <button class:active={tab === "connection"} onclick={() => (tab = "connection")}>
        Connection
      </button>
      <button class:active={tab === "safe"} onclick={() => (tab = "safe")}>Safe</button>
    </div>

    {#if tab === "servers"}
      {#if servers.length}
        <div class="srvlist">
          {#each servers as s (s)}
            <div class="srow" class:current={s === conn.port}>
              <span class="sname mono" title={s}>{s}</span>
              <div class="sbtns">
                <button onclick={() => onSelectServer(s)} disabled={busy || s === conn.port}>
                  Connect
                </button>
                <button onclick={() => onRelogin(s)} disabled={busy} title="Log in again">
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
    {:else if tab === "connection"}
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
        <button class="primary" onclick={onConnect} disabled={busy}>
          {busy ? "Connecting…" : "Connect"}
        </button>
      </div>
    {:else}
      <label class="toggle">
        <input
          type="checkbox"
          checked={safe.enabled}
          onchange={(e) => safe.setEnabled(e.currentTarget.checked)}
        />
        Enable safe mode
      </label>
      <p class="hint dim">
        When on, any command that isn't allowed below must be approved before it runs. Tick a command
        to allow it without asking (reads are allowed by default); untick to require approval.
      </p>
      <div class="section">
        <span>Allowed commands</span>
        <button class="reset" onclick={() => safe.resetAllows()}>Reset to defaults</button>
      </div>
      <div class="allowlist">
        {#each P4_COMMAND_LIST as c (c.label)}
          <label class="arow" title={c.label}>
            <input
              type="checkbox"
              checked={safe.isAllowed(c.label, c.read)}
              onchange={(e) => safe.setAllowed(c.label, e.currentTarget.checked)}
            />
            <span class="mono">{c.label}</span>
            {#if c.read}<span class="tag dim">read</span>{/if}
          </label>
        {/each}
      </div>
    {/if}

    <div class="foot">
      <button onclick={onClose}>Close</button>
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
  .tabs {
    display: flex;
    gap: 2px;
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    padding: 4px 12px;
    color: var(--text-dim);
  }
  .tabs button.active {
    color: var(--text);
    border-bottom-color: var(--accent);
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
  .allowlist {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 1px 10px;
  }
  .srow,
  .arow {
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
  label.toggle {
    flex-direction: row;
    align-items: center;
    gap: 6px;
  }
  label input {
    width: 100%;
  }
  label.toggle input {
    width: auto;
  }
  .section {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    border-bottom: 1px solid var(--border);
    padding-bottom: 3px;
  }
  .reset {
    font-size: 10px;
    padding: 1px 6px;
    text-transform: none;
    letter-spacing: 0;
  }
  label.arow {
    flex-direction: row;
    align-items: center;
    gap: 6px;
  }
  label.arow input {
    width: auto;
  }
  .arow .mono {
    flex: 1;
    min-width: 0;
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tag {
    flex: none;
    font-size: 10px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    border-top: 1px solid var(--border);
    padding-top: 10px;
  }
  .primary:not(:disabled) {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
