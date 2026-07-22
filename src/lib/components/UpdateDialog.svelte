<script lang="ts">
  let {
    version,
    notes,
    phase,
    downloaded,
    total,
    message,
    onInstall,
    onDismiss,
  }: {
    version: string;
    notes: string;
    phase: "available" | "downloading" | "error";
    downloaded: number;
    total: number;
    message: string;
    onInstall: () => void;
    onDismiss: () => void;
  } = $props();

  const mb = (n: number) => (n / 1024 / 1024).toFixed(1);
  const pct = $derived(total > 0 ? Math.min(100, Math.round((downloaded / total) * 100)) : null);
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle">Update available — v{version}</div>

    {#if phase === "available"}
      {#if notes}<pre class="notes">{notes}</pre>{/if}
      <div class="actions">
        <button onclick={onDismiss}>Later</button>
        <button class="primary" onclick={onInstall}>Update &amp; restart</button>
      </div>
    {:else if phase === "downloading"}
      <div class="line"><span class="spin"></span><span>Downloading…</span></div>
      {#if pct !== null}
        <div class="bar"><div class="fill" style="width:{pct}%"></div></div>
        <div class="dim sz">{mb(downloaded)} / {mb(total)} MB</div>
      {:else}
        <div class="dim sz">{mb(downloaded)} MB</div>
      {/if}
    {:else}
      <div class="err mono">{message}</div>
      <div class="actions"><button class="primary" onclick={onDismiss}>Close</button></div>
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
    z-index: 90;
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
    width: 30rem;
    max-width: 92vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .notes {
    margin: 0;
    max-height: 40vh;
    overflow: auto;
    background: var(--bg-alt);
    border-radius: 5px;
    padding: 8px;
    font-family: var(--mono);
    font-size: 11px;
    white-space: pre-wrap;
    word-break: break-word;
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
  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--bg-alt);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.1s linear;
  }
  .sz {
    font-size: 11px;
  }
  .err {
    font-size: 12px;
    color: var(--warn);
    white-space: pre-wrap;
    word-break: break-word;
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
