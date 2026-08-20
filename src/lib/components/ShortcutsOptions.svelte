<script lang="ts">
  //! The Shortcuts page of Options: every bindable action, its current key, and
  //! a way to change it.
  //!
  //! Rebinding CAPTURES the next keystroke rather than asking the user to type a
  //! key name — nobody should have to know whether the app spells it "Ctrl+Left"
  //! or "control+ArrowLeft". While capturing, every key is swallowed, including
  //! Escape (which cancels) and Enter, so a shortcut cannot fire the very action
  //! being rebound.
  import { ACTIONS, describe, shortcuts, type ActionDef } from "$lib/shortcuts.svelte";

  let capturing = $state<string | null>(null); // action id being rebound
  /** Bumped after every change so the rendered keys re-read from the store. */
  let version = $state(0);

  /** id → binding, rebuilt whenever something changes, so the rows re-render. */
  const keys = $derived.by(() => {
    void version;
    return new Map(ACTIONS.map((a) => [a.id, shortcuts.key(a.id)]));
  });

  const groups = $derived.by(() => {
    void version;
    const out = new Map<string, ActionDef[]>();
    for (const a of ACTIONS) {
      const list = out.get(a.group) ?? [];
      list.push(a);
      out.set(a.group, list);
    }
    return [...out.entries()];
  });

  function onCapture(e: KeyboardEvent) {
    if (!capturing) return;
    // Swallow everything: the point of capture mode is that no key does its
    // usual job, or rebinding Ctrl+W would close the window.
    e.preventDefault();
    e.stopPropagation();
    if (e.key === "Escape") {
      capturing = null;
      return;
    }
    if (e.key === "Backspace" || e.key === "Delete") {
      // Deliberate unbind — Delete is itself bindable, so this only applies
      // while capturing.
      shortcuts.set(capturing, "");
      capturing = null;
      version++;
      return;
    }
    const key = describe(e);
    if (!key) return; // a modifier on its own; wait for the real key
    shortcuts.set(capturing, key);
    capturing = null;
    version++;
  }
</script>

<svelte:window onkeydown={capturing ? onCapture : undefined} />

<div class="wrap">
  <div class="hint dim">
    Click a shortcut to change it, then press the keys. Escape cancels, Backspace
    clears it.
  </div>

  {#each groups as [group, items] (group)}
    <div class="section"><span>{group}</span></div>
    {#each items as a (a.id)}
      {@const key = keys.get(a.id) ?? ""}
      {@const clash = shortcuts.clashes(key, a.id)}
      <div class="row" class:destructive={a.destructive}>
        <span class="label">
          {a.label}
          {#if a.destructive}<span class="warn" title="Asks for confirmation before it acts">
              confirms
            </span>{/if}
        </span>
        <button
          class="key mono"
          class:capturing={capturing === a.id}
          class:unbound={!key}
          onclick={() => (capturing = capturing === a.id ? null : a.id)}
        >
          {capturing === a.id ? "press keys…" : key || "unbound"}
        </button>
        <button
          class="reset"
          title={"Back to the shipped binding" + (a.def ? " (" + a.def + ")" : "")}
          disabled={!shortcuts.isCustom(a.id)}
          onclick={() => {
            shortcuts.reset(a.id);
            version++;
          }}
        >
          ↺
        </button>
      </div>
      {#if clash.length}
        <div class="clash">
          also {clash.map((c) => c.label).join(", ")} — the first one listed wins
        </div>
      {/if}
    {/each}
  {/each}

  <div class="foot">
    <button
      onclick={() => {
        shortcuts.resetAll();
        version++;
      }}
    >
      Reset all to defaults
    </button>
  </div>
</div>

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 52vh;
    overflow: auto;
  }
  .hint {
    font-size: 11px;
    padding: 2px 0 6px;
  }
  .dim {
    opacity: 0.7;
  }
  .section {
    margin: 8px 0 2px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    opacity: 0.6;
    border-bottom: 1px solid var(--border);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 2px 0;
    font-size: 12px;
  }
  .label {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .warn {
    font-size: 10px;
    padding: 0 4px;
    border: 1px solid currentColor;
    border-radius: 8px;
    color: var(--warn);
    opacity: 0.8;
  }
  .mono {
    font-family: var(--mono);
  }
  .key {
    min-width: 11ch;
    text-align: center;
  }
  .key.capturing {
    border-color: var(--accent);
    color: var(--accent);
  }
  .key.unbound {
    opacity: 0.55;
  }
  .reset {
    width: 2.2em;
  }
  .clash {
    font-size: 11px;
    color: var(--warn);
    padding: 0 0 2px 4px;
  }
  .foot {
    display: flex;
    justify-content: flex-end;
    padding-top: 10px;
  }
</style>
