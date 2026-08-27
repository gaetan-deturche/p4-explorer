<script lang="ts">
  //! Managing existing workspaces: pick one, change where it lives, rename it,
  //! or delete it.
  //!
  //! It sits behind the workspace picker rather than in Options, because a
  //! workspace is server state, not an app preference — Options is where the app's
  //! own settings live. The picker is where workspaces already are.
  //!
  //! Every write reports p4's own refusal text. p4 has a lot to say about what it
  //! will not do to a client (files opened, promoted shelves, a name in use), and
  //! that text is the whole answer.
  import type { ClientSpec, P4Record } from "$lib/p4";

  let {
    clients,
    localClients,
    current,
    clientHost,
    loadSpec,
    onSave,
    onRename,
    onDelete,
    onClose,
    pickFolder,
    loadStreams,
    askConfirm,
  }: {
    clients: P4Record[];
    localClients: Set<string>; // bound to this machine (Host matches)
    current: string; // the workspace in use
    clientHost: string; // this machine, for the "bound here" box
    loadSpec: (client: string) => Promise<ClientSpec>;
    onSave: (v: {
      client: string;
      root: string;
      stream: string;
      host: string;
      description: string;
    }) => Promise<void>;
    onRename: (from: string, to: string) => Promise<void>;
    onDelete: (client: string) => Promise<void>;
    onClose: () => void;
    pickFolder: (start: string) => Promise<string | null>;
    loadStreams: () => Promise<{ stream: string; name: string }[]>;
    askConfirm: (msg: string, title?: string, ok?: string) => Promise<boolean>;
  } = $props();

  let picked = $state("");
  let spec = $state<ClientSpec | null>(null);
  let loading = $state(false);
  let busy = $state(false);
  /** Inline, because the dialog is modal: an app-level banner behind it is no use. */
  let err = $state("");
  let msg = $state("");

  // The form. Kept apart from `spec` so Save knows what actually changed and the
  // dialog can tell you there is nothing to save.
  let root = $state("");
  let stream = $state("");
  let boundHere = $state(false);
  let description = $state("");

  const dirty = $derived(
    !!spec &&
      (root.trim() !== spec.root ||
        stream.trim() !== spec.stream ||
        description.trim() !== spec.description ||
        (boundHere ? clientHost : "") !== spec.host),
  );

  // Open on the workspace in use: it is the one being asked about nine times out
  // of ten, and an empty form beside a list is a dead end.
  $effect(() => {
    if (picked || !clients.length) return;
    const start = clients.some((c) => c.client === current) ? current : String(clients[0].client);
    void select(start);
  });

  async function select(name: string) {
    picked = name;
    // The previous spec STAYS while the next one loads. Blanking it collapsed the
    // dialog to the height of one "Loading…" line and grew it back a moment later,
    // which is a jump for a fetch that takes about 50ms. It is dimmed and inert
    // instead, so nothing on screen is mistaken for the workspace being opened.
    err = "";
    msg = "";
    renaming = false;
    loading = true;
    try {
      const s = await loadSpec(name);
      if (picked !== name) return; // moved on while it loaded
      spec = s;
      root = s.root;
      stream = s.stream;
      description = s.description.trim();
      boundHere = !!s.host && s.host === clientHost;
    } catch (e) {
      err = String(e);
      if (spec && spec.client !== name) spec = null; // stale spec would lie about `picked`
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (!spec || !dirty) return;
    busy = true;
    err = "";
    msg = "";
    try {
      await onSave({
        client: spec.client,
        root: root.trim(),
        stream: stream.trim(),
        host: boundHere ? clientHost : "",
        description: description.trim(),
      });
      msg = "Saved.";
      await select(spec.client); // read back what p4 actually stored
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  // --- rename ---------------------------------------------------------------
  let renaming = $state(false);
  let newName = $state("");
  let nameEl: HTMLInputElement | undefined = $state();
  function startRename() {
    if (!spec) return;
    renaming = true;
    newName = spec.client;
    err = "";
    msg = "";
    // Focus after the input exists.
    queueMicrotask(() => {
      nameEl?.focus();
      nameEl?.select();
    });
  }
  async function applyRename() {
    if (!spec) return;
    const to = newName.trim();
    if (!to || to === spec.client) {
      renaming = false;
      return;
    }
    busy = true;
    err = "";
    try {
      await onRename(spec.client, to);
      renaming = false;
      msg = "Renamed.";
      await select(to);
    } catch (e) {
      err = String(e);
    } finally {
      busy = false;
    }
  }

  async function remove() {
    if (!spec) return;
    const name = spec.client;
    const mine = name === current;
    const ok = await askConfirm(
      `${name}\n\nDelete this workspace?\n\nThe files on disk are left alone; Perforce forgets which revisions this workspace had.${
        mine ? "\n\nIt is the one in use, so nothing will be selected afterwards." : ""
      }`,
      "Delete workspace",
      "Delete",
    );
    if (!ok) return;
    busy = true;
    err = "";
    try {
      await onDelete(name);
      spec = null;
      picked = "";
      msg = `${name} deleted.`;
    } catch (e) {
      err = String(e); // p4's reason: opened files, pending changes, not the owner
    } finally {
      busy = false;
    }
  }

  async function browseRoot() {
    const p = await pickFolder(root.trim());
    if (p) root = p;
  }

  // --- stream picker (same fuzzy list as the New-workspace dialog) -----------
  let pickerOpen = $state(false);
  let streams = $state<{ stream: string; name: string }[]>([]);
  let streamsLoading = $state(false);
  let filter = $state("");
  let filterEl: HTMLInputElement | undefined = $state();
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
    queueMicrotask(() => filterEl?.focus());
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key !== "Escape") return;
    if (pickerOpen) pickerOpen = false;
    else if (renaming) renaming = false;
    else onClose();
  }}
/>

<div class="overlay">
  <div class="backdrop"></div>
  <div class="dialog" role="dialog" aria-modal="true" aria-label="Manage workspaces">
    <div class="dtitle">Workspaces</div>

    <div class="body">
      <div class="list scroll">
        {#each clients as c (c.client)}
          {@const name = String(c.client)}
          <button class="row" class:sel={picked === name} onclick={() => select(name)}>
            <span class="dot" title={localClients.has(name) ? "Bound to this machine" : "Shared / another host"}>
              {localClients.has(name) ? "●" : "○"}
            </span>
            <span class="nm mono">{name}</span>
            {#if name === current}<span class="badge">in use</span>{/if}
            <span class="st dim mono">{c.Stream ?? ""}</span>
          </button>
        {:else}
          <div class="empty dim">No workspaces for this user on this server.</div>
        {/each}
      </div>

      <div class="form" class:stale={loading && !!spec}>
        {#if loading && !spec}
          <div class="dim">Loading…</div>
        {:else if !spec}
          <div class="dim">Pick a workspace on the left.</div>
        {:else}
          <div class="hdr">
            {#if renaming}
              <input
                class="mono grow"
                bind:this={nameEl}
                bind:value={newName}
                onkeydown={(e) => e.key === "Enter" && applyRename()}
              />
              <button class="primary" disabled={busy} onclick={applyRename}>Rename</button>
              <button disabled={busy} onclick={() => (renaming = false)}>Cancel</button>
            {:else}
              <span class="mono grow">{spec.client}</span>
              <button
                disabled={busy}
                title="p4 renameclient: pending changes, shelves, opened files and the have-list move with it. Refused for a workspace with opened streams or promoted shelves."
                onclick={startRename}>Rename…</button
              >
              <button
                class="danger-btn"
                disabled={busy}
                title="p4 client -d: refused while the workspace holds opened files or pending changes"
                onclick={remove}>Delete…</button
              >
            {/if}
          </div>

          <label class="fld">
            Root
            <span class="row2">
              <input class="mono grow" bind:value={root} spellcheck="false" />
              <button disabled={busy} onclick={browseRoot}>Browse…</button>
            </span>
          </label>
          <div class="hint dim">
            Changing the root does not move anything: Perforce simply looks
            elsewhere, and the new location needs a sync.
          </div>

          <label class="fld">
            Stream
            <span class="row2">
              <input class="mono grow" bind:value={stream} spellcheck="false" />
              <button disabled={busy} onclick={openPicker}>Pick…</button>
            </span>
          </label>

          <label class="chk">
            <input type="checkbox" bind:checked={boundHere} />
            Bound to this machine
            <span class="dim">({clientHost || "unknown host"})</span>
          </label>
          <div class="hint dim">
            A workspace bound to a host can only be used from it — which is what
            keeps two machines from sharing one workspace's have-list.
          </div>

          <label class="fld">
            Description
            <input bind:value={description} />
          </label>

          <div class="meta dim mono">
            owner {spec.owner} · last used {spec.access || "never"} · {spec.lineEnd} line endings
          </div>
        {/if}
      </div>
    </div>

    {#if err}
      <div class="err mono">{err}</div>
    {:else if msg}
      <div class="ok">{msg}</div>
    {/if}

    <div class="foot">
      <span class="grow"></span>
      <button onclick={onClose}>Close</button>
      <button class="primary" disabled={!dirty || busy} onclick={save}>
        {busy ? "Saving…" : "Save"}
      </button>
    </div>

    {#if pickerOpen}
      <div class="picker">
        <input
          class="mono"
          placeholder="Filter streams…"
          bind:this={filterEl}
          bind:value={filter}
        />
        <div class="scroll plist">
          {#if streamsLoading}
            <div class="dim pad">Loading streams…</div>
          {:else}
            {#each shownStreams as s (s.stream)}
              <button
                class="prow"
                onclick={() => {
                  stream = s.stream;
                  pickerOpen = false;
                }}
              >
                <span class="mono">{s.stream}</span><span class="dim">{s.name}</span>
              </button>
            {:else}
              <div class="dim pad">No stream matches.</div>
            {/each}
          {/if}
        </div>
        <div class="foot">
          <span class="grow"></span>
          <button onclick={() => (pickerOpen = false)}>Cancel</button>
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    z-index: 70;
    display: grid;
    place-items: center;
  }
  .backdrop {
    position: absolute;
    inset: 0;
    background: rgba(0, 0, 0, 0.45);
  }
  .dialog {
    position: relative;
    width: min(920px, 92vw);
    max-height: 86vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 18px 48px rgba(0, 0, 0, 0.5);
  }
  .dtitle {
    padding: 10px 14px;
    font-weight: 600;
    border-bottom: 1px solid var(--border);
  }
  .body {
    flex: 1;
    /* A floor, so the dialog does not resize itself around whichever workspace is
       selected (or around an empty right-hand pane). */
    min-height: 340px;
    display: grid;
    grid-template-columns: minmax(240px, 40%) minmax(0, 1fr);
  }
  .list {
    border-right: 1px solid var(--border);
    padding: 4px 0;
    overflow: auto;
  }
  .row {
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 4px 10px;
    background: none;
    border: 0;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .row:hover {
    background: var(--bg-hover);
  }
  .row.sel {
    background: var(--bg-sel);
  }
  .row .nm {
    flex: none;
    font-size: 12px;
  }
  .row .st {
    flex: 1;
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .badge {
    flex: none;
    font-size: 10px;
    color: var(--accent);
    border: 1px solid currentColor;
    border-radius: 999px;
    padding: 0 5px;
  }
  /* Visibly not current, and not clickable, while the next spec is on its way. */
  .form.stale {
    opacity: 0.5;
    pointer-events: none;
  }
  .form {
    padding: 12px 14px;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .hdr {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-bottom: 4px;
    border-bottom: 1px solid var(--border);
  }
  .fld {
    display: flex;
    flex-direction: column;
    gap: 3px;
    font-size: 11px;
    color: var(--text-dim);
  }
  .row2 {
    display: flex;
    gap: 6px;
  }
  .grow {
    flex: 1;
  }
  .chk {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
  }
  .hint {
    font-size: 11px;
    line-height: 1.45;
    margin: -2px 0 4px;
  }
  .meta {
    margin-top: auto;
    font-size: 11px;
  }
  .empty,
  .pad {
    padding: 10px;
    font-size: 12px;
  }
  .err,
  .ok {
    padding: 6px 14px;
    font-size: 12px;
    white-space: pre-wrap;
    border-top: 1px solid var(--border);
  }
  .err {
    color: var(--warn);
  }
  .ok {
    color: var(--have);
  }
  .foot {
    display: flex;
    gap: 8px;
    padding: 8px 14px;
    border-top: 1px solid var(--border);
  }
  /* The stream picker sits over the dialog, like the New-workspace one. */
  .picker {
    position: absolute;
    inset: 8px;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
  }
  .plist {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .prow {
    display: flex;
    gap: 10px;
    width: 100%;
    padding: 3px 8px;
    background: none;
    border: 0;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .prow:hover {
    background: var(--bg-hover);
  }
</style>
