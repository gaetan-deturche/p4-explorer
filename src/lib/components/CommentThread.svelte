<script lang="ts">
  //! One comment thread: the root, its replies, and what can be done to them.
  //!
  //! Shared by the review window's pane and the diff window's inline bubble, so
  //! a thread reads and behaves the same in both — and so the task/archive/edit
  //! rules live in one place instead of two.
  import { fmtTime, type Comment } from "$lib/p4";
  import { taskLabel, type Thread } from "$lib/comments";

  let {
    thread,
    me,
    busy = false,
    compact = false,
    showAnchor = false,
    onReply,
    onEdit,
    onTask,
    onArchive,
    taskStates,
  }: {
    thread: Thread;
    me: string;
    busy?: boolean;
    /** Inline in a diff: drop the snippet and tighten the spacing. */
    compact?: boolean;
    /** Show the file/line the thread is anchored to (the review pane wants it). */
    showAnchor?: boolean;
    onReply: (body: string) => Promise<void>;
    onEdit: (c: Comment, body: string) => Promise<void>;
    onTask: (c: Comment, state: string) => Promise<void>;
    onArchive: (c: Comment, closed: boolean) => Promise<void>;
    /** The task states Swarm will accept for this comment — asked lazily, since
     *  finding out costs a (rejected) request per comment. */
    taskStates: (c: Comment) => Promise<string[]>;
  } = $props();

  let replying = $state(false);
  let draft = $state("");
  let editing = $state(0); // comment id being edited
  let editDraft = $state("");
  /** Task menus already asked about, by comment id. */
  let states = $state<Record<number, string[]>>({});
  let asking = $state(0);

  const all = $derived([thread.root, ...thread.replies]);

  async function send() {
    const body = draft.trim();
    if (!body) return;
    await onReply(body);
    draft = "";
    replying = false;
  }

  function startEdit(c: Comment) {
    editing = c.id;
    editDraft = c.body;
  }
  async function saveEdit(c: Comment) {
    const body = editDraft.trim();
    if (!body || body === c.body) {
      editing = 0;
      return;
    }
    await onEdit(c, body);
    editing = 0;
  }

  /** Ask Swarm what this comment may become, once per comment. */
  async function openTaskMenu(c: Comment) {
    if (states[c.id]) return;
    asking = c.id;
    try {
      states[c.id] = await taskStates(c);
    } finally {
      asking = 0;
    }
  }

  /** Ctrl+Enter sends, Escape backs out — the shortcuts a comment box has. */
  function boxKey(e: KeyboardEvent, submit: () => void, cancel: () => void) {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      submit();
    } else if (e.key === "Escape") {
      e.preventDefault();
      cancel();
    }
  }
</script>

<div class="thread" class:compact class:archived={thread.root.closed}>
  {#if showAnchor && thread.file}
    <div class="anchor mono" title={thread.file}>
      {thread.file.split("/").pop()}
      {#if thread.rightLine}:{thread.rightLine}{:else if thread.leftLine}:{thread.leftLine}{/if}
      <span class="dim">· v{thread.version}{thread.leftLine ? " base" : ""}</span>
    </div>
  {/if}

  {#if !compact && thread.root.content.length}
    <!-- The snippet Swarm stored with the anchor, so the thread still reads on
         its own once the code has moved. -->
    <pre class="snip mono">{#each thread.root.content as l}<span
          class:add={l.startsWith("+")}
          class:del={l.startsWith("-")}>{l}
</span>{/each}</pre>
  {/if}

  {#each all as c (c.id)}
    <div class="cmt" class:mine={c.user === me}>
      <div class="chead">
        <span class="who">{c.user}</span>
        <span class="dim small" title={fmtTime(String(c.updated || c.time))}>
          {fmtTime(String(c.time))}
        </span>
        {#if c.edited}<span class="dim small">· edited</span>{/if}
        {#if taskLabel(c.taskState)}
          <span class="task t-{c.taskState}">{taskLabel(c.taskState)}</span>
        {/if}
        {#if c.closed}<span class="task archived">archived</span>{/if}
        <span class="grow"></span>
        {#if c.assetFile}
          <span class="dim small" title={c.assetFile}>
            {c.assetFile.split("/").pop()}{c.assetCategory ? ` · ${c.assetCategory}` : ""}
          </span>
        {/if}
      </div>

      {#if editing === c.id}
        <textarea
          bind:value={editDraft}
          rows="3"
          onkeydown={(e) => boxKey(e, () => void saveEdit(c), () => (editing = 0))}
        ></textarea>
        <div class="acts">
          <button disabled={busy} onclick={() => void saveEdit(c)}>Save</button>
          <button disabled={busy} onclick={() => (editing = 0)}>Cancel</button>
          <span class="dim small">Ctrl+Enter saves</span>
        </div>
      {:else}
        <div class="body">{c.body}</div>
        <div class="acts row">
          {#if c.user === me}
            <button class="link" disabled={busy} onclick={() => startEdit(c)}>Edit</button>
          {/if}
          <button
            class="link"
            disabled={busy}
            onclick={() => void onArchive(c, !c.closed)}
            title={c.closed ? "Bring this back into the review" : "Archive this thread in Swarm"}
          >
            {c.closed ? "Unarchive" : "Archive"}
          </button>
          <span class="taskbox">
            <button class="link" disabled={busy} onclick={() => void openTaskMenu(c)}>
              {asking === c.id ? "asking…" : "Task"}
            </button>
            {#if states[c.id]}
              {#if states[c.id].length === 0}
                <span class="dim small">Swarm allows no change here.</span>
              {:else}
                {#each states[c.id] as s (s)}
                  <button class="link go" disabled={busy} onclick={() => void onTask(c, s)}>
                    → {s}
                  </button>
                {/each}
              {/if}
            {/if}
          </span>
        </div>
      {/if}
    </div>
  {/each}

  {#if replying}
    <textarea
      bind:value={draft}
      rows="3"
      placeholder="Reply…"
      onkeydown={(e) => boxKey(e, () => void send(), () => (replying = false))}
    ></textarea>
    <div class="acts">
      <button disabled={busy || !draft.trim()} onclick={() => void send()}>Reply</button>
      <button disabled={busy} onclick={() => (replying = false)}>Cancel</button>
      <span class="dim small">Ctrl+Enter sends · everyone on the review is mailed</span>
    </div>
  {:else}
    <button class="link reply" disabled={busy} onclick={() => (replying = true)}>Reply</button>
  {/if}
</div>

<style>
  .thread {
    border: 1px solid var(--line, #2a2a2a);
    border-radius: 4px;
    padding: 6px;
    margin: 0 0 6px;
    font-size: 12px;
  }
  .thread.compact {
    margin: 0;
  }
  .thread.archived {
    opacity: 0.65;
  }
  .anchor {
    font-size: 11px;
    margin-bottom: 4px;
  }
  .snip {
    margin: 0 0 6px;
    padding: 4px 6px;
    max-height: 7em;
    overflow: auto;
    background: color-mix(in srgb, var(--fg, #ddd) 6%, transparent);
    border-radius: 3px;
    font-size: 11px;
    white-space: pre;
  }
  .snip .add {
    color: #7ec97e;
  }
  .snip .del {
    color: #f08a8a;
  }
  .cmt {
    padding: 3px 0;
  }
  .cmt + .cmt {
    border-top: 1px dotted var(--line, #2a2a2a);
  }
  .chead {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }
  .who {
    font-weight: 600;
  }
  .mine .who {
    color: var(--accent, #4a7);
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
  .task {
    padding: 0 4px;
    border-radius: 6px;
    font-size: 10px;
    background: color-mix(in srgb, var(--fg, #ddd) 12%, transparent);
  }
  .task.t-open {
    color: #e0b060;
    background: color-mix(in srgb, #e0b060 16%, transparent);
  }
  .task.t-addressed {
    color: #8ab4f0;
    background: color-mix(in srgb, #8ab4f0 16%, transparent);
  }
  .task.t-verified {
    color: #7ec97e;
    background: color-mix(in srgb, #7ec97e 16%, transparent);
  }
  .body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    padding: 1px 0 2px;
  }
  textarea {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-family: var(--mono, ui-monospace, Consolas, monospace);
    font-size: 12px;
    resize: vertical;
  }
  .acts {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    padding-top: 3px;
  }
  .acts.row {
    opacity: 0.75;
  }
  .thread:hover .acts.row {
    opacity: 1;
  }
  .taskbox {
    display: inline-flex;
    align-items: center;
    gap: 4px;
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
    opacity: 0.85;
  }
  button.link:hover {
    opacity: 1;
  }
  button.link.go {
    color: var(--accent, #4a7);
  }
  button.reply {
    margin-top: 2px;
  }
</style>
