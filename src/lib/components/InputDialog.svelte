<script lang="ts">
  let {
    title,
    label,
    placeholder = "",
    okLabel = "OK",
    onSubmit,
    onCancel,
  }: {
    title: string;
    label: string;
    placeholder?: string;
    okLabel?: string;
    onSubmit: (value: string) => void;
    onCancel: () => void;
  } = $props();

  let value = $state("");
  let inputEl: HTMLInputElement | undefined = $state();
  $effect(() => inputEl?.focus());

  function submit() {
    const v = value.trim();
    if (v) onSubmit(v);
  }
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true">
    <div class="dtitle">{title}</div>
    <label class="lbl">
      <span>{label}</span>
      <input
        bind:this={inputEl}
        bind:value
        type="text"
        {placeholder}
        onkeydown={(e) => {
          if (e.key === "Enter") submit();
          else if (e.key === "Escape") onCancel();
        }}
      />
    </label>
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" disabled={!value.trim()} onclick={submit}>{okLabel}</button>
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
    width: 26rem;
    max-width: 92vw;
    display: flex;
    flex-direction: column;
    gap: 12px;
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
