<script lang="ts">
  import { untrack } from "svelte";
  import { shortcuts } from "$lib/shortcuts.svelte";
  export type MenuItem = {
    label: string;
    action?: () => void;
    disabled?: boolean;
    submenu?: MenuItem[];
    /** A divider between groups; needs no label. */
    sep?: boolean;
    /** Shortcut id: the key is read from the registry, so what is advertised
     *  here is always what actually fires. */
    accel?: string;
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

  // --- staying inside the window ---------------------------------------------
  // The cursor point is where the menu WANTS to be, not where it can be: opened
  // on a row near the bottom, a menu placed there simply ran off the window and
  // its last items were unreachable. The size is only known once it is in the DOM,
  // so it is measured and then moved.
  //
  // Order matters: flip to the other side of the cursor first (a menu above the
  // pointer is normal and keeps the pointer out of the way), and only clamp if it
  // does not fit on either side.
  let menuEl = $state<HTMLDivElement>();
  /** Only set when the menu genuinely cannot fit: scrolling and submenus are
   *  mutually exclusive. A submenu sits at `left: 100%`, and CSS refuses to keep
   *  one axis visible while the other scrolls — so `overflow-y: auto` turned the
   *  menu into a scroller on BOTH axes, clipping every submenu and putting bars
   *  around a menu that had room to spare. */
  let scrolls = $state(false);
  // The cursor point is the right FIRST guess (it is correct whenever the menu
  // fits), and untracked because this is an initial value, not a binding — the
  // effect below owns it from then on.
  let at = $state(untrack(() => ({ left: x, top: y })));
  const EDGE = 6; // breathing room, so it never touches the frame

  $effect(() => {
    if (!menuEl) return;
    void x;
    void y;
    void openSub; // a submenu can change the height
    const r = menuEl.getBoundingClientRect();
    const maxW = window.innerWidth - EDGE;
    const maxH = window.innerHeight - EDGE;
    // scrollHeight is the natural height even once the cap is on, so this does
    // not latch: a menu that stops being too tall stops scrolling.
    const tooTall = menuEl.scrollHeight > window.innerHeight - 2 * EDGE;
    if (tooTall !== scrolls) scrolls = tooTall;
    let left = x;
    let top = y;
    if (left + r.width > maxW) left = Math.max(EDGE, x - r.width);
    if (top + r.height > maxH) {
      top = y - r.height >= EDGE ? y - r.height : Math.max(EDGE, maxH - r.height);
    }
    if (left !== at.left || top !== at.top) at = { left, top };
  });

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

<div
  class="menu"
  class:scrolls
  bind:this={menuEl}
  style="left:{at.left}px; top:{at.top}px"
>
  {#each items as it, i (it.label + i)}
    {#if it.sep}
      <div class="sep" role="separator"></div>
    {:else if it.submenu}
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
      <button class="item" disabled={it.disabled} onclick={() => run(it)}>
        <span class="ilabel">{it.label}</span>
        {#if it.accel && shortcuts.accel(it.accel)}
          <span class="accel mono">{shortcuts.accel(it.accel)}</span>
        {/if}
      </button>
    {/if}
  {/each}
</div>

<style>
  /* Right-aligned and dimmed, like the menu bar's. */
  .ilabel {
    flex: 1;
    text-align: left;
  }
  .accel {
    opacity: 0.55;
    font-size: 11px;
    white-space: nowrap;
    padding-left: 10px;
  }
  .mono {
    font-family: var(--mono);
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    border: none;
    background: transparent;
    padding: 0;
  }
  .sep {
    height: 1px;
    margin: 4px 6px;
    background: var(--border, #333);
  }
  .menu {
    position: fixed;
    z-index: 61;
    min-width: 13rem;
    max-width: 90vw;
    /* No overflow here by default — see `scrolls`. */
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.4);
    padding: 4px 0;
  }
  /* The last resort for a menu longer than the window. Submenus are clipped in
     this state, which is why it is not the default. */
  .menu.scrolls {
    max-height: calc(100vh - 12px);
    overflow-y: auto;
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
