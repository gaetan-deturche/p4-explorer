<script lang="ts">
  import type { Views } from "$lib/nav";

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
    onNewWorkspace,
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
    onNewWorkspace: () => void;
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
  };

  let open = $state<string | null>(null);
  const busy = $derived(!connected || refreshing || syncing);

  const menus = $derived<{ name: string; items: Item[] }[]>([
    {
      name: "File",
      items: [
        { label: "Options…", action: onOptions },
        { label: "Reconnect", action: onReconnect },
        { label: "", sep: true },
        { label: "Exit", action: onExit },
      ],
    },
    {
      name: "Workspace",
      items: [
        { label: "New workspace…", action: onNewWorkspace, disabled: !connected },
        { label: "", sep: true },
        { label: "Refresh", action: onRefresh, disabled: busy },
        { label: "Sync workspace…", action: onSync, disabled: busy },
      ],
    },
    {
      name: "View",
      items: [
        { label: "Files", action: () => onToggleView("files"), checked: views.files },
        { label: "History", action: () => onToggleView("history"), checked: views.history },
        { label: "Pending", action: () => onToggleView("pending"), checked: views.pending },
        { label: "Streams", action: () => onToggleView("streams"), checked: views.streams },
        { label: "Depot", action: () => onToggleView("repo"), checked: views.repo },
        { label: "Commands", action: () => onToggleView("log"), checked: views.log },
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
  }
  function enter(name: string) {
    if (open !== null) open = name; // once a menu is open, hover switches menus
  }
  function run(it: Item) {
    if (it.disabled || !it.action) return;
    open = null;
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
            {:else}
              <button class="item" disabled={it.disabled} onclick={() => run(it)}>
                {#if it.checked !== undefined}<span class="chk">{it.checked ? "✓" : ""}</span>{/if}{it.label}
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
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 0 0 6px 6px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.35);
    padding: 4px 0;
  }
  .item {
    display: block;
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
