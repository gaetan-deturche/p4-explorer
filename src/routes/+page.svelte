<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { isReleaseBuild, emptyConn, firstLine, type P4Conn, type P4Record } from "$lib/p4";
  import { updates } from "$lib/updates.svelte";
  import { sync } from "$lib/sync.svelte";
  import { pending } from "$lib/pending.svelte";
  import { history } from "$lib/history.svelte";
  import { browse } from "$lib/browse.svelte";
  import { connection } from "$lib/connection.svelte";
  import { cmdlog } from "$lib/cmdlog.svelte";
  import {
    loadLastServer,
    loadUserFor,
    loadCharsetFor,
    saveView,
    loadViews,
    saveViews,
    type Views,
  } from "$lib/nav";
  import MenuBar from "$lib/components/MenuBar.svelte";
  import Toolbar from "$lib/components/Toolbar.svelte";
  import StatusBar from "$lib/components/StatusBar.svelte";
  import OptionsDialog from "$lib/components/OptionsDialog.svelte";
  import ConfirmDialog from "$lib/components/ConfirmDialog.svelte";
  import InputDialog from "$lib/components/InputDialog.svelte";
  import LoginDialog from "$lib/components/LoginDialog.svelte";
  import ApprovalDialog from "$lib/components/ApprovalDialog.svelte";
  import NewWorkspaceDialog from "$lib/components/NewWorkspaceDialog.svelte";
  import SyncProgressDialog from "$lib/components/SyncProgressDialog.svelte";
  import SyncErrorDialog from "$lib/components/SyncErrorDialog.svelte";
  import UpdateDialog from "$lib/components/UpdateDialog.svelte";
  import DepotTree from "$lib/components/DepotTree.svelte";
  import HistoryTable from "$lib/components/HistoryTable.svelte";
  import PendingList from "$lib/components/PendingList.svelte";
  import CommandLog from "$lib/components/CommandLog.svelte";
  import StreamsBrowser from "$lib/components/StreamsBrowser.svelte";
  import ChangeDetails from "$lib/components/ChangeDetails.svelte";
  import ContextMenu from "$lib/components/ContextMenu.svelte";

  // `conn` stays here (two-way bound by Toolbar/OptionsDialog); all connection
  // logic + derived state (connected/busy/clients/servers) lives in the store.
  let conn = $state<P4Conn>(emptyConn());
  let syncing = $state(false);
  let reconciling = $state(false);
  let optionsOpen = $state(false);
  let ctxMenu = $state<{ x: number; y: number; change: string } | null>(null);
  let streamCtx = $state<{ x: number; y: number; stream: string } | null>(null);
  let pendingCtx = $state<{ x: number; y: number; cl: P4Record } | null>(null);
  let treeCtx = $state<{ x: number; y: number; path: string; isDir: boolean } | null>(null);

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

  // Login prompt (user + password), promise-based like askConfirm.
  type Cred = { user: string; password: string };
  let loginState = $state<{
    user: string;
    port: string;
    resolve: (v: Cred | null) => void;
  } | null>(null);
  function promptLogin(port: string, user: string): Promise<Cred | null> {
    return new Promise((resolve) => (loginState = { user, port, resolve }));
  }
  function resolveLogin(v: Cred | null) {
    loginState?.resolve(v);
    loginState = null;
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
  let appVersion = $state("");
  let isRelease = $state(false); // dev/local builds skip auto-update and show -dev

  // Server selector UI (the list + switching live in the connection store).
  let serverCtx = $state<{ x: number; y: number } | null>(null);
  let addServerOpen = $state(false);
  let newWorkspaceOpen = $state(false);

  // Center tab. History/details pane lives in $lib/history.svelte.ts; the depot
  // tree, streams/repo tabs and index live in $lib/browse.svelte.ts.
  let centerTab = $state<"history" | "pending" | "streams" | "repo" | "log">("pending");

  const centerRows = $derived(centerTab === "pending" ? pending.rows : history.rows);

  // --- closable views (Depot pane + center tabs), persisted; re-shown via the
  //     View menu. Depot and Streams are hidden by default.
  let views = $state<Views>(loadViews());
  $effect(() => saveViews(views));
  const TABS: { key: "history" | "pending" | "streams" | "repo" | "log"; label: string }[] = [
    { key: "history", label: "History" },
    { key: "pending", label: "Pending" },
    { key: "streams", label: "Streams" },
    { key: "repo", label: "Depot" },
    { key: "log", label: "Commands" },
  ];
  // Show a center tab (and load its data). History uses the current selection.
  function showTab(key: (typeof TABS)[number]["key"]) {
    centerTab = key;
    if (key === "pending") pending.load();
    else if (key === "streams") browse.loadStreams();
    else if (key === "repo") browse.openRepo();
  }
  // Keep centerTab on a visible tab; if the active one was closed, pick another.
  $effect(() => {
    if (!views[centerTab]) {
      const next = TABS.find((t) => views[t.key]);
      if (next) centerTab = next.key;
    }
  });
  function closeTab(key: (typeof TABS)[number]["key"]) {
    views[key] = false;
    if (centerTab === key) {
      const next = TABS.find((t) => views[t.key]);
      if (next) centerTab = next.key;
    }
  }
  // Toggle a view from the View menu; re-showing a center tab focuses it.
  function toggleView(key: keyof Views) {
    if (views[key]) {
      if (key === "files") views.files = false;
      else closeTab(key);
    } else {
      views[key] = true;
      if (key !== "files") showTab(key as (typeof TABS)[number]["key"]);
    }
  }

  // Persist the current workspace's view (tab + selection) on every change, so a
  // restart / workspace switch returns here. selectClient reads this back before
  // it mutates state, so the read always beats this save.
  $effect(() => {
    const client = conn.client;
    const view = { tab: centerTab, treePath: browse.selectedTreePath, histMode: history.mode };
    if (connection.connected && client) saveView(client, view);
  });

  onDestroy(() => connection.stopKeepAlive());

  // --- pending: context/dialog glue over the `pending` store -----------------
  function onPendingContext(cl: P4Record, e: MouseEvent) {
    pendingCtx = { x: e.clientX, y: e.clientY, cl };
  }

  // Build the context-menu items for a pending changelist.
  function pendingMenuItems(cl: P4Record) {
    const own = cl.user === conn.user;
    const isDefault = cl.change === "default";
    // Real Swarm review status (the #review description marker is unreliable —
    // Swarm links by change and doesn't rewrite the pending CL's description).
    const hasReview = !!pending.reviews[cl.change];
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
    items.push({ label: "Generate patch…", action: () => generatePatch(cl.change, []) });
    return items;
  }

  // --- pending FILE context (local/opened files) -----------------------------
  let fileCtx = $state<{
    x: number;
    y: number;
    file: P4Record;
    change: string;
    files: string[];
  } | null>(null);
  let newClFile = $state<string | null>(null); // a file awaiting a new-changelist name
  let renameCl = $state<{ change: string; desc: string } | null>(null); // CL being renamed

  // PendingList instance, for the optimistic file move shared with drag-and-drop.
  let pendingList = $state<{ moveFile: (file: string, from: string, to: string) => void }>();

  function onPendingFileContext(file: P4Record, change: string, e: MouseEvent, files: string[]) {
    fileCtx = { x: e.clientX, y: e.clientY, file, change, files };
  }
  const generatePatch = (change: string, files: string[]) => pending.generatePatch(change, files);
  // Move via the context menu, optimistically (falls back to a plain reopen if
  // the list isn't mounted for some reason).
  function moveFileTo(file: string, from: string, to: string) {
    if (pendingList) pendingList.moveFile(file, from, to);
    else pending.reopen(file, to);
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

  // Right-click menu for a pending file: view/revert, un-open, patch, or move.
  // `files` is the current selection (≥1); single-file actions use the clicked
  // file, the patch action uses the whole selection.
  function fileMenuItems(file: P4Record, change: string, files: string[]) {
    const targets = pending.rows
      .filter((cl) => cl.change !== change)
      .map((cl) => {
        const desc = firstLine(cl.desc);
        const short = desc.length > 32 ? desc.slice(0, 31) + "…" : desc;
        const label =
          cl.change === "default" ? "Default" : short ? `@${cl.change}  ${short}` : "@" + cl.change;
        return { label, action: () => moveFileTo(file.depotFile, change, cl.change) };
      });
    targets.push({ label: "New changelist…", action: () => (newClFile = file.depotFile) });
    const patchLabel = files.length > 1 ? `Generate patch (${files.length} files)…` : "Generate patch…";
    return [
      { label: "View diff", action: () => pending.openLocalDiff(file.depotFile) },
      { label: patchLabel, action: () => generatePatch("", files.length ? files : [file.depotFile]) },
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
    history.selectChange(change); // highlight the right-clicked row
    ctxMenu = { x: e.clientX, y: e.clientY, change };
  }

  // --- depot tree: right-click to sync / reconcile just this path ------------
  function onTreeContext(path: string, isDir: boolean, e: MouseEvent) {
    if (!connection.connected || !conn.client) return;
    treeCtx = { x: e.clientX, y: e.clientY, path, isDir };
  }

  // --- Streams tab: switching reconfigures the current workspace --------------
  function onStreamContext(stream: string, e: MouseEvent) {
    if (!connection.connected || !conn.client) return;
    if (!stream || stream === browse.rootPath) return; // already on it
    streamCtx = { x: e.clientX, y: e.clientY, stream };
  }

  async function exitApp() {
    const { getCurrentWindow } = await import("@tauri-apps/api/window");
    await getCurrentWindow().close();
  }

  function showAbout() {
    const ver = appVersion ? (isRelease ? " v" + appVersion : " " + appVersion + "-dev") : "";
    notice = `Auger${ver}${connection.serverVersion ? " · server " + connection.serverVersion : ""}`;
    window.setTimeout(() => (notice = ""), 6000);
  }

  onMount(() => {
    cmdlog.start(); // record p4 commands for the Commands view
    history.init({
      conn: () => conn,
      setNotice,
      toQuery: (p) => browse.toQuery(p),
    });
    browse.init({
      conn: () => conn,
      connected: () => connection.connected,
      getTab: () => centerTab,
      setTab: (t) => (centerTab = t),
    });
    connection.init({
      conn: () => conn,
      getTab: () => centerTab,
      setConnError: (m) => (error = m),
      setNotice,
      setOptionsOpen: (v) => (optionsOpen = v),
      getSyncing: () => syncing,
      setSyncing: (v) => (syncing = v),
      askConfirm,
      promptLogin,
    });
    updates.init({
      isRelease: () => isRelease,
      appVersion: () => appVersion,
      notify: (m) => setNotice(m),
      warn: (m) => setError(m),
    });
    pending.init({
      conn: () => conn,
      connected: () => connection.connected,
      syncing: () => syncing,
      setSyncing: (v) => (syncing = v),
      setNotice,
      setError,
      askConfirm,
      refresh: () => browse.refresh(),
    });
    sync.init({
      conn: () => conn,
      connected: () => connection.connected,
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
    // Reconnect to the server used last session (with its remembered user);
    // connect() then restores that server's last workspace and saved view.
    const last = loadLastServer();
    if (last) {
      conn.port = last;
      conn.user = loadUserFor(last);
      conn.charset = loadCharsetFor(last);
    }
    connection.connect();
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
    connected={connection.connected}
    refreshing={browse.refreshing}
    {syncing}
    {views}
    onOptions={() => (optionsOpen = true)}
    onReconnect={() => connection.connect()}
    onExit={exitApp}
    onRefresh={() => browse.refresh()}
    onSync={() => sync.globalSync()}
    onNewWorkspace={() => (newWorkspaceOpen = true)}
    onToggleView={toggleView}
    onAbout={showAbout}
    onCheckUpdates={() => updates.check(false)}
  />
  <Toolbar
    bind:conn
    clients={connection.clients}
    localClients={connection.localClients}
    servers={connection.servers}
    connected={connection.connected}
    refreshing={browse.refreshing}
    {syncing}
    {reconciling}
    onClientChange={(c) => connection.selectClient(c)}
    onNewWorkspace={() => (newWorkspaceOpen = true)}
    onServerChange={(p) => connection.switchServerTo(p)}
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
    {#if views.files}
      <section class="col left" style="width:{leftW}px">
        <div class="panehdr">
          <span>Files</span>
          <button class="paneclose" title="Close view" onclick={() => (views.files = false)}>✕</button>
        </div>
        <DepotTree
          root={browse.tree}
          selectedPath={browse.selectedTreePath}
          indexing={browse.indexing}
          onSelect={(n) => browse.selectNode(n)}
          onExpand={(n) => browse.expandNode(n)}
          onSearch={(t) => browse.searchDepot(t)}
          onOpenResult={(f) => browse.openResult(f)}
          onContext={(n, e) => onTreeContext(n.path, n.isDir, e)}
        />
      </section>

      <div
        class="gutter"
        role="separator"
        aria-orientation="vertical"
        onpointerdown={(e) => startResize(e, "left")}
      ></div>
    {/if}

    <section class="col center">
      <div class="tabs">
        {#each TABS.filter((t) => views[t.key]) as t (t.key)}
          <div class="tab" class:active={centerTab === t.key}>
            <button class="tablabel" onclick={() => showTab(t.key)}>{t.label}</button>
            <button
              class="tabclose"
              title="Close view"
              onclick={(e) => {
                e.stopPropagation();
                closeTab(t.key);
              }}>✕</button
            >
          </div>
        {/each}
      </div>
      {#if !TABS.some((t) => views[t.key])}
        <div class="msg dim">All views are closed — reopen one from the View menu.</div>
      {:else if centerTab === "streams"}
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
      {:else if centerTab === "log"}
        <CommandLog entries={cmdlog.entries} onClear={() => cmdlog.clear()} />
      {:else if centerTab === "pending"}
        <PendingList
          bind:this={pendingList}
          rows={pending.rows}
          loading={pending.loading}
          client={conn.client}
          refreshKey={pending.version}
          reviews={pending.reviews}
          contextChange={pendingCtx?.cl.change ?? ""}
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

  <StatusBar
    connected={connection.connected}
    serverVersion={connection.serverVersion}
    {appVersion}
    {isRelease}
    busy={connection.busy}
    onConnect={() => connection.connect()}
  />
</div>

{#if optionsOpen}
  <OptionsDialog
    bind:conn
    busy={connection.busy}
    servers={connection.servers}
    onConnect={() => connection.connect()}
    onSelectServer={(p) => connection.switchServerTo(p)}
    onRelogin={(p) => connection.relogin(p)}
    onForget={(p) => connection.forgetServer(p)}
    onAdd={(p) => connection.addAndSwitch(p)}
    onClose={() => (optionsOpen = false)}
  />
{/if}

{#if loginState}
  <LoginDialog
    port={loginState.port}
    user={loginState.user}
    onSubmit={(c) => resolveLogin(c)}
    onCancel={() => resolveLogin(null)}
  />
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
    items={[{ label: `Switch workspace to ${stream}`, action: () => connection.switchStream(stream) }]}
    onClose={() => (streamCtx = null)}
  />
{/if}

{#if treeCtx}
  {@const p = treeCtx.path}
  {@const dir = treeCtx.isDir}
  {@const kind = dir ? "folder" : "file"}
  {@const name = p.replace(/\/+$/, "").split("/").pop() || p}
  <ContextMenu
    x={treeCtx.x}
    y={treeCtx.y}
    items={[
      { label: `Sync ${kind} “${name}”`, action: () => sync.syncPath(p, dir) },
      { label: `Reconcile ${kind} “${name}”`, action: () => sync.reconcilePath(p, dir) },
    ]}
    onClose={() => (treeCtx = null)}
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
    items={fileMenuItems(fileCtx.file, fileCtx.change, fileCtx.files)}
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
    items={[{ label: `Forget "${conn.port}"`, action: () => connection.forgetServer(conn.port) }]}
    onClose={() => (serverCtx = null)}
  />
{/if}

{#if addServerOpen}
  <InputDialog
    title="Add server"
    label="Server (P4PORT)"
    placeholder="ssl:host:1666"
    okLabel="Connect"
    onSubmit={(port) => {
      addServerOpen = false;
      connection.addAndSwitch(port);
    }}
    onCancel={() => (addServerOpen = false)}
  />
{/if}

{#if newWorkspaceOpen}
  <NewWorkspaceDialog
    initialStream={browse.rootPath}
    onSubmit={(w) => {
      newWorkspaceOpen = false;
      connection.createWorkspace(w.name, w.root, w.stream);
    }}
    onCancel={() => (newWorkspaceOpen = false)}
  />
{/if}

<ApprovalDialog />

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
    gap: 2px;
    padding: 6px 8px 0;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .tab {
    display: flex;
    align-items: center;
    border-bottom: 2px solid transparent;
  }
  .tab.active {
    border-bottom-color: var(--accent);
  }
  .tablabel {
    border: none;
    border-radius: 0;
    background: none;
    padding: 4px 2px 4px 12px;
    color: var(--text-dim);
  }
  .tab.active .tablabel {
    color: var(--text);
  }
  .tabclose {
    border: none;
    background: none;
    border-radius: 4px;
    padding: 0 5px;
    margin-right: 2px;
    color: var(--text-dim);
    font-size: 10px;
    line-height: 1;
    cursor: pointer;
  }
  .tabclose:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .panehdr {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 4px 6px 4px 10px;
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-dim);
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .paneclose {
    border: none;
    background: none;
    border-radius: 4px;
    padding: 0 6px;
    color: var(--text-dim);
    font-size: 10px;
    cursor: pointer;
  }
  .paneclose:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .center :global(.panel) {
    flex: 1;
    min-height: 0;
  }
  .msg {
    padding: 16px;
    font-size: 12px;
  }
</style>
