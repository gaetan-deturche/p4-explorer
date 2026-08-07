<script lang="ts">
  //! One file's revision history, in its own window.
  //!
  //! Opened by the Rust `open_file_history_window` command from anywhere a file
  //! is shown. It runs its own p4 commands, so it fetches the job (connection +
  //! depot path) by id rather than taking a connection through the URL.
  //!
  //! The list itself is the History tab's own `history` store and `HistoryTable`
  //! — same rows, same columns, same have-revision anchor. Only the actions
  //! differ, and they are the three that answer "what happened to this file":
  //! see the change, take it back out, or go to it.
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import HistoryTable from "$lib/components/HistoryTable.svelte";
  import ChangeDetails from "$lib/components/ChangeDetails.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import ApprovalDialog from "$lib/components/ApprovalDialog.svelte";
  import { history } from "$lib/history.svelte";
  import { editor } from "$lib/editor.svelte";
  import { openDiff } from "$lib/opendiff";
  import {
    p4,
    emptyConn,
    setClipboard,
    openFileHistoryWindow,
    type P4Conn,
    type UndoResult,
  } from "$lib/p4";

  const id = new URLSearchParams(window.location.search).get("id") ?? "";

  let conn = $state<P4Conn>(emptyConn());
  let file = $state("");
  let error = $state("");
  let notice = $state("");
  let busy = $state(false);

  function setNotice(m: string, ms = 4000) {
    notice = m;
    window.setTimeout(() => (notice = ""), ms);
  }

  // --- confirmation ----------------------------------------------------------
  // Same shape as the main window's: a promise the dialog resolves.
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
      const job = await invoke<{ conn: P4Conn; depotFile: string }>("file_history_job", { id });
      conn = job.conn;
      file = job.depotFile;
      // The store reads the connection through hooks; paths here are already
      // depot paths, so the client-view translation is the identity.
      history.init({ conn: () => conn, setNotice, toQuery: (p: string) => p });
      // openDiff reads the diff-tool choice from this store; without init it is
      // never loaded here and every diff would open in-app whatever was chosen.
      void editor.init();
      await history.selectFile(file);
    } catch (e) {
      error = String(e);
    }
  });

  const rows = $derived(history.rows);
  /** The revision this changelist gave the file — file mode carries it per row;
   *  the `changes` fallback (servers whose filelog is unusable) does not. */
  function revOf(change: string): number {
    const r = rows.find((x) => String(x.change) === change);
    return r?.rev ? Number(r.rev) : 0;
  }

  // --- row actions -----------------------------------------------------------
  /** This revision against the one before it. */
  function diffRev(change: string) {
    const rev = revOf(change);
    if (!rev) {
      setNotice("This row has no revision number — the server answered with changelists only.", 6000);
      return;
    }
    void openDiff(conn, { kind: "rev", file, rev }, setNotice);
  }

  /** Take one file's share of a changelist back out: it is opened at the
   *  revision before that change, in a new pending changelist. Nothing is
   *  submitted. Used from both lists — the revisions of THIS file, and the other
   *  files the selected changelist touched. */
  async function undoFile(change: string, depotFile: string) {
    if (busy || !change) return;
    const go = await askConfirm(
      `Undo the change @${change} made to this file?\n\n${depotFile}\n\nIt will be opened at the revision before that change, in a new pending changelist. Nothing is submitted — review it in the main window and submit it yourself.`,
      "Undo this revision",
      "Undo",
    );
    if (!go) return;
    busy = true;
    try {
      const res = await invoke<UndoResult>("p4_undo_change", { conn, change, files: [depotFile] });
      setNotice(
        `Opened in @${res.change}` +
          (res.needsResolve ? " — needs a resolve before submitting." : " — submit it from the main window."),
        12000,
      );
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  /** Put this exact revision in the workspace (forward or backward). */
  async function syncTo(change: string) {
    if (busy) return;
    const go = await askConfirm(
      `${file}\n\nSync this file to its state at @${change}? This can move it backward.`,
      `Sync to @${change}`,
      "Sync",
    );
    if (!go) return;
    busy = true;
    try {
      const n = await p4.syncStream(conn, [`${file}@${change}`]);
      setNotice(n ? `Synced to @${change}.` : `Already at @${change}.`);
      await history.selectFile(file); // the have-revision anchor moved
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  // --- context menu ----------------------------------------------------------
  let ctx = $state<{ x: number; y: number; change: string } | null>(null);
  function openCtx(change: string, e: MouseEvent) {
    if (!change) return;
    history.selectChange(change); // highlight the row the menu belongs to
    ctx = { x: e.clientX, y: e.clientY, change };
  }
  function menuItems(change: string) {
    return [
      { label: "Diff against previous revision", action: () => diffRev(change) },
      { label: "", sep: true },
      { label: `Sync file to @${change}…`, action: () => void syncTo(change) },
      { label: "", sep: true },
      { label: `Undo what @${change} did to this file…`, action: () => void undoFile(change, file) },
    ];
  }

  // --- the changelist's other files ------------------------------------------
  // A revision row answers "when", the details pane answers "with what else":
  // the rest of the changelist. Its files are right-clickable in turn, so one
  // history window is a way INTO the next.
  let fileCtx = $state<{ x: number; y: number; file: string; rev: number } | null>(null);
  function fileMenu(depotFile: string, rev: number) {
    const change = history.selectedChange;
    return [
      { label: "Diff against previous revision", action: () => void openDiff(conn, { kind: "rev", file: depotFile, rev }, setNotice) },
      ...(depotFile === file
        ? []
        : [{ label: "File history…", action: () => void openFileHistoryWindow(conn, depotFile).catch((e) => (error = String(e))) }]),
      { label: "", sep: true },
      { label: "Copy depot path", action: () => void setClipboard(depotFile).then(() => setNotice("Copied depot path.", 2500)) },
      { label: "", sep: true },
      {
        label: `Undo what @${change} did to this file…`,
        disabled: !change,
        action: () => void undoFile(change, depotFile),
      },
    ];
  }

  // --- split between the revisions and the changelist ---------------------------
  let listW = $state(0.55); // fraction of the width given to the revision list
  let splitEl = $state<HTMLDivElement | undefined>();
  function splitDown(e: PointerEvent) {
    const host = splitEl?.parentElement;
    if (!host) return;
    (e.target as HTMLElement).setPointerCapture(e.pointerId);
    const box = host.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      listW = Math.min(0.85, Math.max(0.2, (ev.clientX - box.left) / box.width));
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }

  function close() {
    void getCurrentWindow().close();
  }
  function onWindowKey(e: KeyboardEvent) {
    if (e.key === "Escape" && !confirmState && !ctx) close();
  }
</script>

<svelte:window onkeydown={onWindowKey} />

<div class="wrap">
  <div class="bar">
    <span class="name mono" title={file}>{file}</span>
    <span class="grow"></span>
    {#if busy}<span class="dim">working…</span>{/if}
    <button onclick={close}>Close</button>
  </div>

  {#if error}
    <div class="err mono">{error}</div>
  {/if}
  {#if notice}
    <div class="note">{notice}</div>
  {/if}

  <div class="split">
    <div class="list" style="flex: 0 0 {listW * 100}%">
      <HistoryTable
        mode={history.mode}
        subject={history.subject}
        rows={history.rows}
        loading={history.loading}
        more={history.more}
        haveChange={history.haveChange}
        haveRev={history.haveRev}
        selectedChange={history.selectedChange}
        onSelectChange={(c) => history.selectChange(c)}
        onContextMenu={openCtx}
        onDeepen={() => history.deepen()}
        deepening={history.deepening}
      />
    </div>
    <div
      class="gutter"
      role="separator"
      aria-orientation="vertical"
      bind:this={splitEl}
      onpointerdown={splitDown}
    ></div>
    <div class="details">
      <ChangeDetails
        change={history.selectedChange}
        rows={history.descRows}
        loading={history.descLoading}
        onDiff={(f, r) => history.fileDiff(f, r)}
        onOpenDiff={(f, r) => history.openFileDiff(f, r)}
        onFileContext={(f, e) =>
          (fileCtx = { x: e.clientX, y: e.clientY, file: f.depotFile, rev: Number(f.rev) })}
      />
    </div>
  </div>
</div>

{#if ctx}
  <ContextMenu x={ctx.x} y={ctx.y} items={menuItems(ctx.change)} onClose={() => (ctx = null)} />
{/if}

{#if fileCtx}
  <ContextMenu
    x={fileCtx.x}
    y={fileCtx.y}
    items={fileMenu(fileCtx.file, fileCtx.rev)}
    onClose={() => (fileCtx = null)}
  />
{/if}

<!-- Safe mode queues its approvals per window: without this, an Undo or Sync
     from here would wait forever on a dialog only the main window renders. -->
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
  .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .grow {
    flex: 1;
  }
  .dim {
    opacity: 0.6;
  }
  .mono {
    font-family: var(--mono, ui-monospace, Consolas, monospace);
  }
  .err,
  .note {
    flex: none;
    padding: 6px 10px;
    font-size: 12px;
  }
  .err {
    color: #f08a8a;
    background: color-mix(in srgb, #f08a8a 12%, transparent);
  }
  .note {
    color: var(--fg, #ddd);
    background: color-mix(in srgb, var(--fg, #ddd) 8%, transparent);
  }
  .split {
    flex: 1;
    min-height: 0;
    display: flex;
  }
  .list,
  .details {
    min-width: 0;
    min-height: 0;
    display: flex;
    overflow: hidden;
  }
  .details {
    flex: 1;
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
</style>
