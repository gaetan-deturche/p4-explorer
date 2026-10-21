<script lang="ts">
  import { untrack } from "svelte";
  let {
    title,
    message,
    okLabel = "OK",
    // An optional tick-box carried by the confirmation itself, for a choice that
    // belongs to the action rather than to a separate dialog (the patch dialog's
    // "leave the files offline" is the same idea).
    optionLabel = "",
    optionChecked = false,
    // Rows with a tick-box each, between the message and `footer` — for a
    // choice made per item (which parked changelists come along on a switch).
    choices = [],
    footer = "",
    onOk,
    onCancel,
  }: {
    title: string;
    message: string;
    okLabel?: string;
    optionLabel?: string;
    optionChecked?: boolean;
    choices?: { label: string; detail?: string; checked: boolean }[];
    footer?: string;
    onOk: (option: boolean, picks: boolean[]) => void;
    onCancel: () => void;
  } = $props();

  let option = $state(untrack(() => optionChecked));
  let picks = $state(untrack(() => choices.map((c) => c.checked)));
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape") onCancel();
    else if (e.key === "Enter") onOk(option, picks);
  }}
/>

<div class="overlay">
  <button class="backdrop" aria-label="Cancel" onclick={onCancel}></button>
  <div class="dialog" class:wide={choices.length > 0} role="dialog" aria-modal="true" tabindex="-1">
    {#if title}<div class="dtitle">{title}</div>{/if}
    <p class="msg">{message}</p>
    {#if choices.length}
      <div class="choices">
        {#each choices as c, i (i)}
          <label class="choice" title={c.label + (c.detail ? " — " + c.detail : "")}>
            <input type="checkbox" bind:checked={picks[i]} />
            <span class="clabel mono">{c.label}</span>
            {#if c.detail}<span class="cdetail">{c.detail}</span>{/if}
          </label>
        {/each}
      </div>
    {/if}
    {#if footer}<p class="msg">{footer}</p>{/if}
    {#if optionLabel}
      <label class="opt">
        <input type="checkbox" bind:checked={option} />
        {optionLabel}
      </label>
    {/if}
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" onclick={() => onOk(option, picks)}>{okLabel}</button>
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
    z-index: 120; /* above every other dialog — confirms are raised from within them */
  }
  .backdrop {
    position: absolute;
    inset: 0;
    border: none;
    border-radius: 0;
    padding: 0;
    background: rgba(0, 0, 0, 0.4);
    cursor: default;
  }
  .dialog {
    position: relative;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.4);
    padding: 16px 18px;
    width: 28rem;
    max-width: 90vw;
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
    line-height: 1.5;
    white-space: pre-wrap;
    overflow-wrap: anywhere; /* break long depot paths instead of overflowing */
    max-height: 50vh;
    overflow-y: auto;
    color: var(--text);
  }
  .choices {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 40vh;
    overflow-y: auto;
  }
  .choice {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    cursor: pointer;
  }
  .clabel {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .cdetail {
    flex: none;
    color: var(--text-dim, #999);
  }
  .dialog.wide {
    width: 36rem;
  }
  .mono {
    font-family: var(--mono);
  }
  .opt {
    display: flex;
    align-items: center;
    gap: 6px;
    margin: 0 0 10px;
    font-size: 12px;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 4px;
  }
  .primary {
    border-color: var(--accent);
    color: var(--accent);
  }
</style>
