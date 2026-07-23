<script lang="ts">
  import { safe } from "$lib/safe.svelte";

  const req = $derived(safe.current);
  let always = $state(false);
  // Reset the checkbox whenever a new request comes to the front of the queue.
  $effect(() => {
    req?.label;
    always = false;
  });
  function decide(allow: boolean) {
    safe.answer({ allow, always });
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (!req) return;
    if (e.key === "Escape") decide(false);
    else if (e.key === "Enter") decide(true);
  }}
/>

{#if req}
  <div class="overlay">
    <div class="backdrop"></div>
    <div class="dialog" role="dialog" aria-modal="true">
      <div class="dtitle">Safe mode — approve command</div>
      <p class="msg">The app wants to run a Perforce command:</p>
      <div class="cmd mono">p4 {req.label}</div>
      <label class="always">
        <input type="checkbox" bind:checked={always} />
        Always allow <span class="mono">{req.label}</span>
      </label>
      <div class="actions">
        <button onclick={() => decide(false)}>Deny</button>
        <button class="primary" onclick={() => decide(true)}>Allow</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .overlay {
    position: fixed;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 130; /* above every other dialog */
  }
  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
  }
  .dialog {
    position: relative;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.4);
    padding: 16px 18px;
    width: 26rem;
    max-width: 92vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .msg {
    margin: 0;
    font-size: 12px;
    color: var(--text-dim);
  }
  .cmd {
    font-size: 13px;
    color: var(--text);
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 8px;
    overflow-wrap: anywhere;
  }
  .always {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--text-dim);
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
