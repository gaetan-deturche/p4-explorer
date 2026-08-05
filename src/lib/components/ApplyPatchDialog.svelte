<script lang="ts">
  import type { PatchFileReport } from "$lib/p4";

  let {
    path,
    phase,
    files,
    busy,
    subject = "",
    skipped = [],
    onApply,
    onResolveHunk,
    onClose,
  }: {
    path: string;
    phase: "preview" | "applying" | "done";
    files: PatchFileReport[];
    busy: boolean;
    /** Names the source when it isn't a file the user picked (e.g. a review). */
    subject?: string;
    /** Files the source carried that a patch cannot express (binaries, adds). */
    skipped?: string[];
    onApply: (mode: "edit" | "offline", partial: boolean) => void;
    onResolveHunk: (depot: string, hunkIndex: number) => void;
    onClose: () => void;
  } = $props();

  let mode = $state<"edit" | "offline">("offline");
  let partial = $state(false);

  const name = $derived(path.split(/[\\/]/).pop() || path);
  const conflicted = $derived(files.filter((f) => f.conflicts > 0).length);
  const blocked = $derived(files.filter((f) => f.status === "missing" || f.status === "notext").length);
  // What Apply would actually touch, given the current partial setting.
  const willTouch = $derived(
    files.filter((f) => f.applied > 0 && (partial || f.conflicts === 0)).length,
  );

  const LABEL: Record<PatchFileReport["status"], string> = {
    clean: "applies",
    fuzz: "applies (shifted)",
    already: "already applied",
    partial: "partly conflicts",
    conflict: "conflicts",
    missing: "not found",
    notext: "not text",
    binary: "binary (replaced whole)",
  };
  function tone(s: PatchFileReport["status"]): string {
    if (s === "clean" || s === "binary") return "ok";
    if (s === "fuzz" || s === "already") return "warnish";
    return "bad";
  }
  function hunkSummary(f: PatchFileReport): string {
    if (f.status === "binary") return ""; // the message carries the size instead
    const n = f.hunks.length;
    if (phase === "done") return `${f.applied}/${n} hunk${n === 1 ? "" : "s"} applied`;
    const fuzz = f.hunks.filter((h) => h.status === "fuzz").length;
    const parts = [`${n} hunk${n === 1 ? "" : "s"}`];
    if (f.conflicts) parts.push(`${f.conflicts} conflicting`);
    if (fuzz) parts.push(`${fuzz} shifted`);
    return parts.join(", ");
  }
  /** Where a shifted hunk actually landed — the useful detail for review. */
  function offsets(f: PatchFileReport): string {
    return f.hunks
      .filter((h) => h.status === "fuzz")
      .map((h) => `#${h.index} → line ${h.line}${h.offset ? ` (${h.offset > 0 ? "+" : ""}${h.offset})` : ""}`)
      .join(", ");
  }
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
    <div class="dtitle">
      {phase === "done" ? "Applied" : "Apply"} — <span class="mono">{name}</span>
    </div>

    {#if skipped.length}
      <div class="hint warnish">
        {skipped.length} file{skipped.length === 1 ? "" : "s"} carry no diff (binary, or added) and
        {skipped.length === 1 ? "is" : "are"} copied from the shelf as-is:
        <span class="mono">{skipped.map((s) => s.split("/").pop()).join(", ")}</span>
      </div>
    {/if}

    {#if phase !== "done"}
      <div class="hint">
        Dry run against this workspace — nothing has been written yet. Hunks are matched by
        context, so a shifted file still takes the patch; anything that can't be placed is
        reported as a conflict.
      </div>
    {/if}

    <div class="scroll">
      <div class="grid">
        <div class="hdr">File</div>
        <div class="hdr">Result</div>
        {#each files as f (f.depot + f.local)}
          <div class="fcell mono" title={f.local || f.depot}>
            {f.depot}
            {#if f.local}<div class="dim sub">{f.local}</div>{/if}
          </div>
          <div class="rcell">
            <div class="rline">
              <span class="pill {tone(f.status)}">{LABEL[f.status]}</span>
              <span class="dim">{hunkSummary(f)}</span>
            </div>
            {#if offsets(f)}<div class="dim sub">{offsets(f)}</div>{/if}
            {#if f.message}<div class="msg">{f.message}</div>{/if}
            {#if f.conflicts > 0}
              <div class="acts">
                {#each f.hunks.filter((hk) => hk.status === "conflict") as hk (hk.index)}
                  <button
                    title="Settle hunk #{hk.index} three-way (patch expects / patch / workspace)"
                    onclick={() => onResolveHunk(f.depot, hk.index)}
                  >
                    Resolve #{hk.index}…
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    </div>

    {#if phase === "done"}
      <div class="actions"><button class="primary" onclick={onClose}>Close</button></div>
    {:else}
      <div class="opts">
        <div class="orow">
          <span class="olabel">Leave the patched files:</span>
          <label>
            <input type="radio" value="offline" bind:group={mode} />
            offline (write to disk only)
          </label>
          <label>
            <input type="radio" value="edit" bind:group={mode} />
            opened for edit in the default changelist
          </label>
        </div>
        {#if conflicted > 0}
          <label class="orow warnish">
            <input type="checkbox" bind:checked={partial} />
            Apply the hunks that fit in the {conflicted} conflicting file{conflicted === 1
              ? ""
              : "s"} and save the rest as <span class="mono">.rej</span>
          </label>
        {/if}
      </div>

      <div class="foot">
        <span class="dim">
          {willTouch} file{willTouch === 1 ? "" : "s"} will be written{blocked
            ? `, ${blocked} skipped`
            : ""}{conflicted && !partial
            ? `, ${conflicted} left untouched (conflicts)`
            : ""}.
        </span>
        <span class="actions">
          <button onclick={onClose} disabled={busy}>Cancel</button>
          <button class="primary" disabled={busy || willTouch === 0} onclick={() => onApply(mode, partial)}>
            {phase === "applying" ? "Applying…" : "Apply"}
          </button>
        </span>
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
    z-index: 86;
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
    width: 54rem;
    max-width: 95vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .hint {
    font-size: 12px;
    color: var(--text-dim);
  }
  .scroll {
    max-height: 46vh;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .grid {
    display: grid;
    grid-template-columns: minmax(0, 1.2fr) minmax(0, 1fr);
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
    border-bottom: 1px solid var(--border);
    border-right: 1px solid var(--border);
    overflow-wrap: anywhere;
    min-width: 0;
  }
  .rcell {
    padding: 6px 8px;
    font-size: 11px;
    border-bottom: 1px solid var(--border);
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .rline {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  .sub {
    font-size: 10px;
    overflow-wrap: anywhere;
  }
  .msg {
    font-size: 10px;
    color: var(--warn);
    overflow-wrap: anywhere;
  }
  .acts {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
    margin-top: 2px;
  }
  .pill {
    border: 1px solid var(--border);
    border-radius: 999px;
    padding: 1px 7px;
    font-size: 10px;
    white-space: nowrap;
  }
  .pill.ok {
    color: var(--ok, #6cc36c);
    border-color: currentColor;
  }
  .pill.warnish {
    color: var(--warn);
    border-color: currentColor;
  }
  .pill.bad {
    color: var(--danger, #d76a6a);
    border-color: currentColor;
  }
  .opts {
    display: flex;
    flex-direction: column;
    gap: 6px;
    font-size: 12px;
  }
  .orow {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }
  .orow label {
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .olabel {
    color: var(--text-dim);
  }
  .warnish {
    color: var(--warn);
  }
  .foot {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    flex-wrap: wrap;
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
</style>
