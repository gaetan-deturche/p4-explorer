<script lang="ts">
  //! Every comment on a review, in the review window: the discussion on the
  //! review as a whole and the threads anchored to its files, plus a box to add
  //! one.
  //!
  //! This is the reading and managing surface. Writing a comment ON a line
  //! happens in the diff window, where the line is — the composer here posts to
  //! the review itself, which is where "this needs a rebase" belongs.
  import CommentThread from "$lib/components/CommentThread.svelte";
  import { generalThreads, threadsForFile, type Thread } from "$lib/comments";
  import type { Comment } from "$lib/p4";

  let {
    threads,
    loading,
    error,
    me,
    busy,
    file,
    onlyFile,
    onOnlyFile,
    onRefresh,
    onAdd,
    onReply,
    onEdit,
    onTask,
    onArchive,
    taskStates,
  }: {
    threads: Thread[];
    loading: boolean;
    error: string;
    me: string;
    busy: boolean;
    /** The file selected in the files list, for the "this file only" filter. */
    file: string;
    onlyFile: boolean;
    onOnlyFile: (v: boolean) => void;
    onRefresh: () => void;
    onAdd: (body: string) => Promise<void>;
    onReply: (t: Thread, body: string) => Promise<void>;
    onEdit: (c: Comment, body: string) => Promise<void>;
    onTask: (c: Comment, state: string) => Promise<void>;
    onArchive: (c: Comment, closed: boolean) => Promise<void>;
    taskStates: (c: Comment) => Promise<string[]>;
  } = $props();

  let draft = $state("");
  let showArchived = $state(false);

  const visible = $derived(showArchived ? threads : threads.filter((t) => !t.root.closed));
  const general = $derived(generalThreads(visible));
  /** Anchored threads, grouped by file: the selected file first when filtering
   *  is off, so clicking a file in the list still brings its threads up. */
  const byFile = $derived.by(() => {
    const files = [...new Set(visible.filter((t) => t.file).map((t) => t.file))];
    const wanted = onlyFile && file ? files.filter((f) => f === file) : files;
    wanted.sort((a, b) => {
      if (a === file) return -1;
      if (b === file) return 1;
      return a.localeCompare(b);
    });
    return wanted.map((f) => ({ file: f, threads: threadsForFile(visible, f) }));
  });
  const archivedCount = $derived(threads.filter((t) => t.root.closed).length);

  async function add() {
    const body = draft.trim();
    if (!body) return;
    await onAdd(body);
    draft = "";
  }
</script>

<div class="pane">
  <div class="head">
    Comments
    <span class="dim small">
      — {visible.length} thread{visible.length === 1 ? "" : "s"}
      {#if visible.some((t) => t.openTask)}
        · {visible.filter((t) => t.openTask).length} open task{visible.filter((t) => t.openTask)
          .length === 1
          ? ""
          : "s"}
      {/if}
    </span>
    <span class="grow"></span>
    {#if file}
      <label class="opt" title={file}>
        <input type="checkbox" checked={onlyFile} onchange={(e) => onOnlyFile(e.currentTarget.checked)} />
        this file only
      </label>
    {/if}
    {#if archivedCount}
      <label class="opt">
        <input type="checkbox" bind:checked={showArchived} />
        archived ({archivedCount})
      </label>
    {/if}
    <button class="link" onclick={onRefresh} disabled={loading}>
      {loading ? "loading…" : "refresh"}
    </button>
  </div>

  <div class="body">
    {#if error}
      <div class="err mono">{error}</div>
    {/if}

    {#if loading && threads.length === 0}
      <div class="pad dim">Loading comments…</div>
    {:else}
      {#if general.length}
        <div class="group dim small">On the review</div>
        {#each general as t (t.root.id)}
          <CommentThread
            thread={t}
            {me}
            {busy}
            onReply={(b) => onReply(t, b)}
            {onEdit}
            {onTask}
            {onArchive}
            {taskStates}
          />
        {/each}
      {/if}

      {#each byFile as g (g.file)}
        <div class="group dim small" title={g.file}>{g.file.split("/").pop()}</div>
        {#each g.threads as t (t.root.id)}
          <CommentThread thread={t} {me} {busy} showAnchor onReply={(b) => onReply(t, b)} {onEdit} {onTask} {onArchive} {taskStates} />
        {/each}
      {/each}

      {#if visible.length === 0}
        <div class="pad dim">
          {onlyFile && file ? "No comments on this file." : "Nothing has been said on this review yet."}
        </div>
      {/if}
    {/if}
  </div>

  <div class="composer">
    <textarea
      bind:value={draft}
      rows="2"
      placeholder="Comment on the review…"
      onkeydown={(e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
          e.preventDefault();
          void add();
        }
      }}
    ></textarea>
    <div class="crow">
      <button disabled={busy || !draft.trim()} onclick={() => void add()}>Comment</button>
      <!-- This server has no /comments/notify route, so Swarm cannot batch the
           mail: saying so beats letting the user find out from the replies. -->
      <span class="dim small">Ctrl+Enter · everyone on the review is mailed at once</span>
      <span class="grow"></span>
      <span class="dim small">Comment on a LINE from the diff window.</span>
    </div>
  </div>
</div>

<style>
  .pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
    height: 100%;
    font-size: 12px;
  }
  .head {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    background: color-mix(in srgb, var(--fg, #ddd) 6%, transparent);
    border-bottom: 1px solid var(--line, #2a2a2a);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .head .dim,
  .head .opt,
  .head .link {
    text-transform: none;
    letter-spacing: normal;
  }
  .opt {
    display: inline-flex;
    align-items: center;
    gap: 3px;
    font-size: 11px;
    opacity: 0.8;
    max-width: 9rem;
    overflow: hidden;
    white-space: nowrap;
  }
  .grow {
    flex: 1;
  }
  .dim {
    opacity: 0.6;
  }
  .small {
    font-size: 11px;
  }
  .mono {
    font-family: var(--mono, ui-monospace, Consolas, monospace);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 6px;
  }
  .pad {
    padding: 6px 2px;
  }
  .err {
    padding: 4px 6px;
    margin-bottom: 6px;
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 12%, transparent);
  }
  .group {
    padding: 4px 0 2px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .composer {
    flex: none;
    border-top: 1px solid var(--line, #2a2a2a);
    padding: 5px 6px;
  }
  .composer textarea {
    width: 100%;
    box-sizing: border-box;
    font-family: var(--mono, ui-monospace, Consolas, monospace);
    font-size: 12px;
    resize: vertical;
  }
  .crow {
    display: flex;
    align-items: center;
    gap: 8px;
    padding-top: 4px;
  }
  button.link {
    background: none;
    border: none;
    padding: 0;
    color: inherit;
    font: inherit;
    font-size: 11px;
    text-decoration: underline;
    cursor: pointer;
  }
</style>
