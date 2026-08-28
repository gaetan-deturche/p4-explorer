<script lang="ts">
  //! One review, in its own window: who is reviewing it, what versions it has
  //! been through, what changed between any two of them, and the state changes
  //! this user is allowed to make.
  //!
  //! A window rather than more rows in the Reviews tab, for the reason the user
  //! gave: none of this fits a list row, and reading a review happens WHILE the
  //! rest of the app stays where it was.
  //!
  //! The versions pane is the point of it. Swarm shows a review as a single
  //! current shelf, so "what did the author change since I last looked?" has no
  //! answer in the list — here every version is a row, any two can be compared,
  //! and the file list says which files actually differ between them (digests,
  //! not merely "present in both").
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import ApprovalDialog from "$lib/components/ApprovalDialog.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import CommentsPane from "$lib/components/CommentsPane.svelte";
  import { threadsOf, type Thread } from "$lib/comments";
  import { editor } from "$lib/editor.svelte";
  import { shortcuts } from "$lib/shortcuts.svelte";
  import { openDiff } from "$lib/opendiff";
  import {
    p4,
    emptyConn,
    baseName,
    fmtTime,
    setClipboard,
    openFileHistoryWindow,
    type P4Conn,
    type Comment,
    type CommentAnchor,
    type ReviewDetail,
    type ReviewVersion,
    type VersionFile,
    type VersionRef,
  } from "$lib/p4";

  const job = new URLSearchParams(window.location.search).get("job") ?? "";

  let conn = $state<P4Conn>(emptyConn());
  let id = $state(0);
  let detail = $state<ReviewDetail | null>(null);
  let loading = $state(true);
  let error = $state("");
  let notice = $state("");
  let busy = $state("");

  /** What Swarm will accept from this user, and what it says is blocked. */
  let actions = $state<{ key: string; label: string }[]>([]);
  let blocked = $state<string[]>([]);

  function setNotice(m: string, ms = 5000) {
    notice = m;
    if (ms > 0) window.setTimeout(() => (notice = ""), ms);
  }

  // --- confirmation (same shape as the other windows) ------------------------
  let confirmState = $state<{
    msg: string;
    title: string;
    ok: string;
    resolve: (v: boolean) => void;
  } | null>(null);
  function askConfirm(msg: string, title: string, ok: string): Promise<boolean> {
    return new Promise((resolve) => (confirmState = { msg, title, ok, resolve }));
  }
  function answer(v: boolean) {
    confirmState?.resolve(v);
    confirmState = null;
  }

  onMount(async () => {
    try {
      const j = await invoke<{ conn: P4Conn; id: number }>("review_job", { job });
      conn = j.conn;
      id = j.id;
      // openDiff reads the diff-tool choice from this store; without init every
      // diff from here would open in-app whatever the user chose.
      void editor.init();
      void shortcuts.init();
      await load();
    } catch (e) {
      error = String(e);
      loading = false;
    }
  });

  async function load() {
    loading = true;
    error = "";
    try {
      detail = await p4.swarmReviewDetail(conn, id);
      // Default comparison: the previous version against the latest, which is
      // the question a reviewer coming back to a review actually has.
      // The whole review at its latest version: what a reviewer is being asked
      // to judge. The increment since the previous version is one click away on
      // the versions table.
      pickB = detail.versions.length;
      pickA = 0;
      void loadTransitions();
      void loadComments();
      await loadFiles();
    } catch (e) {
      error = String(e);
    } finally {
      loading = false;
    }
  }

  async function loadTransitions() {
    try {
      const t = await p4.swarmTransitions(conn, id);
      actions = t.items;
      blocked = t.blocked;
    } catch (e) {
      // Not fatal: the review still reads. Only the buttons are missing, and
      // saying why beats an empty row of them.
      actions = [];
      blocked = [String(e)];
    }
  }

  // --- versions --------------------------------------------------------------
  let pickA = $state(0); // 1-based version numbers; 0 = nothing yet
  let pickB = $state(0);
  let files = $state<VersionFile[]>([]);
  let filesLoading = $state(false);
  let filesError = $state("");

  const versions = $derived(detail?.versions ?? []);
  /** A = 0 means the BASE of B: the revisions B's files were opened from. It is
   *  the review's own starting point, so "what does this version change?" is
   *  asked the same way as "what changed between two versions". */
  const onBase = $derived(pickA === 0);
  const verA = $derived(versions.find((v) => v.n === pickA));
  const verB = $derived(versions.find((v) => v.n === pickB));

  function refOf(v: ReviewVersion): VersionRef {
    return { change: v.change, pending: v.pending, label: `v${v.n}`, base: false };
  }
  /** The base of a version: same changelist, read one step back per file. The
   *  label stays the VERSION's name — the backend words the "base of" part, so
   *  the two sides of a base comparison can never read alike. */
  function baseRefOf(v: ReviewVersion): VersionRef {
    return { change: v.change, pending: v.pending, label: `v${v.n}`, base: true };
  }
  /** The left-hand side of the current comparison. */
  const sideA = $derived(onBase ? (verB ? baseRefOf(verB) : undefined) : verA ? refOf(verA) : undefined);
  const sideB = $derived(verB ? refOf(verB) : undefined);

  async function loadFiles() {
    const a = sideA;
    const b = sideB;
    if (!a || !b) return;
    filesLoading = true;
    filesError = "";
    files = [];
    try {
      files = await p4.reviewVersionFiles(conn, a, b);
    } catch (e) {
      filesError = String(e);
    } finally {
      filesLoading = false;
    }
  }

  /** Re-read the file list when either side of the comparison changes. */
  let lastPick = $state("");
  $effect(() => {
    const key = `${pickA}|${pickB}|${versions.length}`;
    if (key !== lastPick && pickA >= 0 && pickB > 0) {
      lastPick = key;
      void loadFiles();
    }
  });

  // A < B always: a comparison reads "from A to B", and inverting it silently
  // would show every addition as a removal. The dropdowns grey out the options
  // that would break that, so these clamps only ever fire if a selection is
  // invalidated from elsewhere (a reload with fewer versions).
  function selectA(n: number) {
    pickA = n;
    if (n >= pickB) pickB = Math.min(versions.length, Math.max(n + 1, 1));
    if (pickA >= pickB) pickA = 0;
  }
  function selectB(n: number) {
    pickB = n;
    if (pickA >= n) pickA = 0;
  }
  /** Can this version be the left side, given the right one? The base (0) always
   *  can: every version has one. */
  function canBeA(n: number): boolean {
    return n === 0 || n < pickB;
  }
  /** Can it be the right side? Only after A. */
  function canBeB(n: number): boolean {
    return n > pickA;
  }

  // --- comments --------------------------------------------------------------
  let comments = $state<Comment[]>([]);
  let commentsLoading = $state(false);
  let commentsError = $state("");
  let commentsBusy = $state(false);
  /** The file clicked in the list: it drives the comments filter and nothing
   *  else, so clicking a row is free. */
  let selFile = $state("");
  let onlyFile = $state(true);

  const threads = $derived<Thread[]>(threadsOf(comments));
  /** Live threads per file, for the count on each row. Archived ones are left
   *  out: a row saying "3" for three archived threads reads as unfinished work. */
  const threadsByFile = $derived.by(() => {
    const m = new Map<string, number>();
    for (const th of threads) {
      if (!th.file || th.root.closed) continue;
      m.set(th.file, (m.get(th.file) ?? 0) + 1);
    }
    return m;
  });
  const openTasksByFile = $derived.by(() => {
    const m = new Map<string, number>();
    for (const th of threads) {
      if (!th.file || th.root.closed || !th.openTask) continue;
      m.set(th.file, (m.get(th.file) ?? 0) + 1);
    }
    return m;
  });

  async function loadComments() {
    commentsLoading = true;
    commentsError = "";
    try {
      comments = await p4.swarmComments(conn, id);
    } catch (e) {
      commentsError = String(e);
    } finally {
      commentsLoading = false;
    }
  }

  /** Every write goes through here: Swarm is the record, so the list is re-read
   *  rather than patched from a guess about what it did. */
  async function commentWrite(fn: () => Promise<unknown>) {
    if (commentsBusy) return;
    commentsBusy = true;
    try {
      await fn();
      await loadComments();
    } catch (e) {
      error = String(e);
    } finally {
      commentsBusy = false;
    }
  }

  function addComment(body: string) {
    return commentWrite(() => p4.swarmAddComment(conn, id, body, null));
  }
  function replyTo(t: Thread, body: string) {
    // A reply repeats its thread's anchor and names the comment it answers —
    // which is exactly what Swarm stores for one (context.comment).
    const anchor: CommentAnchor = {
      file: t.file,
      version: t.version,
      leftLine: t.leftLine,
      rightLine: t.rightLine,
      content: t.root.content,
      parent: t.root.id,
    };
    return commentWrite(() => p4.swarmAddComment(conn, id, body, anchor));
  }
  function editComment(c: Comment, body: string) {
    return commentWrite(() => p4.swarmEditComment(conn, c.id, body, null, null));
  }
  function taskComment(c: Comment, state: string) {
    return commentWrite(() => p4.swarmEditComment(conn, c.id, null, state, null));
  }
  function archiveComment(c: Comment, closed: boolean) {
    return commentWrite(() => p4.swarmEditComment(conn, c.id, null, null, closed));
  }
  function taskStates(c: Comment) {
    return p4.swarmTaskTransitions(conn, c.id).catch(() => [] as string[]);
  }

  // --- the files / comments split -------------------------------------------
  let filesW = $state(0.58);
  let splitEl = $state<HTMLElement>();
  function splitDown(e: PointerEvent) {
    e.preventDefault();
    const box = splitEl?.parentElement?.getBoundingClientRect();
    if (!box) return;
    const move = (ev: PointerEvent) => {
      filesW = Math.min(0.85, Math.max(0.25, (ev.clientX - box.left) / box.width));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  // --- diffs -----------------------------------------------------------------
  function diffFile(depotFile: string) {
    const a = sideA;
    const b = sideB;
    if (!a || !b) return;
    void openDiff(
      conn,
      {
        kind: "versions",
        file: depotFile,
        a,
        b,
        review: id,
        aVersion: onBase ? 0 : pickA,
        bVersion: pickB,
      },
      setNotice,
    );
  }

  // --- state changes ---------------------------------------------------------
  /** Swarm's transition keys carry the destructive ones; those get a
   *  confirmation, the rest go straight through. Approve is deliberately in the
   *  first group: it is what other people act on. */
  function needsConfirm(key: string): boolean {
    return key.startsWith("approved") || key === "rejected" || key === "archived";
  }

  async function transition(key: string, label: string) {
    if (busy) return;
    if (needsConfirm(key)) {
      const extra =
        key === "approved:commit"
          ? "\n\nThis also SUBMITS the review's shelved files."
          : key === "approved" && staleAfter
            ? "\n\nOther reviewers' votes are older than the current version."
            : "";
      const go = await askConfirm(
        `${label} review #${id}?${extra}`,
        label,
        label.split(" ")[0],
      );
      if (!go) return;
    }
    busy = label;
    try {
      const state = await p4.swarmSetState(conn, id, key);
      setNotice(`Review #${id} is now ${state}.`, 8000);
      await load(); // votes and state both moved; re-read rather than guess
    } catch (e) {
      error = String(e);
    } finally {
      busy = "";
    }
  }

  /** Someone has voted on an older version than the current one — worth saying
   *  before approving on the strength of their vote. */
  const staleAfter = $derived((detail?.reviewers ?? []).some((r) => r.stale && r.vote !== 0));

  // --- presentation ----------------------------------------------------------
  function voteMark(vote: number): string {
    return vote > 0 ? "✔" : vote < 0 ? "✖" : "·";
  }
  function voteClass(vote: number): string {
    return vote > 0 ? "up" : vote < 0 ? "down" : "none";
  }
  function fmtSize(n: number): string {
    if (!n) return "";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  }
  const changed = $derived(files.filter((f) => f.status !== "same"));

  // --- file context menu -----------------------------------------------------
  let ctx = $state<{ x: number; y: number; file: string } | null>(null);
  function fileMenu(file: string) {
    return [
      { label: "Diff", action: () => diffFile(file) },
      { label: "File history…", action: () => void openFileHistoryWindow(conn, file) },
      { label: "", sep: true },
      { label: "Copy path", action: () => void setClipboard(file) },
    ];
  }

  function openSwarm() {
    void (async () => {
      const url = await p4.swarmUrl(conn).catch(() => "");
      if (!url) {
        setNotice("No Swarm server is configured (P4.Swarm.URL is unset).", 6000);
        return;
      }
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(`${url.replace(/\/+$/, "")}/reviews/${id}`).catch((e) => (error = String(e)));
    })();
  }

  function close() {
    void getCurrentWindow().close();
  }
  function onWindowKey(e: KeyboardEvent) {
    if (e.key === "Escape" && !confirmState && !ctx) return close();
    if (shortcuts.match(e, ["app"]) === "closeWindow") {
      e.preventDefault();
      close();
    }
  }
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="wrap">
  <div class="bar">
    <span class="rid">Review #{id}</span>
    {#if detail}
      <span class="state s-{detail.state}">{detail.stateLabel || detail.state}</span>
      <span class="dim">by {detail.author}</span>
      {#if detail.testStatus}<span class="dim">· tests {detail.testStatus}</span>{/if}
      <span class="dim">· updated {fmtTime(String(detail.updated))}</span>
    {/if}
    <span class="grow"></span>
    {#if busy}<span class="dim">{busy}…</span>{/if}
    <button onclick={() => void load()} disabled={loading}>Refresh</button>
    <button onclick={openSwarm}>Open in Swarm</button>
    <button onclick={close}>Close</button>
  </div>

  {#if error}
    <div class="err mono">
      {error}
      <button class="x" onclick={() => (error = "")} title="Dismiss">✕</button>
    </div>
  {/if}
  {#if notice}
    <div class="note">{notice}</div>
  {/if}

  {#if loading && !detail}
    <div class="pad dim">Loading review #{id}…</div>
  {:else if detail}
    <div class="desc" title={detail.description}>{detail.description}</div>

    <div class="split">
      <div class="side">
        <div class="head">Reviewers</div>
        {#if detail.reviewers.length === 0}
          <div class="pad dim">Nobody is on this review yet.</div>
        {:else}
          <ul class="revs">
            {#each detail.reviewers as r (r.user)}
              <li class:author={r.isAuthor}>
                <span class="vote {voteClass(r.vote)}" title={r.vote > 0 ? "Approved" : r.vote < 0 ? "Voted against" : "Has not voted"}>
                  {voteMark(r.vote)}
                </span>
                <span class="who" title={r.user}>{r.user}</span>
                {#if r.isAuthor}<span class="tag">author</span>{/if}
                {#if r.required}<span class="tag req">required</span>{/if}
                {#if r.stale && r.vote !== 0}
                  <span class="tag stale" title="Voted on v{r.votedVersion}, which is not the current version">
                    stale v{r.votedVersion}
                  </span>
                {:else if r.vote !== 0 && r.votedVersion}
                  <span class="dim small">v{r.votedVersion}</span>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}

        <div class="head">Actions</div>
        {#if actions.length === 0}
          <div class="pad dim">
            {blocked.length ? blocked.join("; ") : "Swarm allows you no state change on this review."}
          </div>
        {:else}
          <div class="acts">
            {#each actions as a (a.key)}
              <button class:primary={a.key.startsWith("approved")} disabled={!!busy} onclick={() => void transition(a.key, a.label)}>
                {a.label}
              </button>
            {/each}
          </div>
          {#if blocked.length}
            <div class="pad dim small">{blocked.join("; ")}</div>
          {/if}
          {#if staleAfter}
            <div class="pad warn small">Some votes are older than the current version.</div>
          {/if}
        {/if}

        {#if detail.commits.length}
          <div class="head">Submitted as</div>
          <div class="pad mono small">{detail.commits.map((c) => `@${c}`).join(", ")}</div>
        {/if}
      </div>

      <div class="main">
        <div class="head">
          Versions
          <span class="dim small">— {versions.length} of this review</span>
        </div>
        <div class="vtable">
          <table>
            <thead>
              <tr>
                <th class="pick"></th>
                <th class="n">#</th>
                <th class="ch">Change</th>
                <th>Who</th>
                <th>When</th>
                <th>State</th>
              </tr>
            </thead>
            <tbody>
              <!-- The review's starting point. It belongs to whichever version is
                   selected as B: each version was shelved against its own sync,
                   so "the base" is only meaningful next to a version. -->
              <tr class="baserow" class:sel={onBase}>
                <td class="pick">{#if onBase}<span class="chip">A</span>{/if}</td>
                <td class="n">base</td>
                <td class="ch mono dim">{verB ? `of v${verB.n}` : ""}</td>
                <td colspan="3" class="dim">the revisions this version's files were opened from</td>
              </tr>
              {#each versions as v (v.n)}
                <tr class:sel={v.n === pickA || v.n === pickB}>
                  <td class="pick">
                    {#if v.n === pickA}<span class="chip">A</span>{/if}
                    {#if v.n === pickB}<span class="chip b">B</span>{/if}
                  </td>
                  <td class="n">v{v.n}</td>
                  <td class="ch mono">@{v.change}</td>
                  <td>{v.user}</td>
                  <td>{fmtTime(String(v.time))}</td>
                  <td class="dim">{v.pending ? "shelved" : "submitted"}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <div class="head pick2">
          <span class="lbl">Compare</span>
          <select
            aria-label="Left side of the comparison"
            value={String(pickA)}
            onchange={(e) => selectA(Number(e.currentTarget.value))}
          >
            <option value="0" disabled={!canBeA(0)}>base</option>
            {#each versions as v (v.n)}
              <option value={String(v.n)} disabled={!canBeA(v.n)}>
                v{v.n}{v.pending ? "" : " (submitted)"}
              </option>
            {/each}
          </select>
          <span class="lbl">→</span>
          <select
            aria-label="Right side of the comparison"
            value={String(pickB)}
            onchange={(e) => selectB(Number(e.currentTarget.value))}
          >
            {#each versions as v (v.n)}
              <option value={String(v.n)} disabled={!canBeB(v.n)}>
                v{v.n}{v.pending ? "" : " (submitted)"}
              </option>
            {/each}
          </select>
          <span class="dim small">
            {changed.length} of {files.length} file{files.length === 1 ? "" : "s"} differ
          </span>
        </div>
        <div class="lower">
        <div class="filesbox" style="flex: 0 0 {filesW * 100}%">
        <div class="files">
          {#if filesLoading}
            <div class="pad dim">Comparing…</div>
          {:else if filesError}
            <div class="pad err mono">{filesError}</div>
          {:else if files.length === 0}
            <div class="pad dim">Neither version has any files.</div>
          {:else}
            {#each files as f (f.depotFile)}
              <button
                class="frow st-{f.status}"
                class:selfile={selFile === f.depotFile}
                onclick={() => (selFile = f.depotFile)}
                ondblclick={() => diffFile(f.depotFile)}
                oncontextmenu={(e) => {
                  e.preventDefault();
                  ctx = { x: e.clientX, y: e.clientY, file: f.depotFile };
                }}
              >
                <span class="act">{f.status}</span>
                <span class="fname">{baseName(f.depotFile)}</span>
                <span class="fpath dim">{f.depotFile}</span>
                {#if threadsByFile.get(f.depotFile)}
                  <span
                    class="cbadge"
                    class:hastask={!!openTasksByFile.get(f.depotFile)}
                    title={`${threadsByFile.get(f.depotFile)} comment thread(s)` +
                      (openTasksByFile.get(f.depotFile)
                        ? `, ${openTasksByFile.get(f.depotFile)} open task(s)`
                        : "")}
                  >
                    ≡ {threadsByFile.get(f.depotFile)}
                  </span>
                {/if}
                <span class="size dim">
                  {#if f.status === "changed"}{fmtSize(f.sizeA)} → {fmtSize(f.sizeB)}{:else}{fmtSize(f.sizeB || f.sizeA)}{/if}
                </span>
              </button>
            {/each}
          {/if}
        </div>
        {#if files.length}
          <div class="hint dim small">
            Double-click a file to diff {onBase ? "the base" : `v${pickA}`} against v{pickB} —
            comment on a line there.
          </div>
        {/if}
        </div>
        <div
          class="gutter"
          role="separator"
          aria-orientation="vertical"
          bind:this={splitEl}
          onpointerdown={splitDown}
        ></div>
        <CommentsPane
          {threads}
          loading={commentsLoading}
          error={commentsError}
          me={conn.user}
          busy={commentsBusy}
          file={selFile}
          {onlyFile}
          onOnlyFile={(v) => (onlyFile = v)}
          onRefresh={() => void loadComments()}
          onAdd={addComment}
          onReply={replyTo}
          onEdit={editComment}
          onTask={taskComment}
          onArchive={archiveComment}
          {taskStates}
        />
        </div>
      </div>
    </div>
  {/if}
</div>

{#if ctx}
  <ContextMenu x={ctx.x} y={ctx.y} items={fileMenu(ctx.file)} onClose={() => (ctx = null)} />
{/if}

<!-- Safe mode queues approvals per window: approving a review is a non-read, so
     without this the dialog would never appear here. -->
<ApprovalDialog />

{#if confirmState}
  <ConfirmDialog
    title={confirmState.title}
    message={confirmState.msg}
    okLabel={confirmState.ok}
    onOk={() => answer(true)}
    onCancel={() => answer(false)}
  />
{/if}

<style>
  .wrap {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }
  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--line, #2a2a2a);
    font-size: 12px;
  }
  .rid {
    font-weight: 600;
  }
  .state {
    padding: 1px 6px;
    border-radius: 8px;
    font-size: 11px;
    background: color-mix(in srgb, var(--fg, #ddd) 14%, transparent);
  }
  .state.s-approved {
    color: #7ec97e;
    background: color-mix(in srgb, #7ec97e 16%, transparent);
  }
  .state.s-rejected {
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 16%, transparent);
  }
  .state.s-needsRevision {
    color: #e0b060;
    background: color-mix(in srgb, #e0b060 16%, transparent);
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
  .warn {
    color: #e0b060;
  }
  .mono {
    font-family: var(--mono, ui-monospace, Consolas, monospace);
  }
  .err,
  .note {
    flex: none;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    font-size: 12px;
  }
  .err {
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 12%, transparent);
  }
  .err .x {
    margin-left: auto;
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
  }
  .note {
    background: color-mix(in srgb, var(--fg, #ddd) 8%, transparent);
  }
  .desc {
    flex: none;
    max-height: 5.5em;
    overflow: auto;
    padding: 6px 10px;
    border-bottom: 1px solid var(--line, #2a2a2a);
    font-size: 12px;
    white-space: pre-wrap;
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .side {
    flex: 0 0 260px;
    min-height: 0;
    overflow: auto;
    border-right: 1px solid var(--line, #2a2a2a);
    font-size: 12px;
  }
  .main {
    flex: 1;
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    font-size: 12px;
  }
  .head {
    flex: none;
    padding: 4px 8px;
    background: color-mix(in srgb, var(--fg, #ddd) 6%, transparent);
    border-bottom: 1px solid var(--line, #2a2a2a);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .head .dim {
    text-transform: none;
    letter-spacing: normal;
  }
  .pad {
    padding: 6px 8px;
  }
  .revs {
    margin: 0;
    padding: 4px 0;
    list-style: none;
  }
  .revs li {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 8px;
  }
  .revs li.author {
    opacity: 0.75;
  }
  .vote {
    width: 1em;
    text-align: center;
  }
  .vote.up {
    color: #7ec97e;
  }
  .vote.down {
    color: #f08a8a;
  }
  .vote.none {
    opacity: 0.45;
  }
  .who {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tag {
    flex: none;
    padding: 0 4px;
    border-radius: 6px;
    font-size: 10px;
    background: color-mix(in srgb, var(--fg, #ddd) 12%, transparent);
  }
  .tag.req {
    color: #8ab4f0;
    background: color-mix(in srgb, #8ab4f0 16%, transparent);
  }
  .tag.stale {
    color: #e0b060;
    background: color-mix(in srgb, #e0b060 16%, transparent);
  }
  .acts {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding: 6px 8px;
  }
  .acts button.primary {
    border-color: #7ec97e;
    color: #7ec97e;
  }
  .vtable {
    flex: none;
    max-height: 34%;
    overflow: auto;
  }
  .vtable table {
    width: 100%;
    border-collapse: collapse;
  }
  .vtable th,
  .vtable td {
    padding: 2px 6px;
    text-align: left;
    white-space: nowrap;
  }
  .vtable th {
    position: sticky;
    top: 0;
    background: var(--bg, #1e1e1e);
    font-weight: 500;
    font-size: 11px;
    opacity: 0.7;
  }
  .vtable tr.baserow td {
    border-bottom: 1px solid var(--line, #2a2a2a);
    font-style: italic;
  }
  .vtable tr.sel {
    background: color-mix(in srgb, var(--accent, #4a7) 14%, transparent);
  }
  .vtable .pick {
    width: 3rem;
    white-space: nowrap;
  }
  .chip {
    display: inline-block;
    min-width: 1em;
    padding: 0 3px;
    border-radius: 3px;
    font-size: 10px;
    text-align: center;
    background: color-mix(in srgb, var(--accent, #4a7) 30%, transparent);
  }
  .chip.b {
    background: color-mix(in srgb, #8ab4f0 30%, transparent);
  }
  /* The two sides of the comparison, as Swarm does it: one place to look, and
     an option that would invert the diff is greyed rather than absent. */
  .head.pick2 {
    gap: 6px;
    display: flex;
    align-items: center;
  }
  .head.pick2 .lbl {
    text-transform: none;
    letter-spacing: normal;
    opacity: 0.7;
  }
  .head.pick2 select {
    font: inherit;
    font-size: 11px;
    text-transform: none;
    letter-spacing: normal;
  }
  .vtable .n {
    width: 2.5rem;
  }
  .vtable .ch {
    width: 6rem;
  }
  .lower {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .filesbox {
    display: flex;
    flex-direction: column;
    min-width: 0;
    min-height: 0;
  }
  .gutter {
    flex: none;
    width: 5px;
    cursor: col-resize;
    background: var(--line, #2a2a2a);
  }
  .gutter:hover {
    background: var(--accent, #4a7);
  }
  .files {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }
  .frow.selfile {
    background: color-mix(in srgb, var(--accent, #4a7) 18%, transparent);
  }
  .cbadge {
    flex: none;
    margin-left: auto;
    padding: 0 4px;
    border-radius: 6px;
    font-size: 10px;
    background: color-mix(in srgb, var(--fg, #ddd) 14%, transparent);
  }
  .cbadge.hastask {
    color: #e0b060;
    background: color-mix(in srgb, #e0b060 18%, transparent);
  }
  .cbadge + .size {
    margin-left: 6px;
  }
  .frow {
    display: flex;
    align-items: baseline;
    gap: 8px;
    width: 100%;
    padding: 1px 8px;
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    text-align: left;
    cursor: default;
  }
  .frow:hover {
    background: color-mix(in srgb, var(--fg, #ddd) 8%, transparent);
  }
  .act {
    flex: none;
    width: 4.8rem;
    font-size: 11px;
    opacity: 0.8;
  }
  .st-changed .act {
    color: #e0b060;
  }
  .st-added .act {
    color: #7ec97e;
  }
  .st-removed .act {
    color: #f08a8a;
  }
  .st-same .act {
    opacity: 0.45;
  }
  .fname {
    flex: none;
  }
  .fpath {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .size {
    margin-left: auto;
    flex: none;
    font-size: 11px;
  }
  .hint {
    flex: none;
    padding: 3px 8px;
    border-top: 1px solid var(--line, #2a2a2a);
  }
</style>
