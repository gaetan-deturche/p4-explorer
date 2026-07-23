<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import {
    p4,
    isReleaseBuild,
    emptyConn,
    firstLine,
    type P4Conn,
    type P4Record,
  } from "$lib/p4";
  import { loadServers, saveServers, withServer, withoutServer } from "$lib/servers";
  import { updates } from "$lib/updates.svelte";
  import { sync } from "$lib/sync.svelte";
  import { pending } from "$lib/pending.svelte";
  import { history } from "$lib/history.svelte";
  import { browse } from "$lib/browse.svelte";
  import MenuBar from "$lib/components/MenuBar.svelte";
  import Toolbar from "$lib/components/Toolbar.svelte";
  import StatusBar from "$lib/components/StatusBar.svelte";
  import OptionsDialog from "$lib/components/OptionsDialog.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import InputDialog from "$lib/components/InputDialog.svelte";
  import SyncProgressDialog from "$lib/components/SyncProgressDialog.svelte";
  import SyncErrorDialog from "$lib/components/SyncErrorDialog.svelte";
  import UpdateDialog from "$lib/components/UpdateDialog.svelte";
  import DepotTree from "$lib/components/DepotTree.svelte";
  import HistoryTable from "$lib/components/HistoryTable.svelte";
  import PendingList from "$lib/components/PendingList.svelte";
  import StreamsBrowser from "$lib/components/StreamsBrowser.svelte";
  import ChangeDetails from "$lib/components/ChangeDetails.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";

  let conn = $state<P4Conn>(emptyConn());
  let connected = $state(false);
  let busy = $state(false);
  let syncing = $state(false);
  let reconciling = $state(false);
  let optionsOpen = $state(false);
  let ctxMenu = $state<{ x: number; y: number; change: string } | null>(null);
  let streamCtx = $state<{ x: number; y: number; stream: string } | null>(null);
  let pendingCtx = $state<{ x: number; y: number; cl: P4Record } | null>(null);

  // In-app confirm dialog (replaces window.confirm).
  let confirmState = $state<{
    title: string;
    message: string;
    okLabel: string;
    resolve: (v: boolean) => void;
  } | null>(null);
  function askConfirm(message: string, title = "Confirm", okLabel = "OK"): Promise<boolean> {
    return new Promise((resolve) => (confirmState = { title, message, okLabel, resolve }));
  }
  function resolveConfirm(v: boolean) {
    confirmState?.resolve(v);
    confirmState = null;
  }

  // Transient status helpers (auto-clear).
  function setNotice(m: string, ms = 4000) {
    notice = m;
    window.setTimeout(() => (notice = ""), ms);
  }
  function setError(m: string, ms = 6000) {
    error = m;
    window.setTimeout(() => (error = ""), ms);
  }

  // Feature stores wired in onMount: sync ($lib/sync.svelte.ts), history, browse,
  // pending, updates. Depot tree / streams / repo / index state lives in browse.

  // Resizable widths: tree pane (left) and the changelist-details pane that
  // lives on the right INSIDE the History tab.
  let leftW = $state(300);
  let detailsW = $state(400);
  function startResize(e: PointerEvent, which: "left" | "details") {
    e.preventDefault();
    const startX = e.clientX;
    const startL = leftW;
    const startD = detailsW;
    const move = (ev: PointerEvent) => {
      const dx = ev.clientX - startX;
      if (which === "left") leftW = Math.max(160, startL + dx);
      else detailsW = Math.max(220, startD - dx); // drag left → wider details
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }
  let error = $state("");
  let notice = $state(""); // transient info (e.g. sync result)
  let serverVersion = $state("");
  let appVersion = $state("");
  let isRelease = $state(false); // dev/local builds skip auto-update and show -dev
  let clients = $state<P4Record[]>([]);

  // Server selector: remembered connections (localStorage) + seeded from p4 env.
  let servers = $state<string[]>([]);
  let serverCtx = $state<{ x: number; y: number } | null>(null);
  let addServerOpen = $state(false);
  function rememberServer(port: string) {
    const next = withServer(servers, port);
    if (next !== servers) {
      servers = next;
      saveServers(servers);
    }
  }
  function forgetServer(port: string) {
    servers = withoutServer(servers, port);
    saveServers(servers);
  }
  async function switchServerTo(port: string) {
    if (port === conn.port) return;
    conn.port = port;
    conn.client = "";
    browse.reset();
    await connect();
  }
  function submitAddServer(port: string) {
    addServerOpen = false;
    const v = port.trim();
    if (!v) return;
    rememberServer(v);
    switchServerTo(v);
  }

  // Center tab. History/details pane lives in $lib/history.svelte.ts; the depot
  // tree, streams/repo tabs and index live in $lib/browse.svelte.ts.
  let centerTab = $state<"history" | "pending" | "streams" | "repo">("pending");

  const centerRows = $derived(centerTab === "pending" ? pending.rows : history.rows);

  // --- connection warm-keeping ------------------------------------------------
  // Each `p4` CLI call reconnects; the first `dirs` on a huge stream root is a
  // ~2.7s server-side cold disk read of db.rev. Re-running it periodically keeps
  // that cache hot so it doesn't recur mid-session.
  let keepAliveId: number | null = null;
  function startKeepAlive() {
    if (keepAliveId !== null) clearInterval(keepAliveId);
    // Doubles as a health check: if the server becomes unreachable the status
    // flips to disconnected (and recovers automatically when it returns), so
    // the UI never shows a stale "connected" while operations silently fail.
    keepAliveId = window.setInterval(async () => {
      try {
        await p4.info(conn);
        if (!connected) {
          connected = true; // recovered
          error = "";
        }
        if (browse.rootPath) p4.dirs(conn, browse.rootPath).catch(() => {}); // keep cache warm
      } catch (e) {
        if (connected) {
          connected = false;
          error = "Lost connection to the Perforce server. Retrying…";
        }
      }
    }, 20000);
  }
  onDestroy(() => {
    if (keepAliveId !== null) clearInterval(keepAliveId);
  });

  // --- connection / workspace -------------------------------------------------
  async function connect() {
    busy = true;
    error = "";
    try {
      const info = await p4.info(conn);
      const i = info[0] ?? {};
      serverVersion = i.serverVersion ?? "";
      if (!conn.user && i.userName) conn.user = i.userName;
      // Seed the server dropdown: adopt the ambient P4PORT if none was set.
      if (!conn.port) {
        const env = await p4.envPort(conn).catch(() => "");
        if (env) conn.port = env;
      }
      rememberServer(conn.port);
      connected = true;
      optionsOpen = false;
      startKeepAlive();
      clients = await p4.clients(conn);
      const cn = i.clientName;
      if (cn && cn !== "*unknown*" && clients.some((c) => c.client === cn)) {
        conn.client = cn;
        await selectClient();
      }
    } catch (e) {
      connected = false;
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function selectClient() {
    const tab = centerTab; // keep the user's current tab across the workspace change
    error = "";
    browse.reset();
    const rec = clients.find((c) => c.client === conn.client);
    if (!rec) return;
    if (!rec.Stream) {
      error = "This workspace has no stream. Depot browsing currently requires a stream client.";
      return;
    }
    await browse.openWorkspace(rec.Stream, rec.Root ?? "", tab);
  }

  // --- pending: context/dialog glue over the `pending` store -----------------
  function onPendingContext(cl: P4Record, e: MouseEvent) {
    pendingCtx = { x: e.clientX, y: e.clientY, cl };
  }

  // Build the context-menu items for a pending changelist.
  function pendingMenuItems(cl: P4Record) {
    const own = cl.user === conn.user;
    const isDefault = cl.change === "default";
    const hasReview = (cl.desc ?? "").includes("#review");
    const items: { label: string; action: () => void }[] = [];
    if (own) {
      items.push({
        label: isDefault ? "Submit default changelist…" : `Submit @${cl.change}…`,
        action: () => pending.submit(cl.change),
      });
    }
    if (own && !isDefault) {
      items.push({
        label: "Rename…",
        action: () => (renameCl = { change: cl.change, desc: (cl.desc ?? "").trim() }),
      });
    }
    if (own && !isDefault) {
      if (hasReview) items.push({ label: "Update review", action: () => pending.updateReview(cl.change) });
      else items.push({ label: "Request review", action: () => pending.requestReview(cl.change) });
    }
    if (!isDefault) {
      items.push({ label: "Open review in browser", action: () => pending.openReview(cl.change) });
    }
    if (own && !isDefault) {
      items.push({ label: "Delete shelf", action: () => pending.deleteShelf(cl.change) });
    }
    return items;
  }

  // --- pending FILE context (local/opened files) -----------------------------
  let fileCtx = $state<{ x: number; y: number; file: P4Record; change: string } | null>(null);
  let newClFile = $state<string | null>(null); // a file awaiting a new-changelist name
  let renameCl = $state<{ change: string; desc: string } | null>(null); // CL being renamed

  function onPendingFileContext(file: P4Record, change: string, e: MouseEvent) {
    fileCtx = { x: e.clientX, y: e.clientY, file, change };
  }
  function submitNewChangelist(desc: string) {
    const file = newClFile;
    newClFile = null;
    if (file) pending.moveToNew(file, desc);
  }
  function submitRename(desc: string) {
    const target = renameCl;
    renameCl = null;
    if (target) pending.rename(target.change, desc);
  }

  // Right-click menu for a pending file: view/revert, un-open, or move to a CL.
  function fileMenuItems(file: P4Record, change: string) {
    const targets = pending.rows
      .filter((cl) => cl.change !== change)
      .map((cl) => {
        const desc = firstLine(cl.desc);
        const short = desc.length > 32 ? desc.slice(0, 31) + "…" : desc;
        const label =
          cl.change === "default" ? "Default" : short ? `@${cl.change}  ${short}` : "@" + cl.change;
        return { label, action: () => pending.reopen(file.depotFile, cl.change) };
      });
    targets.push({ label: "New changelist…", action: () => (newClFile = file.depotFile) });
    return [
      { label: "View diff", action: () => pending.openLocalDiff(file.depotFile) },
      { label: "Revert file…", action: () => pending.revert(file.depotFile) },
      {
        label: "Remove from changelist (keep changes)…",
        action: () => pending.revertKeep(file.depotFile),
      },
      { label: "Move to changelist", submenu: targets },
    ];
  }

  // Switch to the Pending tab and (re)load it.
  function openPending() {
    centerTab = "pending";
    pending.load();
  }

  // --- history row context menu: "update to this changelist" ----------------
  function openHistContext(change: string, e: MouseEvent) {
    if (!change || centerTab !== "history") return;
    ctxMenu = { x: e.clientX, y: e.clientY, change };
  }

  // --- Streams tab: switching reconfigures the current workspace --------------
  function onStreamContext(stream: string, e: MouseEvent) {
    if (!connected || !conn.client) return;
    if (!stream || stream === browse.rootPath) return; // already on it
    streamCtx = { x: e.clientX, y: e.clientY, stream };
  }
  async function switchStream(stream: string) {
    if (!connected || syncing) return;
    if (
      !(await askConfirm(
        `${stream}\n\nThis reconfigures the workspace and syncs to that stream. Open files will block the switch — shelve or submit them first.`,
        "Switch workspace stream",
        "Switch",
      ))
    ) {
      return;
    }
    syncing = true;
    error = "";
    notice = "";
    try {
      await p4.switch(conn, stream); // throws (surfaced below) if p4 refuses
      clients = await p4.clients(conn); // pick up the client's new Stream
      await selectClient();
      notice = `Switched workspace to ${stream}.`;
      window.setTimeout(() => (notice = ""), 4000);
    } catch (e) {
      error = String(e);
    } finally {
      syncing = false;
    }
  }

  async function exitApp() {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().close();
  }

  function showAbout() {
    const ver = appVersion ? (isRelease ? " v" + appVersion : " " + appVersion + "-dev") : "";
    notice = `Auger${ver}${serverVersion ? " · server " + serverVersion : ""}`;
    window.setTimeout(() => (notice = ""), 6000);
  }

  onMount(() => {
    servers = loadServers();
    history.init({
      conn: () => conn,
      showHistoryTab: () => (centerTab = "history"),
      setNotice,
    });
    browse.init({
      conn: () => conn,
      connected: () => connected,
      getTab: () => centerTab,
      setTab: (t) => (centerTab = t),
    });
    updates.init({
      isRelease: () => isRelease,
      appVersion: () => appVersion,
      notify: (m) => setNotice(m),
      warn: (m) => setError(m),
    });
    pending.init({
      conn: () => conn,
      connected: () => connected,
      syncing: () => syncing,
      setSyncing: (v) => (syncing = v),
      setNotice,
      setError,
      askConfirm,
      refresh: () => browse.refresh(),
    });
    sync.init({
      conn: () => conn,
      connected: () => connected,
      busy: () => syncing || reconciling,
      setSyncing: (v) => (syncing = v),
      setReconciling: (v) => (reconciling = v),
      setNotice,
      setError,
      askConfirm,
      refresh: () => browse.refresh(),
      loadPending: () => pending.load(),
      rootPath: () => browse.rootPath,
      histSubject: () => history.subject,
      histMode: () => history.mode,
    });
    connect();
    getVersion()
      .then((v) => (appVersion = v))
      .catch(() => {});
    isReleaseBuild()
      .then((v) => {
        isRelease = v;
        if (v) updates.check(true); // silent check only on release builds
      })
      .catch(() => {});
  });
</script>

<div class="app">
  <MenuBar
    {connected}
    refreshing={browse.refreshing}
    {syncing}
    onOptions={() => (optionsOpen = true)}
    onReconnect={connect}
    onExit={exitApp}
    onRefresh={() => browse.refresh()}
    onSync={() => sync.globalSync()}
    onAbout={showAbout}
    onCheckUpdates={() => updates.check(false)}
  />
  <Toolbar
    bind:conn
    {clients}
    {servers}
    {connected}
    refreshing={browse.refreshing}
    {syncing}
    {reconciling}
    onClientChange={selectClient}
    onServerChange={switchServerTo}
    onAddServer={() => (addServerOpen = true)}
    onServerContext={(e) => {
      if (conn.port) serverCtx = { x: e.clientX, y: e.clientY };
    }}
    onRefresh={() => browse.refresh()}
    onSync={() => sync.globalSync()}
    onReconcile={() => sync.reconcile()}
  />

  {#if error}
    <div class="error mono">{error}</div>
  {/if}
  {#if notice}
    <div class="notice">{notice}</div>
  {/if}

  <div class="cols">
    <section class="col left" style="width:{leftW}px">
      <DepotTree
        root={browse.tree}
        selectedPath={browse.selectedTreePath}
        indexing={browse.indexing}
        onSelect={(n) => browse.selectNode(n)}
        onExpand={(n) => browse.expandNode(n)}
        onSearch={(t) => browse.searchDepot(t)}
        onOpenResult={(f) => browse.openResult(f)}
      />
    </section>

    <div
      class="gutter"
      role="separator"
      aria-orientation="vertical"
      onpointerdown={(e) => startResize(e, "left")}
    ></div>

    <section class="col center">
      <div class="tabs">
        <button class:active={centerTab === "history"} onclick={() => (centerTab = "history")}>
          History
        </button>
        <button class:active={centerTab === "pending"} onclick={openPending}>Pending</button>
        <button class:active={centerTab === "streams"} onclick={() => browse.loadStreams()}>
          Streams
        </button>
        <button class:active={centerTab === "repo"} onclick={() => browse.openRepo()}>Repo</button>
      </div>
      {#if centerTab === "streams"}
        <StreamsBrowser
          rows={browse.streamRows}
          loading={browse.streamsLoading}
          currentStream={browse.rootPath}
          onContext={onStreamContext}
        />
      {:else if centerTab === "repo"}
        <DepotTree
          root={browse.repoTree}
          selectedPath={browse.repoSelected}
          onSelect={(n) => browse.repoSelect(n)}
          onExpand={(n) => browse.repoExpand(n)}
        />
      {:else if centerTab === "pending"}
        <PendingList
          rows={pending.rows}
          loading={pending.loading}
          client={conn.client}
          onLocalFiles={pending.localFiles}
          onShelvedFiles={pending.shelvedFiles}
          onLocalDiff={pending.localDiff}
          onShelvedDiff={pending.shelvedDiff}
          onOpenLocalDiff={pending.openLocalDiff}
          onOpenShelvedDiff={pending.openShelvedDiff}
          onContext={onPendingContext}
          onFileContext={onPendingFileContext}
          onMoveFile={pending.reopen}
        />
      {:else}
        <div class="hsplit">
          <div class="hlist">
            <HistoryTable
              mode={history.mode}
              subject={history.subject}
              rows={centerRows}
              loading={history.loading}
              more={history.more}
              haveChange={history.haveChange}
              haveRev={history.haveRev}
              selectedChange={history.selectedChange}
              onSelectChange={(c) => history.selectChange(c)}
              onContextMenu={openHistContext}
            />
          </div>
          <div
            class="gutter"
            role="separator"
            aria-orientation="vertical"
            onpointerdown={(e) => startResize(e, "details")}
          ></div>
          <div class="hdetails" style="width:{detailsW}px">
            <ChangeDetails
              change={history.selectedChange}
              rows={history.descRows}
              loading={history.descLoading}
              onDiff={(f, r) => history.fileDiff(f, r)}
              onOpenDiff={(f, r) => history.openFileDiff(f, r)}
            />
          </div>
        </div>
      {/if}
    </section>
  </div>

  <StatusBar {connected} {serverVersion} {appVersion} {isRelease} {busy} onConnect={connect} />
</div>

{#if optionsOpen}
  <OptionsDialog bind:conn {busy} onConnect={connect} onClose={() => (optionsOpen = false)} />
{/if}

{#if confirmState}
  <ConfirmDialog
    title={confirmState.title}
    message={confirmState.message}
    okLabel={confirmState.okLabel}
    onOk={() => resolveConfirm(true)}
    onCancel={() => resolveConfirm(false)}
  />
{/if}

{#if sync.progress}
  <SyncProgressDialog
    title={sync.progress.title}
    count={sync.progress.count}
    current={sync.progress.current}
    issues={sync.progress.issues}
    issueLine={sync.progress.issueLine}
    phase={sync.progress.phase}
    message={sync.progress.message}
    onCancel={() => sync.cancel()}
    onClose={() => sync.dismissProgress()}
  />
{/if}

{#if sync.errors}
  <SyncErrorDialog
    title={sync.errors.title}
    items={sync.errors.items}
    busyFile={sync.busyFile}
    onFixFile={(f, force) => sync.fixFile(f, force)}
    onRetryAll={() => sync.fixAll(false)}
    onForceAll={() => sync.fixAll(true)}
    onClose={() => sync.dismissErrors()}
  />
{/if}

{#if ctxMenu}
  {@const change = ctxMenu.change}
  <ContextMenu
    x={ctxMenu.x}
    y={ctxMenu.y}
    items={[
      { label: `Update to changelist @${change}`, action: () => sync.updateToChange(change) },
    ]}
    onClose={() => (ctxMenu = null)}
  />
{/if}

{#if streamCtx}
  {@const stream = streamCtx.stream}
  <ContextMenu
    x={streamCtx.x}
    y={streamCtx.y}
    items={[{ label: `Switch workspace to ${stream}`, action: () => switchStream(stream) }]}
    onClose={() => (streamCtx = null)}
  />
{/if}

{#if pendingCtx}
  {@const items = pendingMenuItems(pendingCtx.cl)}
  {#if items.length}
    <ContextMenu x={pendingCtx.x} y={pendingCtx.y} {items} onClose={() => (pendingCtx = null)} />
  {/if}
{/if}

{#if fileCtx}
  <ContextMenu
    x={fileCtx.x}
    y={fileCtx.y}
    items={fileMenuItems(fileCtx.file, fileCtx.change)}
    onClose={() => (fileCtx = null)}
  />
{/if}

{#if newClFile !== null}
  <InputDialog
    title="New changelist"
    label="Description"
    placeholder="Describe the change…"
    okLabel="Create & move"
    onSubmit={submitNewChangelist}
    onCancel={() => (newClFile = null)}
  />
{/if}

{#if renameCl}
  <InputDialog
    title="Rename changelist @{renameCl.change}"
    label="Description"
    initial={renameCl.desc}
    multiline
    okLabel="Save"
    onSubmit={submitRename}
    onCancel={() => (renameCl = null)}
  />
{/if}

{#if serverCtx}
  <ContextMenu
    x={serverCtx.x}
    y={serverCtx.y}
    items={[{ label: `Forget "${conn.port}"`, action: () => forgetServer(conn.port) }]}
    onClose={() => (serverCtx = null)}
  />
{/if}

{#if addServerOpen}
  <InputDialog
    title="Add server"
    label="Server (P4PORT)"
    placeholder="ssl:host:1666"
    okLabel="Connect"
    onSubmit={submitAddServer}
    onCancel={() => (addServerOpen = false)}
  />
{/if}

{#if updates.state}
  <UpdateDialog
    version={updates.state.version}
    notes={updates.state.notes}
    phase={updates.state.phase}
    downloaded={updates.state.downloaded}
    total={updates.state.total}
    message={updates.state.message}
    onInstall={() => updates.install()}
    onDismiss={() => updates.dismiss()}
  />
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }
  .error {
    background: var(--warn);
    color: white;
    padding: 6px 12px;
    font-size: 12px;
    white-space: pre-wrap;
  }
  .notice {
    background: var(--have-bg);
    color: var(--have);
    border-bottom: 1px solid var(--have);
    padding: 5px 12px;
    font-size: 12px;
  }
  .cols {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .col {
    min-width: 0;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }
  .col.left {
    flex: none;
  }
  .col.center {
    flex: 1;
  }
  .gutter {
    flex: none;
    width: 5px;
    cursor: col-resize;
    background: var(--border);
  }
  .gutter:hover {
    background: var(--accent);
  }
  /* History tab: list on the left, changelist details on the right. */
  .hsplit {
    flex: 1;
    display: flex;
    min-height: 0;
  }
  .hlist {
    flex: 1;
    min-width: 0;
    display: flex;
    min-height: 0;
  }
  .hdetails {
    flex: none;
    min-width: 0;
    display: flex;
    min-height: 0;
  }
  .center {
    background: var(--bg-panel);
  }
  .tabs {
    display: flex;
    gap: 4px;
    padding: 6px 8px 0;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .tabs button {
    border: none;
    border-bottom: 2px solid transparent;
    border-radius: 0;
    background: none;
    padding: 4px 12px;
    color: var(--text-dim);
  }
  .tabs button.active {
    color: var(--text);
    border-bottom-color: var(--accent);
  }
  .center :global(.panel) {
    flex: 1;
    min-height: 0;
  }
</style>
