<script lang="ts">
  //! The find bar the panes use instead of the webview's own.
  //!
  //! Ctrl+F is swallowed by each window and lands here: the webview's find only
  //! searches the DOM, and these panes render sixty rows out of twenty thousand.

  let {
    query = $bindable(""),
    caseSensitive = $bindable(false),
    /** Which match is selected, 0-based; -1 before the first step. */
    index,
    total,
    onStep,
    onClose,
  }: {
    query: string;
    caseSensitive: boolean;
    index: number;
    total: number;
    onStep: (delta: number) => void;
    onClose: () => void;
  } = $props();

  let input: HTMLInputElement | undefined = $state();
  // Focus and select on open, so a second Ctrl+F replaces the previous query
  // rather than appending to it.
  export function focus() {
    input?.focus();
    input?.select();
  }
  $effect(() => {
    focus();
  });

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      onClose();
    } else if (e.key === "Enter") {
      e.preventDefault();
      onStep(e.shiftKey ? -1 : 1);
    }
  }
</script>

<div class="find">
  <input
    bind:this={input}
    bind:value={query}
    onkeydown={onKey}
    placeholder="Find"
    spellcheck="false"
    aria-label="Find in this file"
  />
  <span class="count" class:none={!!query && total === 0}>
    {#if !query}
      &nbsp;
    {:else if total === 0}
      no matches
    {:else}
      {index + 1} / {total}
    {/if}
  </span>
  <button
    class:on={caseSensitive}
    title="Match case"
    aria-pressed={caseSensitive}
    onclick={() => (caseSensitive = !caseSensitive)}>Aa</button
  >
  <button disabled={total === 0} title="Previous match (shift+enter)" onclick={() => onStep(-1)}>
    ▲
  </button>
  <button disabled={total === 0} title="Next match (enter)" onclick={() => onStep(1)}>▼</button>
  <button title="Close (esc)" onclick={onClose}>✕</button>
</div>

<style>
  .find {
    flex: none;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border, #333);
    background: var(--bg-alt, #202020);
    font-size: 12px;
  }
  input {
    font: inherit;
    color: var(--text, #ddd);
    background: var(--bg-panel, #191919);
    border: 1px solid var(--border, #333);
    border-radius: 4px;
    padding: 2px 6px;
    width: 18rem;
  }
  input:focus {
    outline: none;
    border-color: var(--accent, #6ea8fe);
  }
  .count {
    color: var(--text-dim, #999);
    min-width: 7ch;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }
  .count.none {
    color: var(--warn, #e0a33a);
  }
  button {
    font-size: 11px;
    padding: 1px 7px;
  }
  button.on {
    border-color: var(--accent, #6ea8fe);
    color: var(--accent, #6ea8fe);
  }
  button:disabled {
    opacity: 0.5;
  }
</style>
