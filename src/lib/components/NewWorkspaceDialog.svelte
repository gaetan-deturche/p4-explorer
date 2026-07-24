<script lang="ts">
  import { untrack } from "svelte";

  let {
    initialStream = "",
    onSubmit,
    onCancel,
  }: {
    initialStream?: string;
    onSubmit: (v: { name: string; root: string; stream: string }) => void;
    onCancel: () => void;
  } = $props();

  let name = $state("");
  let root = $state("");
  let stream = $state(untrack(() => initialStream));
  let nameEl: HTMLInputElement | undefined = $state();
  $effect(() => nameEl?.focus());

  const ready = $derived(!!name.trim() && !!root.trim() && !!stream.trim());
  function submit() {
    if (ready) onSubmit({ name: name.trim(), root: root.trim(), stream: stream.trim() });
  }
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true">
    <div class="dtitle">New workspace</div>
    <p class="hint dim">Creates a stream workspace bound to this machine (Host).</p>
    <label class="lbl">
      <span>Name</span>
      <input
        bind:this={nameEl}
        bind:value={name}
        class="mono"
        placeholder="user_host_stream"
        onkeydown={(e) => e.key === "Escape" && onCancel()}
      />
    </label>
    <label class="lbl">
      <span>Root (local folder)</span>
      <input
        bind:value={root}
        class="mono"
        placeholder="H:\Dev\..."
        onkeydown={(e) => e.key === "Escape" && onCancel()}
      />
    </label>
    <label class="lbl">
      <span>Stream</span>
      <input
        bind:value={stream}
        class="mono"
        placeholder="//Depot/Stream"
        onkeydown={(e) => {
          if (e.key === "Enter") submit();
          else if (e.key === "Escape") onCancel();
        }}
      />
    </label>
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" disabled={!ready} onclick={submit}>Create</button>
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
    z-index: 95;
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
    width: 28rem;
    max-width: 92vw;
    display: flex;
    flex-direction: column;
    gap: 10px;
  }
  .dtitle {
    font-size: 13px;
    font-weight: 600;
  }
  .hint {
    margin: 0;
    font-size: 11px;
  }
  .lbl {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .lbl input {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 8px;
  }
  .lbl input:focus {
    outline: none;
    border-color: var(--accent);
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
  .primary:disabled {
    opacity: 0.5;
  }
</style>
