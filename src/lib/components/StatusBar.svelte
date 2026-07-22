<script lang="ts">
  let {
    connected,
    serverVersion,
    appVersion,
    busy,
    onConnect,
  }: {
    connected: boolean;
    serverVersion: string;
    appVersion: string;
    busy: boolean;
    onConnect: () => void;
  } = $props();
</script>

<div class="statusbar">
  <span class="app dim">Auger{appVersion ? ` v${appVersion}` : ""}</span>
  {#if connected}
    <span class="ok">● connected</span>
  {:else}
    <button onclick={onConnect} disabled={busy}>{busy ? "Connecting…" : "Connect"}</button>
    <span class="dim">not connected</span>
  {/if}
  {#if connected && serverVersion}<span class="ver mono dim">{serverVersion}</span>{/if}
</div>

<style>
  .statusbar {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 3px 12px;
    background: var(--bg-alt);
    border-top: 1px solid var(--border);
    font-size: 11px;
    min-height: 26px;
  }
  .app {
    font-size: 11px;
  }
  .ok {
    color: var(--have);
  }
  .ver {
    margin-left: auto;
    font-size: 10px;
  }
  .statusbar button {
    font-size: 11px;
    padding: 2px 12px;
  }
</style>
