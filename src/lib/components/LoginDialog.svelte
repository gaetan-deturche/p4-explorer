<script lang="ts">
  import { untrack } from "svelte";

  let {
    port,
    user = "",
    onSubmit,
    onCancel,
  }: {
    port: string;
    user?: string;
    onSubmit: (v: { user: string; password: string }) => void;
    onCancel: () => void;
  } = $props();

  let u = $state(untrack(() => user));
  let pw = $state("");
  let userEl: HTMLInputElement | undefined = $state();
  let pwEl: HTMLInputElement | undefined = $state();
  // Focus the password if the user is already known, else the user field.
  $effect(() => (u ? pwEl : userEl)?.focus());

  const ready = $derived(!!u.trim() && !!pw);
  function submit() {
    if (ready) onSubmit({ user: u.trim(), password: pw });
  }
</script>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true">
    <div class="dtitle">Log in to {port || "Perforce"}</div>
    <label class="lbl">
      <span>User</span>
      <input
        bind:this={userEl}
        bind:value={u}
        class="mono"
        placeholder="username"
        onkeydown={(e) => {
          if (e.key === "Enter") submit();
          else if (e.key === "Escape") onCancel();
        }}
      />
    </label>
    <label class="lbl">
      <span>Password</span>
      <input
        bind:this={pwEl}
        bind:value={pw}
        type="password"
        onkeydown={(e) => {
          if (e.key === "Enter") submit();
          else if (e.key === "Escape") onCancel();
        }}
      />
    </label>
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" disabled={!ready} onclick={submit}>Log in</button>
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
    width: 24rem;
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
