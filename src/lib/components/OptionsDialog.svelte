<script lang="ts">
  import type { P4Conn } from "$lib/p4";

  let {
    conn = $bindable(),
    busy,
    onConnect,
    onClose,
  }: {
    conn: P4Conn;
    busy: boolean;
    onConnect: () => void;
    onClose: () => void;
  } = $props();
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<div class="overlay">
  <button class="backdrop" aria-label="Close options" onclick={onClose}></button>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle">Connection options</div>
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
    width: 26rem;
    max-width: 90vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .hint {
    margin: 0;
    font-size: 11px;
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
