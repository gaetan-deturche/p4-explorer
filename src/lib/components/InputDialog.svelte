<script lang="ts">
  import { untrack } from "svelte";

  let {
    title,
    label,
    placeholder = "",
    initial = "",
    okLabel = "OK",
    multiline = false,
    password = false,
    onSubmit,
    onCancel,
  }: {
    title: string;
    label: string;
    placeholder?: string;
    initial?: string;
    okLabel?: string;
    multiline?: boolean;
    password?: boolean; // mask input and don't trim (for credentials)
    onSubmit: (value: string) => void;
    onCancel: () => void;
  } = $props();

  let value = $state(untrack(() => initial));
  let el: HTMLInputElement | HTMLTextAreaElement | undefined = $state();
  $effect(() => el?.focus());

  const cleaned = $derived(password ? value : value.trim());
  function submit() {
    if (cleaned) onSubmit(cleaned);
  }
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true">
    <div class="dtitle">{title}</div>
    <label class="lbl">
      <span>{label}</span>
      {#if multiline}
        <textarea
          bind:this={el}
          bind:value
          rows="4"
          {placeholder}
          onkeydown={(e) => {
            if (e.key === "Escape") onCancel();
            else if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) submit();
          }}
        ></textarea>
      {:else}
        <input
          bind:this={el}
          bind:value
          type={password ? "password" : "text"}
          {placeholder}
          onkeydown={(e) => {
            if (e.key === "Enter") submit();
            else if (e.key === "Escape") onCancel();
          }}
        />
      {/if}
    </label>
    {#if multiline}<span class="hint">Ctrl+Enter to save</span>{/if}
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" disabled={!cleaned} onclick={submit}>{okLabel}</button>
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
  .lbl {
    display: flex;
    flex-direction: column;
    gap: 5px;
    font-size: 12px;
    color: var(--text-dim);
  }
  .lbl input,
  .lbl textarea {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 8px;
    resize: vertical;
  }
  .lbl input:focus,
  .lbl textarea:focus {
    outline: none;
    border-color: var(--accent);
  }
  .hint {
    font-size: 11px;
    color: var(--text-dim);
    margin-top: -4px;
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
