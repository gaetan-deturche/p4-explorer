<script lang="ts">
  let {
    title,
    items,
    busyFile,
    onFixFile,
    onRetryAll,
    onForceAll,
    onClose,
  }: {
    title: string;
    items: { line: string; file: string | null }[];
    busyFile: string | null;
    onFixFile: (file: string, force: boolean) => void;
    onRetryAll: () => void;
    onForceAll: () => void;
    onClose: () => void;
  } = $props();

  type Cat = "locked" | "clobber" | "resolve" | "other";
  function categorize(line: string): Cat {
    const l = line.toLowerCase();
    if (
      l.includes("another process") ||
      l.includes("autre processus") ||
      l.includes("used by") ||
      l.startsWith("unlink")
    )
      return "locked";
    if (l.includes("clobber")) return "clobber";
    if (l.includes("resolve") || l.includes("résoud")) return "resolve";
    return "other";
  }
  // The error text with the file path stripped out (path is shown in its column).
  function errText(it: { line: string; file: string | null }): string {
    if (!it.file) return it.line;
    const m = it.line
      .split(it.file)
      .join(" ")
      .replace(/\s{2,}/g, " ")
      .replace(/^[-:>\s]+/, "")
      .trim();
    return m || it.line;
  }
  const busy = $derived(busyFile !== null);
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle">
      ⚠ {title}: {items.length} file{items.length === 1 ? "" : "s"} could not be synced
    </div>
    <div class="hint">
      <b>Retry</b> re-syncs (safe — for files that were open in another app; close it first).
      <b>Force</b> overwrites writable files with the depot version (discards local changes).
      Conflicts must be resolved in P4V.
    </div>

    <div class="scroll">
      <div class="grid">
        <div class="hdr">File</div>
        <div class="hdr">Error</div>
        {#each items as it (it.line)}
          {@const cat = categorize(it.line)}
          <div class="fcell mono" title={it.file ?? it.line}>{it.file ?? "(unknown)"}</div>
          <div class="ecell">
            <span class="etext mono">{errText(it)}</span>
            <span class="act">
              {#if !it.file}
                <span class="dim">—</span>
              {:else if cat === "resolve"}
                <button disabled title="Resolve in P4V / p4 resolve">Resolve</button>
              {:else if cat === "clobber"}
                <button class="danger-btn" disabled={busy} onclick={() => onFixFile(it.file!, true)}>
                  {busyFile === it.file ? "…" : "Force"}
                </button>
              {:else}
                <button class="primary" disabled={busy} onclick={() => onFixFile(it.file!, false)}>
                  {busyFile === it.file ? "…" : "Retry"}
                </button>
              {/if}
            </span>
          </div>
        {/each}
      </div>
    </div>

    <div class="actions">
      <button onclick={onClose}>Close</button>
      <button class="danger-btn" disabled={busy} onclick={onForceAll}>Force overwrite all</button>
      <button class="primary" disabled={busy} onclick={onRetryAll}>
        {busyFile === "*" ? "Working…" : "Retry all"}
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
    z-index: 85;
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
    width: 52rem;
    max-width: 95vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
    color: var(--warn);
  }
  .hint {
    font-size: 12px;
    color: var(--text-dim);
  }
  .scroll {
    max-height: 48vh;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    align-items: stretch;
  }
  .hdr {
    position: sticky;
    top: 0;
    background: var(--bg-alt);
    font-size: 11px;
    font-weight: 600;
    color: var(--text-dim);
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
  }
  .fcell {
    padding: 6px 8px;
    font-size: 11px;
    color: var(--text);
    border-bottom: 1px solid var(--border);
    border-right: 1px solid var(--border);
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .ecell {
    padding: 6px 8px;
    border-bottom: 1px solid var(--border);
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 10px;
    min-width: 0;
  }
  .etext {
    font-size: 11px;
    color: var(--warn);
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .act {
    flex: none;
  }
  .act button {
    font-size: 11px;
    padding: 2px 10px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
  .danger-btn {
    border-color: var(--warn);
    color: var(--warn);
  }
  button:disabled {
    opacity: 0.5;
  }
</style>
