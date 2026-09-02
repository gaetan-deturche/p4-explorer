<script lang="ts">
  let {
    title,
    items,
    busyFile,
    onFixFile,
    onResolveFile,
    onIgnoreFile,
    onRetryAll,
    onForceAll,
    onClose,
  }: {
    title: string;
    // `why` is p4's refusal translated into the state behind it (see
    // p4_sync_blockers); it arrives a moment after the dialog opens.
    items: {
      line: string;
      file: string | null;
      why?: string;
      kind?: string;
      /** p4 synced this one and scheduled a resolve on it — not a failure. */
      resolve?: boolean;
      /** Settled since this report opened. */
      done?: boolean;
    }[];
    busyFile: string | null;
    onFixFile: (file: string, force: boolean) => void;
    onResolveFile: (file: string) => void; // opens the three-way resolve
    onIgnoreFile: (file: string) => void; // drop the line, leave the file alone
    onRetryAll: () => void;
    onForceAll: () => void;
    onClose: () => void;
  } = $props();

  type Cat = "locked" | "clobber" | "resolve" | "protected" | "other";
  function categorize(it: { line: string; resolve?: boolean }): Cat {
    if (it.resolve) return "resolve"; // stated by the producer, not guessed at
    const l = it.line.toLowerCase();
    if (l.includes("offline changes")) return "protected"; // kept by the retry guard
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
  /** The two kinds of entry read differently and are acted on differently: one
   *  is a file p4 would not write, the other a file it wrote and left a decision
   *  on. Retry and Force apply only to the first. */
  const open = $derived(items.filter((i) => !i.done));
  const failures = $derived(open.filter((i) => !i.resolve).length);
  const unsettled = $derived(open.length - failures);
  const fixed = $derived(items.length - open.length);
  const plural = (n: number) => (n === 1 ? "" : "s");
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <!-- Nothing left to act on: a warning colour would be reporting a problem
         that no longer exists, and the hint below has nothing to explain. -->
    {#if !failures && !unsettled}
      <div class="dtitle ok">✓ {title}: all sorted ({fixed} file{plural(fixed)})</div>
    {:else}
      <div class="dtitle">
        ⚠ {title}:
        {#if failures}{failures} file{plural(failures)} could not be synced{/if}{#if failures && unsettled}
          ·
        {/if}{#if unsettled}{unsettled} file{plural(unsettled)} need{unsettled === 1 ? "s" : ""} resolving{/if}{#if fixed}
          · <span class="fixedcount">{fixed} sorted</span>
        {/if}
      </div>
      <div class="hint">
        {#if failures}
          <b>Retry</b> re-syncs (safe — for files that were open in another app; close it first).
          <b>Force</b> overwrites writable files with the depot version (discards local changes).
        {/if}
        {#if unsettled}
          <!-- Not a failure: these synced. p4 will refuse the submit until each is
               settled, which is the only other place it would have been said. -->
          These synced onto work you have open, so p4 scheduled a merge.
          <b>Resolve</b> opens the three-way merge; nothing can be submitted until it is settled.
        {/if}
      </div>
    {/if}

    <div class="scroll">
      <div class="grid">
        <div class="hdr">File</div>
        <div class="hdr">Error</div>
        {#each items as it (it.line)}
          {@const cat = categorize(it)}
          <div class="fcell mono" class:done={it.done} title={it.file ?? it.line}>
            {it.file ?? "(unknown)"}
          </div>
          <div class="ecell" class:done={it.done}>
            <span class="etext mono">{errText(it)}</span>
            {#if it.why}
              <!-- "Can't clobber writable file" does not say whether the local
                   copy is precious, untracked, or merely writable. This does. -->
              <span class="why">{it.why}</span>
            {/if}
            <span class="act">
              {#if it.done}
                <span class="fixed">✓ {it.resolve ? "resolved" : "synced"}</span>
              {:else if !it.file}
                <span class="dim">—</span>
              {:else if cat === "resolve"}
                <button
                  class="primary"
                  disabled={busy}
                  title="Settle the conflict three-way (base / depot / workspace)"
                  onclick={() => onResolveFile(it.file!)}
                >
                  Resolve…
                </button>
              {:else if cat === "protected"}
                <button
                  class="danger-btn"
                  disabled={busy}
                  title="Overwrite with the depot version — the offline changes are DISCARDED"
                  onclick={() => onFixFile(it.file!, true)}
                >
                  {busyFile === it.file ? "…" : "Overwrite"}
                </button>
                <button
                  disabled={busy}
                  title="Keep the file as it is and dismiss this entry (the file stays un-synced)"
                  onclick={() => onIgnoreFile(it.file!)}
                >
                  Ignore
                </button>
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
      <!-- Only when there is something to re-sync: a resolve is settled one file
           at a time, and forcing one would discard the work it is there to keep. -->
      {#if failures}
        <button class="danger-btn" disabled={busy} onclick={onForceAll}>Force overwrite all</button>
        <button class="primary" disabled={busy} onclick={onRetryAll}>
          {busyFile === "*" ? "Working…" : "Retry all"}
        </button>
      {/if}
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
  .dtitle.ok {
    color: #7cc47c;
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
  /* Settled: still listed, so the report stays a record of what the sync ran
     into — struck through and green so what is LEFT is what stands out. */
  .fcell.done,
  .ecell.done {
    background: rgba(124, 196, 124, 0.1);
  }
  .fcell.done {
    color: var(--text-dim, #999);
  }
  .ecell.done .etext {
    color: #7cc47c;
    text-decoration: line-through;
    opacity: 0.75;
  }
  .fixed {
    color: #7cc47c;
    font-size: 11px;
    white-space: nowrap;
    padding: 2px 4px;
  }
  .fixedcount {
    color: #7cc47c;
  }
  .why {
    display: block;
    margin: 2px 0 4px;
    font-size: 11px;
    line-height: 1.45;
    color: var(--text-dim);
    max-width: 62ch;
  }
  .etext {
    font-size: 11px;
    color: var(--warn);
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .act {
    flex: none;
    display: flex;
    gap: 4px;
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
