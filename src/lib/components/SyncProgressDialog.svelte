<script lang="ts">
  let {
    title,
    count,
    current,
    issues,
    issueLine,
    phase,
    message,
    onCancel,
    onClose,
  }: {
    title: string;
    count: number;
    current: string;
    issues: number;
    issueLine: string;
    phase: "running" | "error";
    message: string;
    onCancel: () => void;
    onClose: () => void;
  } = $props();
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle">{title}</div>

    {#if phase === "error"}
      <div class="err mono">{message}</div>
      <div class="actions">
        <button class="primary" onclick={onClose}>Close</button>
      </div>
    {:else}
      <div class="line">
        <span class="spin"></span>
        {#if count === 0}
          <span class="cnt mono">Preparing… (scanning the workspace)</span>
        {:else}
          <span class="cnt mono">{count} file{count === 1 ? "" : "s"} synced</span>
        {/if}
      </div>
      {#if current}<div class="cur mono dim" title={current}>{current}</div>{/if}
      {#if issues > 0}
        <div class="warnhead mono">
          ⚠ {issues} file{issues === 1 ? "" : "s"} couldn't be synced (open in another app)
        </div>
        {#if issueLine}<div class="cur mono dim" title={issueLine}>{issueLine}</div>{/if}
      {/if}
      <div class="actions">
        <button onclick={onCancel}>Cancel</button>
      </div>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 80;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.4);
  }
  .dialog {
    position: relative;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.4);
    padding: 16px 18px;
    width: 32rem;
    max-width: 92vw;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .line {
    display: flex;
    align-items: center;
    gap: 10px;
    font-size: 12px;
  }
  .spin {
    width: 14px;
    height: 14px;
    border: 2px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.7s linear infinite;
    flex: none;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  .cnt {
    font-size: 12px;
  }
  .cur {
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .warnhead {
    font-size: 12px;
    color: var(--warn);
  }
  .err {
    font-size: 12px;
    color: var(--warn);
    white-space: pre-wrap;
    word-break: break-word;
    max-height: 40vh;
    overflow: auto;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 4px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
