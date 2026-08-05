<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import {
    isReleaseBuild,
    emptyConn,
    firstLine,
    setClipboard,
    p4,
    type P4Conn,
    type P4Record,
    type ReviewRow,
  } from "$lib/p4";
  import { localPathFor } from "$lib/cache";
  import { updates } from "$lib/updates.svelte";
  import { sync, type SyncTarget } from "$lib/sync.svelte";
  import { patches } from "$lib/patch.svelte";
  import { merges, afterMerge } from "$lib/merge.svelte";
  import { pending } from "$lib/pending.svelte";
  import { reviews, type Role, type StatusFilter } from "$lib/reviews.svelte";
  import { history } from "$lib/history.svelte";
  import { browse } from "$lib/browse.svelte";
  import { connection } from "$lib/connection.svelte";
  import { cmdlog } from "$lib/cmdlog.svelte";
  import { notifications } from "$lib/notifications.svelte";
  import { editor } from "$lib/editor.svelte";
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
  import ApplyPatchDialog from "$lib/components/ApplyPatchDialog.svelte";
  import DepotTree from "$lib/components/DepotTree.svelte";
  import HistoryTable from "$lib/components/HistoryTable.svelte";
  import PendingList from "$lib/components/PendingList.svelte";
  import ReviewList from "$lib/components/ReviewList.svelte";
  import CommandLog from "$lib/components/CommandLog.svelte";
  import NotificationLog from "$lib/components/NotificationLog.svelte";
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
  let treeCtx = $state<{
    x: number;
    y: number;
    path: string;
    isDir: boolean;
    targets: SyncTarget[]; // the tree selection the actions apply to
  } | null>(null);

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

  // Login prompt (user + password), promise-based like askConfirm. `error`
  // carries the previous attempt's failure into the re-prompt.
  type Cred = { user: string; password: string };
  let loginState = $state<{
    user: string;
    port: string;
    error: string;
    resolve: (v: Cred | null) => void;
  } | null>(null);
  function promptLogin(port: string, user: string, error = ""): Promise<Cred | null> {
    return new Promise((resolve) => (loginState = { user, port, error, resolve }));
  }
  function resolveLogin(v: Cred | null) {
    loginState?.resolve(v);
    loginState = null;
  }

  // Transient status helpers (auto-clear).
  function setNotice(m: string, ms = 4000) {
    notice = m;
    notifications.add("notice", m);
    window.setTimeout(() => (notice = ""), ms);
  }
  function setError(m: string, ms = 6000) {
    error = m;
    notifications.add("error", m);
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
  let centerTab = $state<"history" | "pending" | "reviews" | "streams" | "log" | "notes">("pending");

  const centerRows = $derived(centerTab === "pending" ? pending.rows : history.rows);

  // --- closable views (Depot pane + center tabs), persisted; re-shown via the
  //     View menu. Depot and Streams are hidden by default.
  let views = $state<Views>(loadViews());
  $effect(() => saveViews(views));
  const TABS: {
    key: "history" | "pending" | "reviews" | "streams" | "log" | "notes";
    label: string;
  }[] = [
    { key: "history", label: "History" },
    { key: "pending", label: "Pending" },
    { key: "reviews", label: "Reviews" },
    { key: "streams", label: "Streams" },
    { key: "log", label: "Commands" },
    { key: "notes", label: "Notifications" },
  ];
  // Show a center tab (and load its data). History uses the current selection.
  function showTab(key: (typeof TABS)[number]["key"]) {
    centerTab = key;
    if (key === "pending") pending.load();
    else if (key === "reviews") void reviews.load();
    else if (key === "streams") browse.loadStreams();
  }
  // Keep centerTab on a visible tab; if the active one was closed, pick another.
  $effect(() => {
    if (!views[centerTab]) {
      const next = TABS.find((t) => views[t.key]);
      if (next) centerTab = next.key;
    }
  });
  // Reviews come from the server, so the tab needs a load when it becomes the
  // active one (including restored at startup) and after a (re)connect — there is
  // no local cache to paint in the meantime. `rootPath` is a dependency too: it
  // scopes the list, so switching stream must re-ask rather than keep the other
  // stream's reviews on screen.
  $effect(() => {
    void browse.rootPath;
    if (centerTab === "reviews" && connection.connected) void reviews.load();
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
    type MenuItem = {
      label: string;
      action?: () => void;
      disabled?: boolean;
      submenu?: MenuItem[];
      sep?: boolean;
    };
    const items: MenuItem[] = [];
    /** Start a new group: only ever between two real entries. */
    const group = () => {
      if (items.length && !items[items.length - 1].sep) items.push({ label: "", sep: true });
    };
    if (own) {
      items.push({
        label: isDefault ? "Submit default changelist…" : `Submit @${cl.change}…`,
        action: () => pending.submit(cl.change),
      });
    }
    group();
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
    group();
    items.push({ label: "Generate patch…", action: () => generatePatch(cl.change, []) });
    if (items[items.length - 1]?.sep) items.pop();
    return items;
  }

  // --- reviews: context menu over the Swarm review list ----------------------
  let reviewCtx = $state<{ x: number; y: number; r: ReviewRow } | null>(null);

  function reviewMenuItems(r: ReviewRow) {
    type MenuItem = { label: string; action?: () => void; disabled?: boolean; sep?: boolean };
    const items: MenuItem[] = [];
    // Whether the review maps into this workspace is only knowable from its
    // files, so it isn't gated here: `review_patch` refuses with the depot name
    // when nothing maps. The content changelist is resolved first — a review that
    // was submitted without approval has no shelf, and its content is the
    // submitted change instead.
    items.push({
      label: "Apply to workspace…",
      disabled: !r.change,
      action: async () => {
        const c = await reviews.content(r);
        const from = c.files.length ? c.change : r.change;
        patches.previewReview(String(from), `Review #${r.id} (@${from})`);
      },
    });
    items.push({ label: "", sep: true });
    items.push({ label: "Open in Swarm", action: () => openReviewPage(r.id) });
    items.push({
      label: `Copy @${r.change}`,
      disabled: !r.change,
      action: () => void setClipboard(String(r.change)),
    });
    return items;
  }

  async function openReviewPage(id: number) {
    const url = await reviews.url(id);
    if (!url) {
      setError("No Swarm server is configured (P4.Swarm.URL is unset).");
      return;
    }
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(url).catch((e) => setError(String(e)));
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
    const sel = files.length ? files : [file.depotFile];
    // Grouped by what an entry does — look at it, copy it, produce something from
    // it, change the file, move it — so a destructive action never sits directly
    // under one that only reads.
    return [
      // A conflict blocks the submit, so when there is one it comes first — and
      // the entry is absent otherwise, where it would mean nothing. The set comes
      // from the same fstat that drives the "needs resolve" badge, so it is free.
      ...(pending.needsResolve(file.depotFile)
        ? [{ label: "Resolve…", action: () => merges.resolveFile(file.depotFile) }, { label: "", sep: true }]
        : []),
      { label: "View diff", action: () => pending.openLocalDiff(file.depotFile) },
      { label: openInLabel, action: () => openLocalInEditor(file.depotFile) },
      { label: "", sep: true },
      copyMenu(file.depotFile, file.clientFile),
      { label: "", sep: true },
      { label: patchLabel, action: () => generatePatch("", sel) },
      { label: "", sep: true },
      {
        label: sel.length > 1 ? `Revert (${sel.length} files)…` : "Revert file…",
        action: () => pending.revertMixed(sel),
      },
      {
        // Named for the outcome: it becomes an entry in the Offline section.
        label: "Make offline (keep local edits)…",
        action: () => pending.revertKeep(file.depotFile),
      },
      { label: "", sep: true },
      { label: "Move to changelist", submenu: targets },
    ];
  }

  // Switch to the Pending tab and (re)load it.
  function openPending() {
    centerTab = "pending";
    pending.load();
  }

  // --- "Open in <editor>" ------------------------------------------------------
  // The menu label for the preferred editor ("Open in Notepad++"). Editors are
  // detected in the background; Notepad always exists, so this is never empty
  // once init resolves.
  const openInLabel = $derived(`Open in ${editor.current?.name ?? "editor"}`);
  function editorOpen(fn: () => Promise<void>) {
    fn().catch((e) => setError(String(e)));
  }
  /** A workspace-local file by depot path (opened/pending/tree-Local files). */
  function openLocalInEditor(depotFile: string) {
    editorOpen(() => editor.openDepotLocal(conn, browse.clientRoot, browse.rootPath, depotFile));
  }
  /** A server revision (depot/workspace head, history #rev, shelved @=change). */
  function openSpecInEditor(spec: string) {
    editorOpen(() => editor.openSpec(conn, spec));
  }

  // Context state for the two file lists that had no menu before.
  let shelvedCtx = $state<{ x: number; y: number; file: P4Record; change: string } | null>(null);
  let offlineCtx = $state<{ x: number; y: number; file: P4Record; files: string[] } | null>(null);
  let detailsCtx = $state<{ x: number; y: number; file: P4Record } | null>(null);

  // --- Copy name / depot path / workspace path -------------------------------
  function copied(text: string, label: string) {
    setClipboard(text)
      .then(() => setNotice(`Copied ${label}.`, 2500))
      .catch((e) => setError(String(e)));
  }
  /** The local (workspace) path of a depot path: stream mapping first, `p4 fstat`
   *  for files outside it. */
  async function copyWorkspacePath(depotFile: string, known?: string) {
    if (known) return copied(known, "workspace path");
    const mapped = localPathFor(browse.clientRoot, browse.rootPath, depotFile);
    if (mapped) return copied(mapped, "workspace path");
    const recs = await p4.fstat(conn, depotFile).catch(() => [] as P4Record[]);
    const local = recs[0]?.clientFile;
    if (local) copied(local, "workspace path");
    else setError("This file has no workspace path here.");
  }
  /** The shared "Copy" submenu for any file/folder path shown in the app.
   *  `clientFile` short-circuits the workspace-path lookup when already known. */
  function copyMenu(depotPath: string, clientFile?: string) {
    const name = depotPath.replace(/\/+$/, "").split("/").pop() || depotPath;
    return {
      label: "Copy",
      submenu: [
        { label: "File name", action: () => copied(name, "file name") },
        { label: "Depot path", action: () => copied(depotPath, "depot path") },
        { label: "Workspace path", action: () => void copyWorkspacePath(depotPath, clientFile) },
      ],
    };
  }

  // --- history row context menu: "update to this changelist" ----------------
  function openHistContext(change: string, e: MouseEvent) {
    if (!change || centerTab !== "history") return;
    history.selectChange(change); // highlight the right-clicked row
    ctxMenu = { x: e.clientX, y: e.clientY, change };
  }

  // --- depot tree: right-click to sync / reconcile just this path ------------
  function onTreeContext(path: string, isDir: boolean, e: MouseEvent, targets: SyncTarget[]) {
    if (!connection.connected || !conn.client) return;
    treeCtx = { x: e.clientX, y: e.clientY, path, isDir, targets };
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
    editor.init(); // detect editors + resolve the preferred one (background)
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
      setConnError: (m) => {
        error = m;
        notifications.add("error", m);
      },
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
    merges.init({
      conn: () => conn,
      connected: () => connection.connected,
      setNotice,
      setError,
      refresh: () => browse.refresh(),
      loadPending: () => pending.load(),
    });
    // A resolve window is its own webview; this is how the main one finds out.
    void import("@tauri-apps/api/event").then(({ listen }) =>
      listen("merge-done", () => {
        setNotice("Merge saved.", 4000);
        void afterMerge();
      }),
    );
    reviews.init({
      conn: () => conn,
      connected: () => connection.connected,
      setError,
      rootPath: () => browse.rootPath,
    });
    patches.init({
      conn: () => conn,
      connected: () => connection.connected,
      setNotice,
      setError,
      refresh: () => browse.refresh(),
      loadPending: () => pending.load(),
      scanOffline: () => pending.scanOffline(),
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
      loadPending: () => {
        pending.load();
        void pending.scanOffline(); // a sync/retry changes offline state — refresh the list
      },
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
        if (v) {
          updates.check(true); // silent check only on release builds
          updates.startAutoCheck();
        }
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
    onApplyPatch={() => patches.pickAndPreview()}
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

  <!-- Floating toasts: overlaid, so appearing/disappearing never reflows the app. -->
  {#if error || notice}
    <div class="toasts">
      {#if error}
        <div class="error mono">{error}</div>
      {/if}
      {#if notice}
        <div class="notice">{notice}</div>
      {/if}
    </div>
  {/if}

  <div class="cols">
    {#if views.files}
      <section class="col left" style="width:{leftW}px">
        <div class="panehdr">
          <span>Files</span>
          <!-- Only the server-backed sources can contain deleted-at-head files
               (Local lists what's on disk), so the option only appears there. -->
          {#if browse.source !== "local"}
            <label class="hdrchk" title="List files deleted at head — struck through; they can't be synced, but their history is browsable">
              <input
                type="checkbox"
                checked={browse.showDeleted}
                onchange={(e) => browse.setShowDeleted(e.currentTarget.checked)}
              />
              show deleted
            </label>
          {/if}
          <button class="paneclose" title="Close view" onclick={() => (views.files = false)}>✕</button>
        </div>
        <div class="srcsel">
          {#each [{ k: "local", l: "Local" }, { k: "workspace", l: "Workspace" }, { k: "depot", l: "Depot" }] as s (s.k)}
            <button
              class="srcbtn"
              class:active={browse.source === s.k}
              title={s.k === "local"
                ? "Files on disk in this workspace"
                : s.k === "workspace"
                  ? "This workspace's stream, from the server"
                  : "The whole depot (all depots), from the server"}
              onclick={() => browse.setSource(s.k as "local" | "workspace" | "depot")}
            >
              {s.l}
            </button>
          {/each}
        </div>
        <DepotTree
          root={browse.tree}
          selectedPath={browse.selectedTreePath}
          indexing={browse.indexing}
          onSelect={(n) => browse.selectNode(n)}
          onExpand={(n) => browse.expandNode(n)}
          onSearch={(t) => browse.searchDepot(t)}
          onOpenResult={(f) => browse.openResult(f)}
          onContext={(n, e, sel) =>
            onTreeContext(
              n.path,
              n.isDir,
              e,
              sel.map((x) => ({ path: x.path, isDir: x.isDir })),
            )}
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
      <!-- Flex wrapper so every tab panel fills the space left by .tabs; without it
           a panel's own height:100% ignores the tab bar and overflows under the
           status bar (as the tree did). -->
      <div class="tabbody">
      {#if !TABS.some((t) => views[t.key])}
        <div class="msg dim">All views are closed — reopen one from the View menu.</div>
      {:else if centerTab === "streams"}
        <StreamsBrowser
          rows={browse.streamRows}
          loading={browse.streamsLoading}
          currentStream={browse.rootPath}
          onContext={onStreamContext}
        />
      {:else if centerTab === "log"}
        <CommandLog entries={cmdlog.entries} onClear={() => cmdlog.clear()} />
      {:else if centerTab === "notes"}
        <NotificationLog entries={notifications.entries} onClear={() => notifications.clear()} />
      {:else if centerTab === "reviews"}
        <ReviewList
          rows={reviews.rows}
          loading={reviews.loading}
          paging={reviews.paging}
          more={reviews.more}
          error={reviews.error}
          status={reviews.status}
          user={reviews.user}
          role={reviews.role}
          search={reviews.search}
          streamOnly={reviews.streamOnly}
          hideSubmitted={reviews.hideSubmitted}
          streamPath={reviews.streamPath}
          me={conn.user}
          refreshKey={reviews.version}
          contextReview={reviewCtx?.r.id ?? 0}
          onStatus={(s: StatusFilter) => reviews.setStatus(s)}
          onUser={(u: string) => reviews.setUser(u)}
          onRole={(r: Role) => reviews.setRole(r)}
          onSearch={(q: string) => reviews.setSearch(q)}
          onStreamOnly={(v: boolean) => reviews.setStreamOnly(v)}
          onHideSubmitted={(v: boolean) => reviews.setHideSubmitted(v)}
          onLoadMore={() => void reviews.loadMore()}
          onContent={(r) => reviews.content(r)}
          onDiff={(f, rev, change, submitted) => reviews.diff(f, rev, change, submitted)}
          onOpenDiff={(f, rev, change, submitted) => reviews.openDiff(f, rev, change, submitted)}
          onContext={(r, e) => {
            e.preventDefault();
            reviewCtx = { x: e.clientX, y: e.clientY, r };
          }}
          onFileContext={(f, r, e) => (shelvedCtx = { x: e.clientX, y: e.clientY, file: f, change: String(r.change) })}
        />
      {:else if centerTab === "pending"}
        <PendingList
          bind:this={pendingList}
          rows={pending.rows}
          loading={pending.loading}
          client={conn.client}
          refreshKey={pending.version}
          reviews={pending.reviews}
          offline={pending.offline}
          offlineScanning={pending.offlineScanning}
          offlineScannedAt={pending.offlineScannedAt}
          offlineCached={pending.offlineCached}
          onOfflineDiff={pending.offlineDiff}
          onOpenOfflineDiff={pending.openLocalDiff}
          needsResolve={pending.needsResolve}
          contextChange={pendingCtx?.cl.change ?? ""}
          onLocalFiles={pending.localFiles}
          onLocalFilesCached={pending.localFilesCached}
          onShelvedFiles={pending.shelvedFiles}
          onShelvedFilesCached={pending.shelvedFilesCached}
          onLocalDiff={pending.localDiff}
          onShelvedDiff={pending.shelvedDiff}
          onOpenLocalDiff={pending.openLocalDiff}
          onOpenShelvedDiff={pending.openShelvedDiff}
          onContext={onPendingContext}
          onFileContext={onPendingFileContext}
          onShelvedContext={(f, change, e) =>
            (shelvedCtx = { x: e.clientX, y: e.clientY, file: f, change })}
          onOfflineContext={(f, e, files) =>
            (offlineCtx = { x: e.clientX, y: e.clientY, file: f, files })}
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
              onDeepen={() => history.deepen()}
              deepening={history.deepening}
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
              onFileContext={(f, e) => (detailsCtx = { x: e.clientX, y: e.clientY, file: f })}
            />
          </div>
        </div>
      {/if}
      </div>
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
    error={loginState.error}
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
    onResolveFile={(f) => merges.resolveFile(f)}
    onIgnoreFile={(f) => sync.ignoreFile(f)}
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
  <!-- Sync/unsync/reconcile act on the whole tree selection (Ctrl/Shift click);
       a single right-clicked node is just a selection of one. -->
  {@const tgts = treeCtx.targets}
  {@const what = tgts.length > 1 ? `${tgts.length} items` : `${kind} “${name}”`}
  <ContextMenu
    x={treeCtx.x}
    y={treeCtx.y}
    items={[
      // Files: Local source opens the on-disk file; Workspace/Depot download the
      // head revision from the server (p4 print to temp) and open that.
      ...(dir
        ? []
        : [
            {
              label: browse.source === "local" ? openInLabel : `${openInLabel} (from server)`,
              action: () =>
                browse.source === "local"
                  ? openLocalInEditor(p)
                  : openSpecInEditor(browse.source === "depot" ? p : browse.toQuery(p)),
            },
            { label: "", sep: true },
          ]),
      copyMenu(p),
      { label: "", sep: true },
      { label: `Sync ${what}`, action: () => sync.syncPath(tgts) },
      // The inverse of sync: drop the local copy, keep the depot untouched.
      { label: `Unsync ${what}…`, action: () => sync.unsyncPath(tgts) },
      { label: `Reconcile ${what}`, action: () => sync.reconcilePath(tgts) },
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

{#if shelvedCtx}
  {@const f = shelvedCtx.file}
  {@const ch = shelvedCtx.change}
  <ContextMenu
    x={shelvedCtx.x}
    y={shelvedCtx.y}
    items={[
      // The shelved content lives on the server — download @=change and open.
      { label: `${openInLabel} (shelved)`, action: () => openSpecInEditor(`${f.depotFile}@=${ch}`) },
      copyMenu(f.depotFile),
    ]}
    onClose={() => (shelvedCtx = null)}
  />
{/if}

{#if offlineCtx}
  {@const f = offlineCtx.file}
  <!-- Patch/revert act on the WHOLE selection (opened + offline mix); only
       "Check out" is inherently offline-only, so it uses the offline subset. -->
  {@const sel = offlineCtx.files.length ? offlineCtx.files : f.depotFile ? [f.depotFile] : []}
  {@const offSel = sel.filter((d) => pending.offline.some((o) => o.depotFile === d))}
  {@const co = offSel.length > 1 ? ` (${offSel.length} files)` : ""}
  {@const many = sel.length > 1 ? ` (${sel.length} files)` : ""}
  <ContextMenu
    x={offlineCtx.x}
    y={offlineCtx.y}
    items={[
      // Desync entries aren't local edits — offer the record repair (p4 flush).
      ...(f.desync
        ? [
            {
              label: "Repair sync record (file untouched)",
              action: () => pending.repairDesync(f.depotFile),
            },
          ]
        : []),
      { label: `Check out${co}`, action: () => pending.checkoutOffline(offSel) },
      { label: `Generate patch${many}…`, action: () => generatePatch("", sel) },
      { label: `Revert${many}…`, action: () => pending.revertMixed(sel) },
      {
        label: openInLabel,
        action: () => {
          const local = f.clientFile;
          if (local) editorOpen(() => editor.openLocal(local));
          else if (f.depotFile) openLocalInEditor(f.depotFile);
        },
      },
      copyMenu(f.depotFile ?? f.clientFile ?? "", f.clientFile),
    ]}
    onClose={() => (offlineCtx = null)}
  />
{/if}

{#if detailsCtx}
  {@const f = detailsCtx.file}
  <ContextMenu
    x={detailsCtx.x}
    y={detailsCtx.y}
    items={[
      // The submitted revision from the server (not the local file, which may differ).
      {
        label: `${openInLabel} (revision #${f.rev})`,
        action: () => openSpecInEditor(`${f.depotFile}#${f.rev}`),
      },
      copyMenu(f.depotFile),
    ]}
    onClose={() => (detailsCtx = null)}
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

{#if reviewCtx}
  <ContextMenu
    x={reviewCtx.x}
    y={reviewCtx.y}
    items={reviewMenuItems(reviewCtx.r)}
    onClose={() => (reviewCtx = null)}
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
    initialName={connection.suggestWorkspaceName()}
    initialStream={browse.rootPath}
    pickFolder={(s) => connection.pickFolder(s)}
    loadStreams={() => connection.loadStreams()}
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

{#if patches.open}
  <ApplyPatchDialog
    path={patches.path}
    phase={patches.phase}
    files={patches.files}
    busy={patches.busy}
    subject={patches.subject}
    skipped={patches.skipped}
    onApply={(mode, partial) => patches.apply(mode, partial)}
    onResolveHunk={(depot, hunk) => merges.resolvePatchHunk(patches.path, depot, hunk)}
    onClose={() => patches.close()}
  />
{/if}

<style>
  .app {
    display: flex;
    flex-direction: column;
    /* 100% (of html/body, which fill the webview) not 100vh: in the Tauri WebView2
       the viewport unit overshoots the real client area, pushing the status bar
       partly off-screen. overflow:hidden guards any residual sub-pixel overflow. */
    height: 100%;
    overflow: hidden;
  }
  /* Overlay container just below the toolbar; the toasts inside never affect
     the app layout. */
  .toasts {
    position: fixed;
    /* At the Refresh/Sync button-row level: that band is mostly empty, so the
       toast masks chrome rather than content below. */
    top: 64px;
    left: 50%;
    transform: translateX(-50%);
    z-index: 45;
    display: flex;
    flex-direction: column;
    gap: 6px;
    max-width: min(720px, 90vw);
    pointer-events: none; /* don't block clicks on the UI beneath */
  }
  .error,
  .notice {
    pointer-events: auto; /* but allow selecting/copying the message text */
    border-radius: 6px;
    box-shadow: 0 4px 18px rgba(0, 0, 0, 0.35);
    padding: 6px 14px;
    font-size: 12px;
  }
  .error {
    background: var(--warn);
    color: white;
    white-space: pre-wrap;
  }
  .notice {
    background: var(--have-bg);
    color: var(--have);
    border: 1px solid var(--have);
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
  /* Holds the active tab's panel; fills the space under .tabs so a panel's own
     height:100% resolves against this (not the whole column). */
  .tabbody {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
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
  /* A small labelled checkbox option in a pane header, pushed to the right next
     to the close button. A real checkbox so both states read unambiguously. */
  .hdrchk {
    margin-left: auto;
    margin-right: 6px;
    display: flex;
    align-items: center;
    gap: 4px;
    font-size: 10px;
    font-weight: 400;
    text-transform: none;
    letter-spacing: 0;
    color: var(--text-dim);
    cursor: pointer;
  }
  .hdrchk input {
    width: 11px;
    height: 11px;
    margin: 0;
    cursor: pointer;
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
  .srcsel {
    display: flex;
    gap: 2px;
    padding: 4px 6px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }
  .srcbtn {
    flex: 1;
    border: 1px solid var(--border);
    background: var(--bg-alt);
    border-radius: 4px;
    padding: 3px 6px;
    font-size: 11px;
    color: var(--text-dim);
    cursor: pointer;
  }
  .srcbtn:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .srcbtn.active {
    color: var(--accent);
    border-color: var(--accent);
    background: var(--bg-sel);
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
