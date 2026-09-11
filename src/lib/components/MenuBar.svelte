<script lang="ts">
  import type { Views } from "$lib/nav";
  import { shortcuts } from "$lib/shortcuts.svelte";

  let {
    connected,
    refreshing,
    syncing,
    views,
    onOptions,
    onReconnect,
    onExit,
    onRefresh,
    onSync,
    onApplyPatch,
    onNewWorkspace,
    workspaces,
    currentWorkspace,
    onOpenWorkspaceWindow,
    onToggleView,
    onAbout,
    onCheckUpdates,
  }: {
    connected: boolean;
    refreshing: boolean;
    syncing: boolean;
    views: Views;
    onOptions: () => void;
    onReconnect: () => void;
    onExit: () => void;
    onRefresh: () => void;
    onSync: () => void;
    onApplyPatch: () => void;
    onNewWorkspace: () => void;
    /** The workspaces on this server, for the "open in a new window" submenu. */
    workspaces: { client: string; root: string }[];
    currentWorkspace: string;
    onOpenWorkspaceWindow: (client: string) => void;
    onToggleView: (key: keyof Views) => void;
    onAbout: () => void;
    onCheckUpdates: () => void;
  } = $props();

  type Item = {
    label: string;
    action?: () => void;
    disabled?: boolean;
    sep?: boolean;
    checked?: boolean;
    /** Shortcut id: the key is read from the registry, so a rebinding shows here
     *  without this file knowing anything about which key it is. */
    accel?: string;
    /** A submenu, opened by hovering this item. */
    sub?: Item[];
    title?: string;
  };

  let open = $state<string | null>(null);
  let openSub = $state<number | null>(null);

  // A submenu runs downwards from its own row, so reaching any entry but the
  // first means crossing the rows below it. Leaving therefore only SCHEDULES the
  // close, and arriving anywhere inside the submenu cancels it; a deliberate
  // move onto another item still closes it, just a moment later.
  const GRACE_MS = 400;
  let closeTimer: number | null = null;
  function cancelClose() {
    if (closeTimer !== null) {
      clearTimeout(closeTimer);
      closeTimer = null;
    }
  }
  function openSubmenu(i: number) {
    cancelClose();
    openSub = i;
  }
  function scheduleClose() {
    cancelClose();
    closeTimer = window.setTimeout(() => {
      openSub = null;
      closeTimer = null;
    }, GRACE_MS);
  }
  function closeSubNow() {
    cancelClose();
    openSub = null;
  }
  $effect(() => cancelClose); // nothing pending once the bar is gone
  const busy = $derived(!connected || refreshing || syncing);

  const menus = $derived<{ name: string; items: Item[] }[]>([
    {
      name: "File",
      items: [
        { label: "Options…", action: onOptions, accel: "options" },
        { label: "Reconnect", action: onReconnect },
        { label: "", sep: true },
        { label: "Exit", action: onExit },
      ],
    },
    {
      name: "Workspace",
      items: [
        {
          label: "Open in a new window",
          disabled: !connected || !workspaces.length,
          // The workspace a window is already showing is not a destination:
          // asking for it again would just raise this window.
          sub: workspaces.map((w) => ({
            label: w.client === currentWorkspace ? `${w.client}  (this window)` : w.client,
            title: w.root,
            disabled: w.client === currentWorkspace,
            action: () => onOpenWorkspaceWindow(w.client),
          })),
        },
        { label: "", sep: true },
        { label: "New workspace…", action: onNewWorkspace, disabled: !connected },
        { label: "", sep: true },
        { label: "Refresh", action: onRefresh, disabled: busy, accel: "refresh" },
        { label: "Sync workspace…", action: onSync, disabled: busy, accel: "sync" },
        { label: "", sep: true },
        { label: "Apply patch…", action: onApplyPatch, disabled: busy, accel: "applyPatch" },
      ],
    },
    {
      name: "View",
      items: [
        { label: "Files", action: () => onToggleView("files"), checked: views.files },
        { label: "History", action: () => onToggleView("history"), checked: views.history },
        { label: "Pending", action: () => onToggleView("pending"), checked: views.pending },
        { label: "Reviews", action: () => onToggleView("reviews"), checked: views.reviews },
        { label: "Stashes", action: () => onToggleView("stashes"), checked: views.stashes },
        { label: "Streams", action: () => onToggleView("streams"), checked: views.streams },
        { label: "Commands", action: () => onToggleView("log"), checked: views.log },
        { label: "Notifications", action: () => onToggleView("notes"), checked: views.notes },
      ],
    },
    {
      name: "Help",
      items: [
        { label: "Check for updates…", action: onCheckUpdates },
        { label: "About", action: onAbout },
      ],
    },
  ]);

  function toggle(name: string) {
    open = open === name ? null : name;
    closeSubNow();
  }
  function enter(name: string) {
    if (open !== null && open !== name) {
      open = name; // once a menu is open, hover switches menus
      closeSubNow();
    }
  }
  function run(it: Item) {
    if (it.disabled || !it.action) return;
    open = null;
    closeSubNow();
    it.action();
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && (open = null)} />

<div class="menubar">
  {#each menus as m (m.name)}
    <div class="menu">
      <button
        class="top"
        class:active={open === m.name}
        onclick={() => toggle(m.name)}
        onpointerenter={() => enter(m.name)}
      >
        {m.name}
      </button>
      {#if open === m.name}
        <div class="dropdown">
          {#each m.items as it, i (i)}
            {#if it.sep}
              <div class="msep"></div>
            {:else if it.sub}
              <!-- svelte-ignore a11y_no_static_element_interactions -->
              <div
                class="subwrap"
                onpointerenter={() => !it.disabled && openSubmenu(i)}
                onpointerleave={scheduleClose}
              >
                <button class="item" class:on={openSub === i} disabled={it.disabled}>
                  <span class="ilabel">{it.label}</span><span class="arrow">▸</span>
                </button>
                {#if openSub === i}
                  <div class="dropdown sub">
                    {#each it.sub as s, j (j)}
                      <button
                        class="item"
                        disabled={s.disabled}
                        title={s.title ?? ""}
                        onclick={() => run(s)}
                      >
                        <span class="ilabel mono">{s.label}</span>
                      </button>
                    {/each}
                  </div>
                {/if}
              </div>
            {:else}
              <button
                class="item"
                disabled={it.disabled}
                onpointerenter={scheduleClose}
                onclick={() => run(it)}
              >
                {#if it.checked !== undefined}<span class="chk">{it.checked ? "✓" : ""}</span>{/if}<span
                  class="ilabel">{it.label}</span
                >
                <!-- The key comes from the registry, so a rebinding is reflected
                     here without touching this file. -->
                {#if it.accel && shortcuts.accel(it.accel)}
                  <span class="accel mono">{shortcuts.accel(it.accel)}</span>
                {/if}
              </button>
            {/if}
          {/each}
        </div>
      {/if}
    </div>
  {/each}
</div>

{#if open !== null}
  <button class="scrim" aria-label="Close menu" onclick={() => (open = null)}></button>
{/if}

<style>
  /* An accelerator sits right-aligned, dimmed: findable when looked for, quiet
     when not. */
  .ilabel {
    flex: 1;
    text-align: left;
    white-space: nowrap;
  }
  .accel {
    opacity: 0.55;
    font-size: 11px;
    white-space: nowrap;
  }
  .mono {
    font-family: var(--mono);
  }
  .menubar {
    position: relative;
    z-index: 40;
    display: flex;
    align-items: stretch;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
    padding: 0 4px;
    -webkit-user-select: none;
    user-select: none;
  }
  .menu {
    position: relative;
  }
  /* A submenu hangs off the right edge of its row and overlaps the parent's
     border by a pixel, so crossing into it never passes over a gap that would
     close it. */
  .subwrap {
    position: relative;
  }
  .dropdown.sub {
    top: -5px;
    left: calc(100% - 1px);
    border-radius: 6px;
  }
  .item.on {
    background: var(--bg-hover);
  }
  .arrow {
    opacity: 0.55;
    font-size: 10px;
  }
  .top {
    border: none;
    background: none;
    border-radius: 0;
    padding: 5px 10px;
    font-size: 12px;
    color: var(--text);
    cursor: default;
  }
  .top:hover,
  .top.active {
    background: var(--bg-hover);
    border-color: transparent;
  }
  .dropdown {
    position: absolute;
    top: 100%;
    left: 0;
    z-index: 41;
    min-width: 12rem;
    width: max-content;
    max-width: 90vw;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 0 0 6px 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    padding: 4px 0;
  }
  .item {
    /* flex, not block: the label takes the slack so the accelerator lands on the
       right edge. */
    display: flex;
    align-items: center;
    gap: 10px;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 0;
    padding: 4px 14px;
    font-size: 12px;
    color: var(--text);
  }
  .item:hover:not(:disabled) {
    background: var(--accent);
    color: #fff;
  }
  .item:disabled {
    color: var(--text-dim);
    opacity: 0.6;
  }
  .chk {
    display: inline-block;
    width: 1.1em;
    color: var(--accent);
  }
  .msep {
    height: 1px;
    background: var(--border);
    margin: 4px 0;
  }
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 30;
    border: none;
    background: transparent;
    padding: 0;
  }
</style>
