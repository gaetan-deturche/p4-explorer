<script lang="ts">
  let {
    title,
    message,
    okLabel = "OK",
    onOk,
    onCancel,
  }: {
    title: string;
    message: string;
    okLabel?: string;
    onOk: () => void;
    onCancel: () => void;
  } = $props();
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape") onCancel();
    else if (e.key === "Enter") onOk();
  }}
/>

<div class="overlay">
  <button class="backdrop" aria-label="Cancel" onclick={onCancel}></button>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    {#if title}<div class="dtitle">{title}</div>{/if}
    <p class="msg">{message}</p>
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" onclick={onOk}>{okLabel}</button>
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
    z-index: 70;
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
    width: 28rem;
    max-width: 90vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .msg {
    margin: 0;
    font-size: 12px;
    line-height: 1.5;
    white-space: pre-wrap;
    color: var(--text);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
