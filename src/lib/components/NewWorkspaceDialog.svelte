<script lang="ts">
  import { untrack } from "svelte";

  let {
    initialName = "",
    initialStream = "",
    onSubmit,
    onCancel,
    pickFolder,
    loadStreams,
  }: {
    initialName?: string;
    initialStream?: string;
    onSubmit: (v: { name: string; root: string; stream: string }) => void;
    onCancel: () => void;
    pickFolder: (start: string) => Promise<string | null>;
    loadStreams: () => Promise<{ stream: string; name: string }[]>;
  } = $props();

  let name = $state(untrack(() => initialName));
  let root = $state("");
  let stream = $state(untrack(() => initialStream));
  let nameEl: HTMLInputElement | undefined = $state();
  // Prefill a suggested name but select it so a keystroke replaces the whole thing.
  $effect(() => {
    nameEl?.focus();
    nameEl?.select();
  });

  const ready = $derived(!!name.trim() && !!root.trim() && !!stream.trim());
  function submit() {
    if (ready) onSubmit({ name: name.trim(), root: root.trim(), stream: stream.trim() });
  }

  async function browseRoot() {
    const picked = await pickFolder(root.trim());
    if (picked) root = picked;
  }

  // Stream picker: a searchable overlay over the whole server's streams.
  let pickerOpen = $state(false);
  let streams = $state<{ stream: string; name: string }[]>([]);
  let streamsLoading = $state(false);
  let filter = $state("");
  let filterEl: HTMLInputElement | undefined = $state();
  // Fuzzy: every whitespace-separated token must appear in "name + path", in any
  // order — so "ue5 main" matches //UE5/Main. Rank by where the first token hits.
  const shownStreams = $derived.by(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return streams;
    const tokens = q.split(/\s+/);
    return streams
      .map((s) => {
        const hay = `${s.name}\n${s.stream}`.toLowerCase();
        const hits = tokens.map((t) => hay.indexOf(t));
        return hits.every((i) => i >= 0) ? { s, score: Math.min(...hits) } : null;
      })
      .filter((m): m is { s: { stream: string; name: string }; score: number } => m !== null)
      .sort((a, b) => a.score - b.score)
      .map((m) => m.s);
  });
  async function openPicker() {
    pickerOpen = true;
    filter = "";
    if (streams.length === 0) {
      streamsLoading = true;
      streams = await loadStreams().catch(() => []);
      streamsLoading = false;
    }
    filterEl?.focus();
  }
  function chooseStream(s: string) {
    stream = s;
    pickerOpen = false;
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
      <div class="field">
        <input
          bind:value={root}
          class="mono"
          placeholder="H:\Dev\..."
          onkeydown={(e) => e.key === "Escape" && onCancel()}
        />
        <button class="pick" title="Browse for a folder…" onclick={browseRoot}>Browse…</button>
      </div>
    </label>
    <label class="lbl">
      <span>Stream</span>
      <div class="field">
        <input
          bind:value={stream}
          class="mono"
          placeholder="//Depot/Stream"
          onkeydown={(e) => {
            if (e.key === "Enter") submit();
            else if (e.key === "Escape") onCancel();
          }}
        />
        <button class="pick" title="Choose an existing stream…" onclick={openPicker}>Choose…</button>
      </div>
    </label>
    <div class="actions">
      <button onclick={onCancel}>Cancel</button>
      <button class="primary" disabled={!ready} onclick={submit}>Create</button>
    </div>
  </div>

  {#if pickerOpen}
    <div class="backdrop2"></div>
    <div class="dialog picker" role="dialog" aria-modal="true">
      <div class="dtitle">Choose a stream</div>
      <input
        bind:this={filterEl}
        bind:value={filter}
        class="mono search"
        placeholder="Search streams…"
        onkeydown={(e) => {
          if (e.key === "Escape") pickerOpen = false;
          else if (e.key === "Enter" && shownStreams.length === 1) chooseStream(shownStreams[0].stream);
        }}
      />
      <div class="slist">
        {#if streamsLoading}
          <div class="msg dim">Loading…</div>
        {:else if shownStreams.length === 0}
          <div class="msg dim">{streams.length === 0 ? "No streams on this server." : "No match."}</div>
        {:else}
          {#each shownStreams as s (s.stream)}
            <button
              class="srow"
              class:current={s.stream === stream}
              onclick={() => chooseStream(s.stream)}
            >
              <span class="sname">{s.name}</span>
              <span class="spath mono dim">{s.stream}</span>
            </button>
          {/each}
        {/if}
      </div>
      <div class="actions">
        <button onclick={() => (pickerOpen = false)}>Cancel</button>
      </div>
    </div>
  {/if}
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
  .backdrop2 {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.3);
    z-index: 1;
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
  .picker {
    z-index: 2;
    width: 34rem;
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
  .field {
    display: flex;
    gap: 6px;
  }
  .field input {
    flex: 1;
    min-width: 0;
  }
  .lbl input,
  .search {
    font: inherit;
    font-size: 13px;
    color: var(--text);
    background: var(--bg-alt);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 6px 8px;
  }
  .lbl input:focus,
  .search:focus {
    outline: none;
    border-color: var(--accent);
  }
  .pick {
    white-space: nowrap;
    flex: none;
  }
  .slist {
    max-height: 20rem;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--bg-alt);
  }
  .msg {
    padding: 10px 12px;
    font-size: 12px;
  }
  .srow {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 1px;
    width: 100%;
    text-align: left;
    border: none;
    background: none;
    border-radius: 0;
    padding: 5px 10px;
    cursor: pointer;
    border-bottom: 1px solid var(--border);
  }
  .srow:hover {
    background: var(--bg-hover);
  }
  .srow.current {
    background: var(--bg-sel);
  }
  .sname {
    font-size: 12px;
    color: var(--text);
  }
  .spath {
    font-size: 11px;
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
