<script lang="ts">
  export type MenuItem = { label: string; action: () => void; disabled?: boolean };

  let {
    x,
    y,
    items,
    onClose,
  }: {
    x: number;
    y: number;
    items: MenuItem[];
    onClose: () => void;
  } = $props();

  function run(it: MenuItem) {
    if (it.disabled) return;
    onClose();
    it.action();
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onClose()} />

<button
  class="scrim"
  aria-label="Close menu"
  onclick={onClose}
  oncontextmenu={(e) => {
    e.preventDefault();
    onClose();
  }}
></button>

<div class="menu" style="left:{x}px; top:{y}px">
  {#each items as it (it.label)}
    <button class="item" disabled={it.disabled} onclick={() => run(it)}>{it.label}</button>
  {/each}
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    border: none;
    background: transparent;
    padding: 0;
  }
  .menu {
    position: fixed;
    z-index: 61;
    min-width: 13rem;
    max-width: 90vw;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    padding: 4px 0;
  }
  .item {
    display: block;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 0;
    padding: 5px 14px;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
  }
  .item:hover:not(:disabled) {
    background: var(--accent);
    color: #fff;
  }
  .item:disabled {
    color: var(--text-dim);
    opacity: 0.6;
  }
</style>
