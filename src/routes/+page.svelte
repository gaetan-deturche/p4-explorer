<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import { getVersion } from "@tauri-apps/api/app";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import {
    p4,
    idx,
    isReleaseBuild,
    emptyConn,
    firstLine,
    type P4Conn,
    type P4Record,
  } from "$lib/p4";
  import { makeNode, type TreeNode } from "$lib/tree";
  import { loadServers, saveServers, withServer, withoutServer } from "$lib/servers";
  import {
    loadFolder,
    saveFolder,
    loadHist,
    saveHist,
    clearClientCache,
    buildChildren,
    localChildren,
    type FolderContents,
    type HistEntry,
  } from "$lib/cache";
  import { updates } from "$lib/updates.svelte";
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
  let refreshing = $state(false);
  let syncing = $state(false);
  let reconciling = $state(false);
  let optionsOpen = $state(false);
  let ctxMenu = $state<{ x: number; y: number; change: string } | null>(null);
  let streamCtx = $state<{ x: number; y: number; stream: string } | null>(null);
  let pendingCtx = $state<{ x: number; y: number; cl: P4Record } | null>(null);
  let swarmBase = ""; // cached Swarm URL

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

  // Live sync progress dialog.
  let syncCancelled = false;
  let syncProgress = $state<{
    title: string;
    count: number;
    current: string;
    issues: number;
    issueLine: string;
    phase: "running" | "error";
    message: string;
  } | null>(null);
  // Full per-file error report shown after a sync that had issues.
  let syncErrorItems: { line: string; file: string | null }[] = [];
  let syncErrors = $state<{
    title: string;
    items: { line: string; file: string | null }[];
    path: string | undefined;
  } | null>(null);
  let busyFile = $state<string | null>(null); // file being fixed ("*" = retry-all)

  // Run a streaming sync with a live progress dialog (live file count).
  // Auto-closes on success; keeps the dialog open on error; cancellable.
  async function runSyncWithProgress(title: string, path: string | undefined): Promise<number | null> {
    syncCancelled = false;
    syncErrorItems = [];
    syncProgress = { title, count: 0, current: "", issues: 0, issueLine: "", phase: "running", message: "" };
    const un1 = await listen<{ count: number; line: string }>("sync-progress", (e) => {
      if (syncProgress) {
        syncProgress.count = e.payload.count;
        syncProgress.current = e.payload.line;
      }
    });
    const un2 = await listen<{ count: number; line: string; file: string | null }>("sync-issue", (e) => {
      syncErrorItems.push({ line: e.payload.line, file: e.payload.file });
      if (syncProgress) {
        syncProgress.issues = e.payload.count;
        syncProgress.issueLine = e.payload.line;
      }
    });
    try {
      const n = await p4.syncStream(conn, path);
      syncProgress = null; // close the progress dialog
      if (syncErrorItems.length > 0) {
        // Completed, but some files errored — show the full report + Fix.
        syncErrors = { title, items: [...syncErrorItems], path };
      } else {
        notice = n > 0 ? `Synced ${n} file${n === 1 ? "" : "s"}.` : "Already up to date.";
        window.setTimeout(() => (notice = ""), 4000);
      }
      return n;
    } catch (e) {
      if (syncCancelled) {
        syncProgress = null;
        notice = "Sync cancelled.";
        window.setTimeout(() => (notice = ""), 4000);
        return null;
      }
      if (syncProgress) {
        syncProgress.phase = "error";
        syncProgress.message = String(e);
      }
      return null;
    } finally {
      un1();
      un2();
    }
  }
  function cancelSync() {
    syncCancelled = true;
    p4.syncCancel().catch(() => {});
  }

  function syncErrorTargets(): string[] {
    if (!syncErrors) return [];
    const files = Array.from(
      new Set(syncErrors.items.map((i) => i.file).filter((f): f is string => !!f)),
    );
    return files.length ? files : [syncErrors.path ?? (rootPath ? `${rootPath}/...` : "...")];
  }
  // Fix a single errored file: plain re-sync, or force (confirmed) to overwrite.
  async function resyncFile(file: string, force: boolean) {
    if (!syncErrors || busyFile) return;
    if (
      force &&
      !(await askConfirm(
        `${file}\n\nForce-overwrite with the depot version? Local changes will be DISCARDED.`,
        "Force overwrite",
        "Overwrite",
      ))
    ) {
      return;
    }
    busyFile = file;
    error = "";
    try {
      await p4.resync(conn, [file], force);
      const rest = syncErrors.items.filter((i) => i.file !== file);
      syncErrors = rest.length ? { ...syncErrors, items: rest } : null;
      await refresh();
      loadPending();
    } catch (e) {
      error = String(e);
    } finally {
      busyFile = null;
    }
  }
  // Retry (force optional) all affected files at once.
  async function resyncAllErrors(force: boolean) {
    if (!syncErrors || busyFile) return;
    if (
      force &&
      !(await askConfirm(
        "Force-overwrite ALL affected files with the depot version?\nLocal changes will be DISCARDED. Conflicts must be resolved separately (p4 resolve / P4V).",
        "Force overwrite all",
        "Overwrite all",
      ))
    ) {
      return;
    }
    const targets = syncErrorTargets();
    busyFile = "*";
    error = "";
    try {
      await p4.resync(conn, targets, force);
      syncErrors = null;
      notice = force ? "Force re-synced the affected files." : "Re-synced the affected files.";
      window.setTimeout(() => (notice = ""), 4000);
      await refresh();
      loadPending();
    } catch (e) {
      error = String(e);
    } finally {
      busyFile = null;
    }
  }
  let indexing = $state(false);
  let indexCount = $state(0);

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
    resetBrowse();
    await connect();
  }
  function submitAddServer(port: string) {
    addServerOpen = false;
    const v = port.trim();
    if (!v) return;
    rememberServer(v);
    switchServerTo(v);
  }

  // Depot tree
  let rootPath = $state(""); // stream root, e.g. //Curiosity/main
  let clientRoot = $state(""); // local workspace root, e.g. H:\Dev\...\Curiosity
  let tree = $state<TreeNode | null>(null);
  let selectedTreePath = $state("");

  // Center: history / pending
  let centerTab = $state<"history" | "pending" | "streams" | "repo">("pending");
  let histMode = $state<"folder" | "file">("folder");
  let histSubject = $state("");
  let histRows = $state<P4Record[]>([]);
  let histLoading = $state(false);
  let haveChange = $state("");
  let haveRev = $state("");
  let pendingRows = $state<P4Record[]>([]);
  let pendingLoading = $state(false);
  // Streams / Repo browser tabs
  let streamRows = $state<P4Record[]>([]);
  let streamsLoading = $state(false);
  let repoTree = $state<TreeNode | null>(null);
  let repoSelected = $state("");
  let histMore = $state(false); // background paging of older changelists in flight

  // Right: changelist details
  let selectedChange = $state("");
  let descRows = $state<P4Record[]>([]);
  let descLoading = $state(false);

  // Monotonic token so stale center-pane loads are dropped when selection changes.
  let loadSeq = 0;

  // Delayed loading indicator: keep the previous history visible and only show
  // "Loading…" if fresh data hasn't arrived within 2s (nothing to keep → now).
  let histLoadTimer: number | null = null;
  function beginHistLoad(seq: number) {
    if (histLoadTimer !== null) clearTimeout(histLoadTimer);
    if (histRows.length === 0) {
      histLoading = true;
      return;
    }
    histLoadTimer = window.setTimeout(() => {
      if (seq === loadSeq) histLoading = true;
    }, 2000);
  }
  function endHistLoad() {
    if (histLoadTimer !== null) {
      clearTimeout(histLoadTimer);
      histLoadTimer = null;
    }
    histLoading = false;
  }

  const centerRows = $derived(centerTab === "pending" ? pendingRows : histRows);

  async function safe<T>(fn: () => Promise<T[]>): Promise<T[]> {
    try {
      return await fn();
    } catch {
      return [];
    }
  }

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
        if (rootPath) p4.dirs(conn, rootPath).catch(() => {}); // keep cache warm
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

  // --- folder-contents cache (in-memory; persistence + helpers in $lib/cache) -
  const browCache = new Map<string, FolderContents>();

  // Populate a directory node's children: instant from cache/local disk, then
  // the authoritative p4 listing replaces it (stale-while-revalidate).
  async function loadNode(node: TreeNode) {
    if (node.loading) return;
    node.loading = true;
    const path = node.path;

    const mem = browCache.get(path);
    const cached = mem ?? loadFolder(conn.client, path);
    if (cached) {
      node.children = buildChildren(cached);
    } else {
      const local = await localChildren(clientRoot, rootPath, path);
      if (local && node.children.length === 0) node.children = buildChildren(local);
    }

    // Refresh unless we already have it fresh in memory this session.
    if (!mem) {
      const [d, f] = await Promise.all([
        safe(() => p4.dirs(conn, path)),
        safe(() => p4.files(conn, path)),
      ]);
      const c = { dirs: d, files: f };
      node.children = buildChildren(c);
      browCache.set(path, c);
      saveFolder(conn.client, path, c);
    }
    node.loaded = true;
    node.loading = false;
  }

  // Single click: select (dir → history, file → details) — does NOT fold.
  function selectNode(node: TreeNode) {
    selectedTreePath = node.path;
    if (node.isDir) loadFolderHistory(node.path);
    else selectFile(node.path);
  }

  // Triangle / double click: toggle fold state, loading children on first open.
  function expandNode(node: TreeNode) {
    node.expanded = !node.expanded;
    if (node.expanded && !node.loaded) loadNode(node);
  }

  // Build (or rebuild) the local fuzzy-search index for the current workspace.
  async function buildIndex() {
    if (!connected || !conn.client || !rootPath || indexing) return;
    indexing = true;
    try {
      indexCount = await idx.build(conn, conn.client, rootPath);
    } catch {
      /* leave count as-is */
    } finally {
      indexing = false;
    }
  }

  // Ensure an index exists (build in the background if this workspace is new).
  async function ensureIndex() {
    if (!connected || !conn.client) return;
    try {
      indexCount = await idx.status(conn.client);
    } catch {
      indexCount = 0;
    }
    if (indexCount === 0) buildIndex();
  }

  // Per-keystroke fuzzy search over the local index (case-insensitive, no p4).
  async function searchDepot(term: string): Promise<P4Record[]> {
    if (!term.trim() || !conn.client) return [];
    const paths = await idx.search(conn.client, term.trim(), 200);
    return paths.map((p) => ({ depotFile: p }) as P4Record);
  }

  function openResult(depotFile: string) {
    selectedTreePath = depotFile;
    selectFile(depotFile);
  }

  // Changelist file diffs.
  function fileDiff(depotFile: string, rev: number): Promise<string> {
    return p4.diff2(conn, depotFile, rev);
  }
  async function openFileDiff(depotFile: string, rev: number) {
    try {
      await p4.openDiff(conn, depotFile, rev);
    } catch (e) {
      notice = String(e);
      window.setTimeout(() => (notice = ""), 5000);
    }
  }

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
    resetBrowse();
    const rec = clients.find((c) => c.client === conn.client);
    if (!rec) return;
    clientRoot = rec.Root ?? "";
    if (rec.Stream) {
      rootPath = rec.Stream;
    } else {
      error = "This workspace has no stream. Depot browsing currently requires a stream client.";
      return;
    }
    tree = makeNode(rootPath, true);
    tree.expanded = true;
    selectedTreePath = rootPath;
    // Load each tab's data (the loaders each flip centerTab); restore it after.
    loadFolderHistory(rootPath);
    loadPending();
    if (tab === "streams") loadStreams();
    else if (tab === "repo") openRepo();
    ensureIndex(); // background: build the fuzzy-search index if this ws is new
    centerTab = tab; // restore the tab the user was on
    await loadNode(tree); // `tree` is the reactive proxy — mutate through it
  }

  function resetBrowse() {
    browCache.clear();
    histMemCache.clear();
    rootPath = "";
    clientRoot = "";
    tree = null;
    selectedTreePath = "";
    histRows = [];
    histSubject = "";
    haveChange = "";
    haveRev = "";
    selectedChange = "";
    descRows = [];
    pendingRows = [];
    streamRows = [];
    repoTree = null;
    repoSelected = "";
  }

  // --- center pane: history / pending / details ------------------------------
  const CHUNK = 50;
  const CAP = 400;

  // History cache: in-memory (session) + persistent (via $lib/cache). Switching
  // to a previously-viewed file/folder shows its history instantly, then refreshes.
  const histMemCache = new Map<string, HistEntry>();

  // Persist a history entry to both the session map and localStorage.
  function cacheHist(id: string, e: HistEntry) {
    histMemCache.set(id, e);
    saveHist(conn.client, id, e);
  }
  function applyHist(e: HistEntry) {
    histMode = e.mode;
    histSubject = e.subject;
    if (e.mode === "folder") {
      haveChange = e.have;
      haveRev = "";
    } else {
      haveRev = e.have;
      haveChange = "";
    }
    histRows = e.rows;
    histMore = false;
  }

  async function loadFolderHistory(path: string, seq: number = ++loadSeq) {
    centerTab = "history";
    const id = "F:" + path;

    // Instant from cache.
    const cached = histMemCache.get(id) ?? loadHist(conn.client, id);
    if (cached) {
      endHistLoad();
      applyHist(cached);
      autoSelectHave();
    } else {
      beginHistLoad(seq);
    }

    // Fetch the first chunk AND the synced-CL together so the list appears with
    // its greying/bold already correct — no ungreyed-then-greyed flash.
    const [firstBatch, have] = await Promise.all([
      safe(() => p4.changes(conn, path, CHUNK)),
      safe(() => p4.haveChange(conn, path)),
    ]);
    if (seq !== loadSeq) return; // selection changed; keep whatever's shown
    endHistLoad();
    const haveCl = have[0]?.change ?? "";

    // If we showed cached data, refresh fully into an accumulator and swap once
    // (no shrink-then-grow flicker). Otherwise paint progressively.
    let rows = firstBatch;
    if (!cached) applyHist({ mode: "folder", subject: path, rows, have: haveCl });

    if (firstBatch.length === CHUNK) {
      let before = Math.min(...firstBatch.map((b) => Number(b.change))) - 1;
      while (rows.length < CAP && Number.isFinite(before) && before > 0) {
        if (!cached) histMore = true;
        const batch = await safe(() => p4.changes(conn, path, CHUNK, before));
        if (seq !== loadSeq) return;
        if (batch.length === 0) break;
        rows = [...rows, ...batch];
        if (!cached) histRows = rows; // progressive when nothing was shown
        const min = Math.min(...batch.map((b) => Number(b.change)));
        if (batch.length < CHUNK || !Number.isFinite(min) || min <= 1) break;
        before = min - 1;
      }
    }
    histMore = false;
    const fresh: HistEntry = { mode: "folder", subject: path, rows, have: haveCl };
    applyHist(fresh); // single atomic swap (covers the cached-refresh case)
    autoSelectHave();
    cacheHist(id, fresh);
  }

  async function selectFile(depotFile: string) {
    const seq = ++loadSeq;
    centerTab = "history";
    const id = "R:" + depotFile;

    const cached = histMemCache.get(id) ?? loadHist(conn.client, id);
    if (cached) {
      endHistLoad();
      applyHist(cached);
      autoSelectHave();
    } else {
      beginHistLoad(seq);
    }

    const [rows, fs] = await Promise.all([
      safe(() => p4.filelog(conn, depotFile, 200)),
      safe(() => p4.fstat(conn, depotFile)),
    ]);
    if (seq !== loadSeq) return;
    endHistLoad();
    const fresh: HistEntry = {
      mode: "file",
      subject: depotFile,
      rows,
      have: fs[0]?.haveRev ?? "",
    };
    applyHist(fresh);
    autoSelectHave();
    cacheHist(id, fresh);
  }

  async function selectChange(change: string) {
    if (!change || change === selectedChange) return;
    selectedChange = change;
    descLoading = true;
    descRows = await safe(() => p4.describe(conn, change));
    descLoading = false;
  }

  // Auto-select the changelist the workspace is currently synced to.
  function autoSelectHave() {
    if (histMode === "folder") {
      if (haveChange) selectChange(haveChange);
      else {
        selectedChange = "";
        descRows = [];
      }
    } else {
      const row = histRows.find((r) => r.rev === haveRev);
      if (row?.change) selectChange(row.change);
      else {
        selectedChange = "";
        descRows = [];
      }
    }
  }

  function pendingLocalFiles(change: string): Promise<P4Record[]> {
    return safe(() => p4.opened(conn, change));
  }
  function pendingShelvedFiles(change: string): Promise<P4Record[]> {
    if (change === "default") return Promise.resolve([]); // default CL can't be shelved
    return safe(() => p4.describeShelved(conn, change));
  }
  function localDiff(depotFile: string): Promise<string> {
    return p4.diffLocal(conn, depotFile);
  }
  function shelvedDiff(depotFile: string, rev: number, change: string): Promise<string> {
    return p4.diffShelved(conn, depotFile, rev, change);
  }
  async function openLocalDiff(depotFile: string) {
    try {
      await p4.openDiffLocal(conn, depotFile);
    } catch (e) {
      notice = String(e);
      window.setTimeout(() => (notice = ""), 5000);
    }
  }
  async function openShelvedDiff(depotFile: string, rev: number, change: string) {
    try {
      await p4.openDiffShelved(conn, depotFile, rev, change);
    } catch (e) {
      notice = String(e);
      window.setTimeout(() => (notice = ""), 5000);
    }
  }

  // Pending has its own loading flag (independent of the history seq/cancel), so
  // loading it alongside the History tab doesn't cancel the history load.
  // --- pending changelist context actions ------------------------------------
  function onPendingContext(cl: P4Record, e: MouseEvent) {
    pendingCtx = { x: e.clientX, y: e.clientY, cl };
  }

  // Run a workspace-mutating pending action, then refresh + reload the list.
  async function pendingMutate(run: () => Promise<unknown>, okNotice: string) {
    if (!connected || syncing) return;
    syncing = true;
    error = "";
    notice = "";
    try {
      await run();
      notice = okNotice;
      window.setTimeout(() => (notice = ""), 4000);
      await refresh();
      loadPending();
    } catch (e) {
      error = String(e);
    } finally {
      syncing = false;
    }
  }

  // Wrap a workspace-mutating pending action with a confirm prompt.
  async function pendingAction(
    run: () => Promise<unknown>,
    confirmMsg: string,
    confirmTitle: string,
    okLabel: string,
    okNotice: string,
  ) {
    if (!connected || syncing) return;
    if (!(await askConfirm(confirmMsg, confirmTitle, okLabel))) return;
    await pendingMutate(run, okNotice);
  }
  function submitCL(change: string) {
    const what = change === "default" ? "the default changelist" : `changelist @${change}`;
    pendingAction(
      () => p4.submit(conn, change),
      `Submit ${what}?\nThis commits the files to the depot and cannot be undone.`,
      "Submit changelist",
      "Submit",
      "Changelist submitted.",
    );
  }
  function requestReviewCL(change: string) {
    pendingAction(
      () => p4.requestReview(conn, change),
      `Request a Swarm review for @${change}?\nThis adds #review to the description and shelves the files.`,
      "Request review",
      "Request",
      "Review requested.",
    );
  }
  function updateReviewCL(change: string) {
    pendingAction(
      () => p4.shelveUpdate(conn, change),
      `Update the review for @${change} by re-shelving its files?`,
      "Update review",
      "Update",
      "Review updated.",
    );
  }
  function deleteShelfCL(change: string) {
    pendingAction(
      () => p4.shelveDelete(conn, change),
      `Delete the shelved files of @${change}?`,
      "Delete shelf",
      "Delete",
      "Shelf deleted.",
    );
  }
  async function openReviewCL(change: string) {
    try {
      if (!swarmBase) swarmBase = await p4.swarmUrl(conn).catch(() => "");
      if (!swarmBase) {
        error = "Swarm URL is not configured on the server.";
        return;
      }
      await openUrl(`${swarmBase.replace(/\/$/, "")}/changes/${change}`);
    } catch (e) {
      error = String(e);
    }
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
        action: () => submitCL(cl.change),
      });
    }
    if (own && !isDefault) {
      items.push({
        label: "Rename…",
        action: () => (renameCl = { change: cl.change, desc: (cl.desc ?? "").trim() }),
      });
    }
    if (own && !isDefault) {
      if (hasReview) items.push({ label: "Update review", action: () => updateReviewCL(cl.change) });
      else items.push({ label: "Request review", action: () => requestReviewCL(cl.change) });
    }
    if (!isDefault) {
      items.push({ label: "Open review in browser", action: () => openReviewCL(cl.change) });
    }
    if (own && !isDefault) {
      items.push({ label: "Delete shelf", action: () => deleteShelfCL(cl.change) });
    }
    return items;
  }

  // --- pending FILE context actions (local/opened files) ---------------------
  let fileCtx = $state<{ x: number; y: number; file: P4Record; change: string } | null>(null);
  let newClFile = $state<string | null>(null); // a file awaiting a new-changelist name
  let renameCl = $state<{ change: string; desc: string } | null>(null); // CL being renamed

  function onPendingFileContext(file: P4Record, change: string, e: MouseEvent) {
    fileCtx = { x: e.clientX, y: e.clientY, file, change };
  }
  function revertFile(depotFile: string) {
    pendingAction(
      () => p4.revert(conn, depotFile),
      `${depotFile}\n\nRevert this file? Your local changes will be discarded.`,
      "Revert file",
      "Revert",
      "File reverted.",
    );
  }
  function revertKeepFile(depotFile: string) {
    pendingAction(
      () => p4.revertKeep(conn, depotFile),
      `${depotFile}\n\nRemove from its changelist but keep your local edits on disk?`,
      "Remove from changelist",
      "Remove",
      "File removed from changelist (changes kept).",
    );
  }
  function reopenFile(depotFile: string, change: string) {
    const label = change === "default" ? "Default" : "@" + change;
    pendingMutate(() => p4.reopen(conn, depotFile, change), `Moved to ${label}.`);
  }
  function submitNewChangelist(desc: string) {
    const file = newClFile;
    newClFile = null;
    if (!file) return;
    pendingMutate(async () => {
      const ch = await p4.newChangelist(conn, desc);
      await p4.reopen(conn, file, ch);
    }, "Moved to a new changelist.");
  }
  function submitRename(desc: string) {
    const target = renameCl;
    renameCl = null;
    if (!target) return;
    pendingMutate(() => p4.setDescription(conn, target.change, desc), "Changelist renamed.");
  }

  // Right-click menu for a pending file: view/revert, un-open, or move to a CL.
  function fileMenuItems(file: P4Record, change: string) {
    const targets = pendingRows
      .filter((cl) => cl.change !== change)
      .map((cl) => {
        const desc = firstLine(cl.desc);
        const short = desc.length > 32 ? desc.slice(0, 31) + "…" : desc;
        const label =
          cl.change === "default" ? "Default" : short ? `@${cl.change}  ${short}` : "@" + cl.change;
        return { label, action: () => reopenFile(file.depotFile, cl.change) };
      });
    targets.push({ label: "New changelist…", action: () => (newClFile = file.depotFile) });
    return [
      { label: "View diff", action: () => openLocalDiff(file.depotFile) },
      { label: "Revert file…", action: () => revertFile(file.depotFile) },
      {
        label: "Remove from changelist (keep changes)…",
        action: () => revertKeepFile(file.depotFile),
      },
      { label: "Move to changelist", submenu: targets },
    ];
  }

  async function loadPending() {
    centerTab = "pending";
    // Workspace-scoped: no client selected → nothing to show (don't list the
    // user's pending CLs from other workspaces).
    if (!connected || !conn.client) {
      pendingRows = [];
      pendingLoading = false;
      return;
    }
    if (pendingRows.length === 0) pendingLoading = true; // keep previous list otherwise
    const rows = await safe(() => p4.pending(conn, 100));
    pendingLoading = false;
    // Prepend the client's Default changelist (opened files not in a numbered CL).
    const def = { change: "default", desc: "", user: conn.user, time: "" } as P4Record;
    pendingRows = [def, ...rows];
  }

  // --- refresh / global sync -------------------------------------------------
  // Re-fetch an expanded node's children, preserving which descendants were open.
  async function reloadNode(node: TreeNode) {
    if (!node.isDir || !node.expanded) return;
    const openPaths = new Set(node.children.filter((c) => c.isDir && c.expanded).map((c) => c.path));
    node.loaded = false;
    await loadNode(node);
    for (const child of node.children) {
      if (openPaths.has(child.path)) {
        child.expanded = true;
        await reloadNode(child);
      }
    }
  }

  async function refresh() {
    if (!connected || refreshing) return;
    refreshing = true;
    try {
      browCache.clear();
      histMemCache.clear();
      clearClientCache(conn.client);
      if (tree) {
        tree.expanded = true;
        await reloadNode(tree);
      }
      if (selectedTreePath && histMode === "file") await selectFile(selectedTreePath);
      else if (selectedTreePath) await loadFolderHistory(selectedTreePath);
      buildIndex(); // rebuild the fuzzy-search index in the background
    } finally {
      refreshing = false;
    }
  }

  async function globalSync() {
    if (!connected || syncing) return;
    if (
      !(await askConfirm(
        "Sync the entire workspace to the latest revision?\nThis may download a lot of files.",
        "Global sync",
        "Sync",
      ))
    ) {
      return;
    }
    syncing = true;
    error = "";
    notice = "";
    try {
      const n = await runSyncWithProgress("Global sync", undefined);
      if (n !== null) await refresh(); // have-revs changed → update markers
    } finally {
      syncing = false;
    }
  }

  // Reconcile offline work: open files changed outside Perforce into Default.
  async function reconcileWorkspace() {
    if (!connected || syncing || refreshing || reconciling || !rootPath) return;
    if (
      !(await askConfirm(
        `${rootPath}\n\nReconcile offline work? This opens files changed, added, or deleted outside Perforce into the default changelist.`,
        "Reconcile offline work",
        "Reconcile",
      ))
    ) {
      return;
    }
    reconciling = true;
    error = "";
    notice = "";
    try {
      const rows = await p4.reconcile(conn, rootPath);
      const n = rows.length;
      notice = n > 0 ? `Reconciled ${n} file${n === 1 ? "" : "s"} into the default changelist.` : "Nothing to reconcile.";
      window.setTimeout(() => (notice = ""), 5000);
      await refresh();
      loadPending();
    } catch (e) {
      error = String(e);
    } finally {
      reconciling = false;
    }
  }

  // --- history row context menu: "update to this changelist" ----------------
  function openHistContext(change: string, e: MouseEvent) {
    if (!change || centerTab !== "history") return;
    ctxMenu = { x: e.clientX, y: e.clientY, change };
  }

  // Sync the currently-viewed path (folder or file) to a changelist — forward
  // or backward — like TortoiseSVN's "update to revision".
  async function updateToChange(change: string) {
    if (!connected || syncing || !histSubject) return;
    const spec =
      histMode === "file" ? `${histSubject}@${change}` : `${histSubject}/...@${change}`;
    const label = histMode === "file" ? histSubject : `${histSubject}/...`;
    if (
      !(await askConfirm(
        `${label}\n\nFiles will be synced to their state at @${change} (this can move backward).`,
        `Update to changelist @${change}`,
        "Update",
      ))
    ) {
      return;
    }
    syncing = true;
    error = "";
    notice = "";
    try {
      const n = await runSyncWithProgress(`Update to @${change}`, spec);
      if (n !== null) await refresh();
    } finally {
      syncing = false;
    }
  }

  // --- Streams tab ------------------------------------------------------------
  async function loadStreams() {
    centerTab = "streams";
    if (!connected) return;
    if (streamRows.length === 0) streamsLoading = true;
    const rows = await safe(() => p4.streams(conn));
    streamsLoading = false;
    streamRows = rows;
  }

  function onStreamContext(stream: string, e: MouseEvent) {
    if (!connected || !conn.client) return; // switching reconfigures the current workspace
    if (!stream || stream === rootPath) return; // already on it
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

  // --- Repo (all-depots) browser tab -----------------------------------------
  function openRepo() {
    centerTab = "repo";
    if (!connected) return;
    if (!repoTree) {
      repoTree = {
        path: "//",
        name: "Depots",
        isDir: true,
        expanded: true,
        loaded: false,
        loading: false,
        children: [],
      };
      loadRepoNode(repoTree);
    }
  }
  async function loadRepoNode(node: TreeNode) {
    if (node.loading) return;
    node.loading = true;
    if (node.path === "//") {
      const depots = await safe(() => p4.depots(conn));
      node.children = depots.filter((d) => d.name).map((d) => makeNode("//" + d.name, true));
    } else {
      const [d, f] = await Promise.all([
        safe(() => p4.dirs(conn, node.path)),
        safe(() => p4.files(conn, node.path)),
      ]);
      node.children = buildChildren({ dirs: d, files: f });
    }
    node.loaded = true;
    node.loading = false;
  }
  function repoExpand(node: TreeNode) {
    node.expanded = !node.expanded;
    if (node.expanded && !node.loaded) loadRepoNode(node);
  }
  function repoSelect(node: TreeNode) {
    repoSelected = node.path;
    if (node.isDir) repoExpand(node);
    else {
      selectFile(node.path);
      centerTab = "history"; // jump to History showing this file's revisions
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
    updates.init({
      isRelease: () => isRelease,
      appVersion: () => appVersion,
      notify: (m) => setNotice(m),
      warn: (m) => setError(m),
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
    {refreshing}
    {syncing}
    onOptions={() => (optionsOpen = true)}
    onReconnect={connect}
    onExit={exitApp}
    onRefresh={refresh}
    onSync={globalSync}
    onAbout={showAbout}
    onCheckUpdates={() => updates.check(false)}
  />
  <Toolbar
    bind:conn
    {clients}
    {servers}
    {connected}
    {refreshing}
    {syncing}
    {reconciling}
    onClientChange={selectClient}
    onServerChange={switchServerTo}
    onAddServer={() => (addServerOpen = true)}
    onServerContext={(e) => {
      if (conn.port) serverCtx = { x: e.clientX, y: e.clientY };
    }}
    onRefresh={refresh}
    onSync={globalSync}
    onReconcile={reconcileWorkspace}
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
        root={tree}
        selectedPath={selectedTreePath}
        {indexing}
        onSelect={selectNode}
        onExpand={expandNode}
        onSearch={searchDepot}
        onOpenResult={openResult}
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
        <button class:active={centerTab === "pending"} onclick={loadPending}>Pending</button>
        <button class:active={centerTab === "streams"} onclick={loadStreams}>Streams</button>
        <button class:active={centerTab === "repo"} onclick={openRepo}>Repo</button>
      </div>
      {#if centerTab === "streams"}
        <StreamsBrowser
          rows={streamRows}
          loading={streamsLoading}
          currentStream={rootPath}
          onContext={onStreamContext}
        />
      {:else if centerTab === "repo"}
        <DepotTree
          root={repoTree}
          selectedPath={repoSelected}
          onSelect={repoSelect}
          onExpand={repoExpand}
        />
      {:else if centerTab === "pending"}
        <PendingList
          rows={pendingRows}
          loading={pendingLoading}
          client={conn.client}
          onLocalFiles={pendingLocalFiles}
          onShelvedFiles={pendingShelvedFiles}
          onLocalDiff={localDiff}
          onShelvedDiff={shelvedDiff}
          onOpenLocalDiff={openLocalDiff}
          onOpenShelvedDiff={openShelvedDiff}
          onContext={onPendingContext}
          onFileContext={onPendingFileContext}
          onMoveFile={reopenFile}
        />
      {:else}
        <div class="hsplit">
          <div class="hlist">
            <HistoryTable
              mode={histMode}
              subject={histSubject}
              rows={centerRows}
              loading={histLoading}
              more={histMore}
              {haveChange}
              {haveRev}
              {selectedChange}
              onSelectChange={selectChange}
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
              change={selectedChange}
              rows={descRows}
              loading={descLoading}
              onDiff={fileDiff}
              onOpenDiff={openFileDiff}
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

{#if syncProgress}
  <SyncProgressDialog
    title={syncProgress.title}
    count={syncProgress.count}
    current={syncProgress.current}
    issues={syncProgress.issues}
    issueLine={syncProgress.issueLine}
    phase={syncProgress.phase}
    message={syncProgress.message}
    onCancel={cancelSync}
    onClose={() => (syncProgress = null)}
  />
{/if}

{#if syncErrors}
  <SyncErrorDialog
    title={syncErrors.title}
    items={syncErrors.items}
    {busyFile}
    onFixFile={resyncFile}
    onRetryAll={() => resyncAllErrors(false)}
    onForceAll={() => resyncAllErrors(true)}
    onClose={() => (syncErrors = null)}
  />
{/if}

{#if ctxMenu}
  {@const change = ctxMenu.change}
  <ContextMenu
    x={ctxMenu.x}
    y={ctxMenu.y}
    items={[
      { label: `Update to changelist @${change}`, action: () => updateToChange(change) },
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
