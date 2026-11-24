<script lang="ts">
  let {
    title,
    count,
    current,
    issues,
    issueLine,
    phase,
    message,
    totalFiles = 0,
    written = 0,
    totalBytes = 0,
    rate = 0,
    idleSecs = 0,
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
    totalFiles?: number;
    written?: number;
    totalBytes?: number;
    rate?: number;
    idleSecs?: number;
    onCancel: () => void;
    onClose: () => void;
  } = $props();

  function size(n: number): string {
    if (n >= 1e9) return `${(n / 1e9).toFixed(2)} GB`;
    if (n >= 1e6) return `${(n / 1e6).toFixed(1)} MB`;
    if (n >= 1e3) return `${(n / 1e3).toFixed(0)} KB`;
    return `${n} B`;
  }
  // Text files grow on disk (CRLF), so the bytes written can pass the depot total.
  let pct = $derived(totalBytes > 0 ? Math.min(100, (written / totalBytes) * 100) : 0);
  // A parallel sync lists every file before it transfers any.
  let listed = $derived(totalFiles > 0 && count >= totalFiles);
  let eta = $derived.by(() => {
    if (!totalBytes || rate <= 0 || written >= totalBytes) return "";
    const s = (totalBytes - written) / rate;
    if (s < 60) return `${Math.ceil(s)} s left`;
    if (s < 3600) return `${Math.ceil(s / 60)} min left`;
    return `${(s / 3600).toFixed(1)} h left`;
  });
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
        {:else if listed}
          <span class="cnt mono">{count} files listed — downloading</span>
        {:else if totalFiles > 0}
          <span class="cnt mono">{count} of {totalFiles} files</span>
        {:else}
          <span class="cnt mono">{count} file{count === 1 ? "" : "s"} synced</span>
        {/if}
      </div>
      {#if totalBytes > 0}
        <div class="bar"><div class="fill" style:width="{pct}%"></div></div>
        <div class="cur mono dim">
          {size(Math.min(written, totalBytes))} of {size(totalBytes)} · {size(rate)}/s{eta ? ` · ${eta}` : ""}
        </div>
      {/if}
      {#if idleSecs >= 15}
        <div class="warnhead mono">
          Nothing written for {idleSecs} s — p4 is waiting on the server or on a file another program holds.
        </div>
      {/if}
      {#if current && !listed}<div class="cur mono dim" title={current}>{current}</div>{/if}
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
  .bar {
    height: 6px;
    border-radius: 3px;
    background: var(--border);
    overflow: hidden;
  }
  .fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.4s linear;
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
