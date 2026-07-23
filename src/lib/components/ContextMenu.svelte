<script lang="ts">
  export type MenuItem = {
    label: string;
    action?: () => void;
    disabled?: boolean;
    submenu?: MenuItem[];
  };

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

  let openSub = $state<number | null>(null);
  // Open submenus to the left when the menu is near the right edge.
  const flipLeft = $derived(typeof window !== "undefined" && x > window.innerWidth * 0.6);

  function run(it: MenuItem) {
    if (it.disabled || !it.action) return;
    // Invoke BEFORE closing: some actions read a value lazily from the context
    // state (e.g. `() => update(change)` where change comes from the {#if}
    // block); closing first tears that down and the action would throw.
    it.action();
    onClose();
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
  {#each items as it, i (it.label)}
    {#if it.submenu}
      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="mi" onpointerenter={() => (openSub = i)} onpointerleave={() => (openSub = null)}>
        <button class="item sub" disabled={it.disabled}>
          <span>{it.label}</span><span class="chev">▸</span>
        </button>
        {#if openSub === i}
          <div class="submenu" class:left={flipLeft}>
            {#each it.submenu as s (s.label)}
              <button class="item" disabled={s.disabled} onclick={() => run(s)}>{s.label}</button>
            {/each}
          </div>
        {/if}
      </div>
    {:else}
      <button class="item" disabled={it.disabled} onclick={() => run(it)}>{it.label}</button>
    {/if}
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
  .mi {
    position: relative;
  }
  .item {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
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
  .chev {
    color: var(--text-dim);
    font-size: 10px;
  }
  .item:hover .chev {
    color: #fff;
  }
  .submenu {
    position: absolute;
    top: -4px;
    left: 100%;
    min-width: 12rem;
    max-height: 60vh;
    overflow-y: auto;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    padding: 4px 0;
  }
  .submenu.left {
    left: auto;
    right: 100%;
  }
</style>
