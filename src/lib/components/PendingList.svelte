<script lang="ts">
  import { fmtTime, firstLine, splitPath, type P4Record, type ReviewInfo } from "$lib/p4";
  import DiffView from "$lib/components/DiffView.svelte";

  let {
    rows,
    loading,
    client,
    refreshKey,
    reviews,
    offline,
    offlineScanning,
    offlineScannedAt,
    offlineCached,
    onOfflineDiff,
    onOpenOfflineDiff,
    needsResolve,
  isUnchanged,
    onLocalFiles,
    onLocalFilesCached,
    onShelvedFiles,
    onShelvedFilesCached,
    onLocalDiff,
    onShelvedDiff,
    onOpenLocalDiff,
    onOpenShelvedDiff,
    contextChange,
    onContext,
    onFileContext,
    onShelvedContext,
    onOfflineContext,
    onMoveFile,
  }: {
    rows: P4Record[];
    loading: boolean;
    client: string; // resets the per-CL cache when the workspace changes
    refreshKey: number; // bumps when pending data changes → refetch open CLs' files
    reviews: Record<string, ReviewInfo | null>; // change → Swarm review status
    offline: P4Record[]; // files changed on disk but not open in any changelist
    offlineScanning: boolean; // an offline-change scan is in progress
    offlineScannedAt: number | null; // when the last scan completed (freshness)
    offlineCached: boolean; // false = never scanned (vs a cached empty result)
    onOfflineDiff: (depotFile: string) => Promise<string>; // forced local-vs-server diff
    onOpenOfflineDiff: (depotFile: string) => void; // open the offline diff externally
    needsResolve: (depotFile: string) => boolean; // p4 is holding a resolve on it
    // Open for edit but identical to the depot: a checkout with nothing in it.
    isUnchanged: (depotFile: string) => boolean;
    contextChange: string; // the changelist whose context menu is open (highlight it)
    onLocalFiles: (change: string) => Promise<P4Record[]>; // opened (workspace) files
    // cached opened/shelved files (all cache layers, resolves in ~ms);
    // undefined = never fetched (loading)
    onLocalFilesCached: (change: string) => Promise<P4Record[] | undefined>;
    onShelvedFiles: (change: string) => Promise<P4Record[]>; // shelved files
    onShelvedFilesCached: (change: string) => Promise<P4Record[] | undefined>;
    onLocalDiff: (depotFile: string) => Promise<string>; // local vs server
    onShelvedDiff: (depotFile: string, rev: number, change: string) => Promise<string>;
    onOpenLocalDiff: (depotFile: string) => void;
    onOpenShelvedDiff: (depotFile: string, rev: number, change: string) => void;
    onContext: (cl: P4Record, e: MouseEvent) => void; // right-click a changelist
    // right-click a file → (file, change, event, selected depot files)
    onFileContext: (file: P4Record, change: string, e: MouseEvent, files: string[]) => void;
    // right-click a shelved file; `files` is the shelved selection it belongs to
    onShelvedContext?: (file: P4Record, change: string, e: MouseEvent, files: string[]) => void;
    // right-click an offline file → (file, event, selected depot files)
    onOfflineContext?: (file: P4Record, e: MouseEvent, files: string[]) => void;
    onMoveFile: (files: string[], toChange: string) => void; // drag files onto another CL
  } = $props();

  // Multi-select of local (opened) AND offline files via click / Ctrl+click /
  // Shift+click — one selection set, keyed by depot path.
  let selected = $state<Set<string>>(new Set());
  let anchor: string | null = null;
  // Selectable files in render order (open changelists, then the open Offline
  // section), for Shift-range.
  const orderedFiles = $derived.by(() => {
    const out: string[] = [];
    for (const r of rows) {
      const s = cls[r.change];
      if (s?.open) for (const f of localOf(String(r.change))) if (f.depotFile) out.push(f.depotFile);
    }
    if (offlineOpen) for (const f of offline) if (f.depotFile) out.push(f.depotFile);
    return out;
  });
  function clickFile(file: string, e: MouseEvent | KeyboardEvent) {
    if (e.shiftKey && anchor) {
      const a = orderedFiles.indexOf(anchor);
      const b = orderedFiles.indexOf(file);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        selected = new Set(orderedFiles.slice(lo, hi + 1));
      }
    } else if (e.ctrlKey || e.metaKey) {
      const n = new Set(selected);
      if (n.has(file)) n.delete(file);
      else n.add(file);
      selected = n;
      anchor = file;
    } else {
      selected = new Set([file]);
      anchor = file;
    }
  }
  function clearSelection() {
    selected = new Set();
    anchor = null;
  }

  // --- shelved selection -----------------------------------------------------
  // Kept apart from the opened-file selection: the same path can be open AND
  // shelved in one changelist, and the actions differ completely (revert a file
  // vs remove its shelved copy). Scoped to ONE shelf, because shelve and
  // unshelve are per-changelist — clicking into another changelist's shelf
  // starts a new selection there rather than building a set no action can take.
  let shelvedOf = $state(""); // which changelist's shelf the selection is in
  let shelvedSel = $state<Set<string>>(new Set());
  let shelvedAnchor: string | null = null;

  /** The shelved rows of `change`, in display order — the range a shift-click
   *  spans. */
  function shelvedOrder(change: string): string[] {
    return (cls[change]?.shelved ?? []).map((f) => String(f.depotFile));
  }
  function clickShelved(change: string, file: string, e: MouseEvent | KeyboardEvent) {
    if (change !== shelvedOf) {
      // A different shelf: start over there.
      shelvedOf = change;
      shelvedSel = new Set([file]);
      shelvedAnchor = file;
      return;
    }
    const order = shelvedOrder(change);
    if (e.shiftKey && shelvedAnchor) {
      const a = order.indexOf(shelvedAnchor);
      const b = order.indexOf(file);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        shelvedSel = new Set(order.slice(lo, hi + 1));
      }
    } else if (e.ctrlKey || e.metaKey) {
      const n = new Set(shelvedSel);
      if (n.has(file)) n.delete(file);
      else n.add(file);
      shelvedSel = n;
      shelvedAnchor = file;
    } else {
      shelvedSel = new Set([file]);
      shelvedAnchor = file;
    }
  }

  // Rubber-band (marquee) selection: drag over empty space to box-select local
  // file rows. Rows carry data-file; hit-testing is done in viewport (client)
  // coords so it works regardless of scroll.
  let bodyEl: HTMLDivElement;
  let marquee = $state<{ left: number; top: number; width: number; height: number } | null>(null);
  let marqStart: { x: number; y: number } | null = null;
  let marqBase: Set<string> = new Set(); // selection to preserve when Ctrl-adding

  function bodyMouseDown(e: MouseEvent) {
    // Only from empty space (not a row/button, which are child targets) and LMB.
    if (e.button !== 0 || e.target !== e.currentTarget) return;
    e.preventDefault();
    const additive = e.ctrlKey || e.metaKey;
    marqStart = { x: e.clientX, y: e.clientY };
    marqBase = additive ? new Set(selected) : new Set();
    if (!additive) clearSelection();
    window.addEventListener("mousemove", bodyMouseMove);
    window.addEventListener("mouseup", bodyMouseUp);
  }
  function bodyMouseMove(e: MouseEvent) {
    if (!marqStart) return;
    const r = bodyEl.getBoundingClientRect();
    const cx = Math.max(r.left, Math.min(e.clientX, r.right));
    const cy = Math.max(r.top, Math.min(e.clientY, r.bottom));
    const x0 = Math.min(marqStart.x, cx),
      x1 = Math.max(marqStart.x, cx);
    const y0 = Math.min(marqStart.y, cy),
      y1 = Math.max(marqStart.y, cy);
    marquee = { left: x0, top: y0, width: x1 - x0, height: y1 - y0 };
    const next = new Set(marqBase);
    for (const el of bodyEl.querySelectorAll<HTMLElement>("[data-file]")) {
      const b = el.getBoundingClientRect();
      if (b.bottom >= y0 && b.top <= y1) next.add(el.dataset.file!);
    }
    selected = next;
  }
  function bodyMouseUp() {
    marqStart = null;
    marquee = null;
    window.removeEventListener("mousemove", bodyMouseMove);
    window.removeEventListener("mouseup", bodyMouseUp);
  }

  // Drag-and-drop: move an opened file from one changelist to another.
  let drag = $state<{ files: string[]; from: string } | null>(null);
  let dragOver = $state<string | null>(null); // CL currently hovered as a drop target

  type CL = {
    open: boolean;
    loading: boolean;
    local: P4Record[];
    shelved: P4Record[];
    shelvedOpen: boolean;
  };
  let cls = $state<Record<string, CL>>({});

  // Files the UI is pretending are gone, the changelist each was hidden from, and
  // whether the command it was covering has finished.
  //
  // This is what makes an optimistic removal survive: a `p4 opened` answer that
  // was already in flight when the user acted cannot resurrect the row (the
  // startup case, where those fetches are still landing), and neither can the
  // stale-while-revalidate paint of the refetch that follows.
  //
  // An entry clears when an authoritative fetch of its changelist comes back
  // WITHOUT the file — the command really happened. While the command is still
  // running, a fetch that still lists the file changes nothing: our guess is
  // about a state p4 has not reached yet.
  //
  // `settled` closes the loop. Once a mutation has completed (every one reloads
  // the pending list, which settles what is hidden), p4's answer wins: a settled
  // entry is dropped whatever the fetch says, so a guess that turned out wrong —
  // or a file reverted and then checked out again inside the same window — cannot
  // leave a row invisible for the rest of the session.
  interface Hide {
    change: string;
    settled: boolean;
  }
  let hidden = $state<Map<string, Hide>>(new Map());

  /** Which expanded changelist currently lists `file` ("*" when none does — it
   *  is not on screen, so hiding it is free and any fetch may clear it). */
  function changeHolding(file: string): string {
    for (const [change, cl] of Object.entries(cls)) {
      if (cl.local.some((f) => String(f.depotFile) === file)) return change;
    }
    return "*";
  }

  /** Hide these files from the list at once, and return the undo. Called by the
   *  store's optimistic path so the rows react before p4 answers. */
  export function forgetRows(files: string[]): () => void {
    if (!files.length) return () => {};
    const next = new Map(hidden);
    for (const f of files) next.set(f, { change: changeHolding(f), settled: false });
    hidden = next;
    return () => {
      const back = new Map(hidden);
      for (const f of files) back.delete(f);
      hidden = back;
    };
  }

  /** The commands behind every current guess have finished: from here on, the
   *  next authoritative answer decides, whatever it says. */
  export function settleRows() {
    if (!hidden.size) return;
    let changed = false;
    const next = new Map(hidden);
    for (const [file, h] of next) {
      if (!h.settled) {
        next.set(file, { ...h, settled: true });
        changed = true;
      }
    }
    if (changed) hidden = next;
  }

  /** The depot paths a changelist is currently showing. The store's caches can be
   *  cold (a fresh boot reads them from SQLite asynchronously) while these rows
   *  are already on screen, so a changelist-wide action asks the list rather than
   *  the cache — otherwise it optimistically removes nothing. */
  export function rowsOf(change: string): string[] {
    return localOf(change).map((f) => String(f.depotFile));
  }

  /** A changelist's rows, minus anything being optimistically removed. */
  function localOf(change: string): P4Record[] {
    const list = cls[change]?.local ?? [];
    if (!hidden.size) return list;
    return list.filter((f) => !hidden.has(String(f.depotFile)));
  }

  /** Stop hiding files that this changelist's fresh list no longer contains. */
  function reconcileHidden(change: string, fresh: P4Record[]) {
    if (!hidden.size) return;
    const present = new Set(fresh.map((f) => String(f.depotFile)));
    const next = new Map(hidden);
    for (const [file, h] of hidden) {
      if (h.change !== change && h.change !== "*") continue;
      if (!present.has(file) || h.settled) next.delete(file);
    }
    if (next.size !== hidden.size) hidden = next;
  }

  // Per-file inline diff, keyed by "<change>|<kind>|<depotFile>".
  let fdiff = $state<Record<string, { open: boolean; loading: boolean; text: string }>>({});

  // Stale-while-revalidate: keep any existing files visible during a refetch so
  // a refresh (e.g. after moving a file) doesn't flash "Loading…".
  async function loadCL(change: string) {
    const prev = cls[change];
    // Paint the cached opened + shelved files first (so an expanded CL isn't
    // empty — or flashing the empty state — while p4 runs); the fetch reconciles.
    // The cache read is async because it falls through to SQLite (localStorage
    // is a bounded mirror and evicts), but it resolves in ~ms — not p4 time.
    // `undefined` from the cache means NEVER fetched (show loading); `[]` means
    // fetched-and-empty (a genuine empty CL — render it empty, no loading flash).
    if (!prev) {
      cls[change] = { open: true, loading: true, local: [], shelved: [], shelvedOpen: false };
    }
    const [cachedLocal, cachedShelved] = prev
      ? [prev.local, prev.shelved]
      : await Promise.all([onLocalFilesCached(change), onShelvedFilesCached(change)]);
    const known = cachedLocal !== undefined || cachedShelved !== undefined;
    cls[change] = {
      open: cls[change]?.open ?? true,
      // Spinner only when nothing is known yet — not for a cached empty CL.
      loading: !prev && !known,
      local: cachedLocal ?? [],
      shelved: cachedShelved ?? [],
      shelvedOpen: cls[change]?.shelvedOpen ?? false,
    };
    const [local, shelved] = await Promise.all([onLocalFiles(change), onShelvedFiles(change)]);
    const local2 = local.filter((f) => f.depotFile);
    const shelved2 = shelved.filter((f) => f.depotFile);
    reconcileHidden(change, local2);
    cls[change] = {
      open: cls[change]?.open ?? true,
      loading: false,
      local: local2,
      shelved: shelved2,
      shelvedOpen: cls[change]?.shelvedOpen ?? false,
    };
  }

  // Optimistic move: reflect the move in the UI immediately (we already have the
  // file record), then fire the p4 command. The reload it triggers reconciles —
  // and rolls back — once p4 answers, so a failed move snaps back on its own.
  // Exported so the right-click "Move to changelist" menu shares this one path.
  /** The selected depot files, for the window's keyboard shortcuts. */
  export function selection(): string[] {
    return [...selected];
  }
  /** The changelist the selection sits in ("" when nothing is selected, or the
   *  selection spans several). A shortcut that acts on a changelist needs one
   *  unambiguous answer. */
  export function selectedChange(): string {
    const owners = new Set<string>();
    for (const [change, cl] of Object.entries(cls)) {
      for (const f of localOf(change)) if (selected.has(String(f.depotFile))) owners.add(change);
    }
    return owners.size === 1 ? [...owners][0] : "";
  }

  /** Move files between changelists, updating the expanded lists at once so the
   *  rows jump immediately; the reload that follows reconciles. */
  export function moveFiles(files: string[], from: string, to: string) {
    const gone = new Set(files);
    const src = cls[from];
    const recs = localOf(from).filter((f) => gone.has(String(f.depotFile)));
    if (src && recs.length) {
      cls[from] = { ...src, local: src.local.filter((f) => !gone.has(String(f.depotFile))) };
      const dst = cls[to];
      if (dst) cls[to] = { ...dst, local: [...dst.local, ...recs.map((r) => ({ ...r }))] };
    }
    onMoveFile(files, to);
  }
  /** One file — the context menu's "Move to changelist" for a single row. */
  export function moveFile(file: string, from: string, to: string) {
    moveFiles([file], from, to);
  }

  function toggleCL(change: string) {
    const cur = cls[change];
    if (!cur) {
      loadCL(change);
      return;
    }
    cls[change] = { ...cur, open: !cur.open };
  }
  function toggleShelved(change: string) {
    const cur = cls[change];
    if (cur) cls[change] = { ...cur, shelvedOpen: !cur.shelvedOpen };
  }

  // Expand every changelist by default. Reset the cache when the workspace
  // changes — CL keys like "default" are reused across workspaces. When
  // refreshKey bumps (a pending mutation reloaded the list), refetch the files
  // of every already-open changelist so moved/reverted files show immediately.
  let lastClient = "";
  let lastKey = -1;
  $effect(() => {
    const key = refreshKey;
    if (client !== lastClient) {
      lastClient = client;
      cls = {};
      hidden = new Map();
      fdiff = {};
      selected = new Set();
      anchor = null;
    }
    const forced = key !== lastKey;
    lastKey = key;
    for (const r of rows) {
      if (!cls[r.change]) loadCL(r.change);
      else if (forced && cls[r.change].open) loadCL(r.change);
    }
  });

  async function toggleFileDiff(change: string, kind: "local" | "shelved", f: P4Record) {
    const key = `${change}|${kind}|${f.depotFile}`;
    const cur = fdiff[key];
    if (cur?.open) {
      fdiff[key] = { ...cur, open: false };
      return;
    }
    if (cur && !cur.loading) {
      fdiff[key] = { ...cur, open: true };
      return;
    }
    fdiff[key] = { open: true, loading: true, text: "" };
    const text =
      kind === "local"
        ? await onLocalDiff(f.depotFile)
        : await onShelvedDiff(f.depotFile, Number(f.rev), change);
    fdiff[key] = { open: true, loading: false, text };
  }
  function openExt(change: string, kind: "local" | "shelved", f: P4Record) {
    if (kind === "local") onOpenLocalDiff(f.depotFile);
    else onOpenShelvedDiff(f.depotFile, Number(f.rev), change);
  }
  // Inline diff for offline (unopened) files — forced local-vs-server diff.
  let offlineDiffs = $state<Record<string, { open: boolean; loading: boolean; text: string }>>({});
  async function toggleOfflineDiff(f: P4Record) {
    const key = f.depotFile ?? f.clientFile ?? "";
    const cur = offlineDiffs[key];
    if (cur?.open) {
      offlineDiffs[key] = { ...cur, open: false };
      return;
    }
    if (cur && !cur.loading) {
      offlineDiffs[key] = { ...cur, open: true };
      return;
    }
    offlineDiffs[key] = { open: true, loading: true, text: "" };
    const text = await onOfflineDiff(f.depotFile ?? "");
    offlineDiffs[key] = { open: true, loading: false, text };
  }

  let offlineOpen = $state(true); // the "Offline changes" group is expanded
</script>

<div class="panel">
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class="scroll body"
    bind:this={bodyEl}
    onmousedown={bodyMouseDown}
    onkeydown={(e) => e.key === "Escape" && clearSelection()}
  >
    {#if loading}
      <div class="msg dim">Loading…</div>
    {:else if rows.length === 0}
      <div class="msg dim">{client ? "No pending changelists." : "Select a workspace to browse."}</div>
    {:else}
      {#each rows as r (r.change)}
        {@const s = cls[r.change]}
        {@const rv = reviews[r.change]}
        {@const shown = localOf(String(r.change))}
        {@const empty = !!s && !s.loading && shown.length === 0 && s.shelved.length === 0}
        <!-- Section wrapper = the sticky header's containing block, so the pinned
             CL row is pushed out by its own section's end (clean hand-off to the
             next CL) instead of being painted over mid-overlap. -->
        <!-- The drop target is the whole SECTION — title row and file list
             alike. Aiming at the title alone meant an expanded changelist was
             mostly a dead zone, and the obvious place to drop a file is among the
             files it will join. -->
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div
          class="clsec"
          class:dropinto={dragOver === r.change}
          ondragover={(e) => {
            if (drag && drag.from !== r.change) {
              e.preventDefault();
              dragOver = r.change;
            }
          }}
          ondragleave={(e) => {
            // Only when the pointer actually leaves the section: moving between
            // its own rows fires dragleave on each of them.
            const to = e.relatedTarget as Node | null;
            if (dragOver === r.change && (!to || !e.currentTarget.contains(to))) dragOver = null;
          }}
          ondrop={(e) => {
            e.preventDefault();
            if (drag && drag.from !== r.change) moveFiles(drag.files, drag.from, r.change);
            drag = null;
            dragOver = null;
          }}
        >
        <button
          class="cl"
          class:contextsel={contextChange === r.change}
          onclick={() => toggleCL(r.change)}
          oncontextmenu={(e) => onContext(r, e)}
        >
          <span class="tw">{empty ? "" : s?.open ? "▾" : "▸"}</span>
          <span class="cnum mono">{r.change === "default" ? "Default" : "@" + r.change}</span>
          <span class="desc" title={r.desc}>
            {r.change === "default" ? "" : firstLine(r.desc) || "(no description)"}
          </span>
          {#if rv}
            <span
              class="review rv-{rv.state}"
              title={"Swarm review" + (rv.id ? " #" + rv.id : "") + ": " + rv.stateLabel}
            >
              {rv.stateLabel}
            </span>
          {/if}
          <span class="user dim">{r.user}</span>
          <span class="date dim">{fmtTime(r.time)}</span>
        </button>
        {#if s?.open && !empty}
          {#if s.loading}
            <div class="finfo dim">Loading files…</div>
          {:else}
            {#if s.shelved.length}
              <!-- Same sticky treatment as the CL header: pinned just below it
                   (top = CL header height), bounded by its own section. -->
              <div class="shsec">
                <button class="subfolder" onclick={() => toggleShelved(r.change)}>
                  <span class="tw">{s.shelvedOpen ? "▾" : "▸"}</span>
                  <span class="ic">📁</span> Shelved <span class="dim">({s.shelved.length})</span>
                </button>
                {#if s.shelvedOpen}
                  {#each s.shelved as f (f.depotFile)}
                    {@render fileRow(f, r.change, "shelved", 2)}
                  {/each}
                {/if}
              </div>
            {/if}
            {#each shown as f (f.depotFile)}
              {@render fileRow(f, r.change, "local", 1)}
            {/each}
          {/if}
        {/if}
        </div>
      {/each}

      <!-- Always present, so a rescan (every refresh) can't make the section pop
           in and out. "scanning" shows only when nothing is cached yet: an empty
           result is a real answer (zero offline files), not a loading state. -->
      <div class="clsec">
        <button class="cl offlinehdr" onclick={() => (offlineOpen = !offlineOpen)}>
          <span class="tw">{offlineOpen ? "▾" : "▸"}</span>
          <span class="cnum mono">Offline</span>
          <span class="desc">modified on disk, not checked out</span>
          <!-- Freshness: this list is a cached scan result — make its age visible. -->
          <span class="date dim">
            {!offlineCached
              ? offlineScanning
                ? "scanning…"
                : "not scanned yet"
              : offlineScannedAt
                ? "scanned " +
                  new Date(offlineScannedAt).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
                : "cached"}
          </span>
          <span class="user dim">{offlineCached ? offline.length : ""}</span>
        </button>
        {#if offlineOpen}
          {#if !offlineCached}
            <div class="finfo dim">{offlineScanning ? "Scanning…" : "Not scanned yet."}</div>
          {:else if offline.length === 0}
            <div class="finfo dim">No offline changes.</div>
          {:else}
            {#each offline as f (f.clientFile ?? f.depotFile)}
              {@const key = f.depotFile ?? f.clientFile ?? ""}
              {@const od = offlineDiffs[key]}
              {@const sp = splitPath(f.depotFile ?? f.clientFile ?? "")}
              <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
              <div
                class="frow mono"
                data-file={f.depotFile}
                class:selected={!!f.depotFile && selected.has(f.depotFile)}
                style="padding-left:20px"
                title={f.desync
                  ? "Not a local edit: the file matches the latest revision but the sync record is behind (interrupted sync). Right-click → Repair sync record.\n" +
                    (f.clientFile ?? f.depotFile ?? "")
                  : "Double-click to open in external diff\n" + (f.clientFile ?? f.depotFile ?? "")}
                onclick={(e) => f.depotFile && clickFile(f.depotFile, e)}
                onkeydown={(e) => {
                  if (f.depotFile && (e.key === "Enter" || e.key === " ")) {
                    e.preventDefault();
                    clickFile(f.depotFile, e);
                  }
                }}
                ondblclick={() => onOpenOfflineDiff(f.depotFile ?? "")}
                oncontextmenu={(e) => {
                  if (onOfflineContext) {
                    e.preventDefault();
                    // Right-click selects the item (unless already in the selection).
                    if (f.depotFile && !selected.has(f.depotFile)) {
                      selected = new Set([f.depotFile]);
                      anchor = f.depotFile;
                    }
                    onOfflineContext(f, e, [...selected]);
                  }
                }}
              >
                <button class="fchev" title="Show diff" onclick={() => toggleOfflineDiff(f)}>
                  {od?.open ? "▾" : "▸"}
                </button>
                {#if f.desync}
                  <span class="act act-desync">desync</span>
                {:else}
                  <!-- `reason` is set for rows a sync could not write (see
                       p4_sync_blockers): the state is the status, and the
                       explanation travels with it. -->
                  <span class="act act-{f.action}" title={String(f.reason ?? "")}
                    >{f.action ?? ""}</span
                  >
                {/if}
                <span class="fpath"><span class="pfile">{sp.name}</span><span class="pdir dim">{sp.dir}</span></span>
              </div>
              {#if od?.open}
                {#if od.loading}
                  <div class="finfo dim" style="padding-left:36px">Loading diff…</div>
                {:else if !od.text.trim()}
                  <div class="finfo dim" style="padding-left:36px">No diff (identical, or file gone).</div>
                {:else}
                  <DiffView text={od.text} />
                {/if}
              {/if}
            {/each}
          {/if}
        {/if}
      </div>
    {/if}
  </div>
  {#if marquee}
    <div
      class="marquee"
      style="left:{marquee.left}px;top:{marquee.top}px;width:{marquee.width}px;height:{marquee.height}px"
    ></div>
  {/if}
</div>

{#snippet fileRow(f: P4Record, change: string, kind: "local" | "shelved", depth: number)}
  {@const key = `${change}|${kind}|${f.depotFile}`}
  {@const fd = fdiff[key]}
  {@const sp = splitPath(f.depotFile)}
  <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
  <div
    class="frow mono"
    data-file={kind === "local" ? f.depotFile : undefined}
    class:dragging={drag?.files.includes(f.depotFile)}
    class:selected={kind === "local"
      ? selected.has(f.depotFile)
      : shelvedOf === change && shelvedSel.has(f.depotFile)}
    style="padding-left:{depth * 16 + 4}px"
    title={"Double-click to open in external diff\n" + f.depotFile}
    draggable={kind === "local"}
    onclick={(e) =>
      kind === "local" ? clickFile(f.depotFile, e) : clickShelved(change, f.depotFile, e)}
    onkeydown={(e) => {
      if (e.key !== "Enter" && e.key !== " ") return;
      e.preventDefault();
      if (kind === "local") clickFile(f.depotFile, e);
      else clickShelved(change, f.depotFile, e);
    }}
    ondblclick={() => openExt(change, kind, f)}
    oncontextmenu={(e) => {
      if (kind === "local") {
        e.preventDefault();
        // Right-click selects the item (unless it's already in the selection).
        if (!selected.has(f.depotFile)) {
          selected = new Set([f.depotFile]);
          anchor = f.depotFile;
        }
        onFileContext(f, change, e, [...selected]);
      } else if (onShelvedContext) {
        e.preventDefault();
        // Right-click selects the row unless it is already in the selection —
        // same rule as the opened files, so a menu never acts on something
        // invisible.
        if (shelvedOf !== change || !shelvedSel.has(f.depotFile)) {
          shelvedOf = change;
          shelvedSel = new Set([f.depotFile]);
          shelvedAnchor = f.depotFile;
        }
        onShelvedContext(f, change, e, [...shelvedSel]);
      }
    }}
    ondragstart={(e) => {
      if (kind !== "local") return;
      // Dragging a row that is part of the selection drags the SELECTION;
      // grabbing an unselected row drags (and selects) just that one, which is
      // what makes a stray drag predictable.
      let files: string[];
      if (selected.has(f.depotFile)) {
        files = [...selected];
      } else {
        files = [f.depotFile];
        selected = new Set(files);
        anchor = f.depotFile;
      }
      drag = { files, from: change };
      if (e.dataTransfer) {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", files.join("\n"));
      }
    }}
    ondragend={() => {
      drag = null;
      dragOver = null;
    }}
  >
    <button
      class="fchev"
      title="Show diff"
      onclick={(e) => {
        e.stopPropagation();
        toggleFileDiff(change, kind, f);
      }}
      ondblclick={(e) => e.stopPropagation()}
    >
      {fd?.open ? "▾" : "▸"}
    </button>
    <!-- `p4 diff -sr`: open for edit and byte-identical to the depot, so there is
         nothing in it to submit. Its own status rather than a note beside "edit",
         because that is what it is — and a status is where the eye already looks
         to see what a row will do on submit.
         "writable" rather than "unchanged": the checkout is what the file HAS
         (it is checked out and writable), while having no changes is only true
         until the next keystroke. A status should not read as a verdict. -->
    {#if kind === "local" && isUnchanged(f.depotFile)}
      <span
        class="act act-unchanged"
        title="Checked out and writable, but still identical to the depot revision — nothing to submit yet. Reverting it loses nothing."
      >writable</span>
    {:else}
      <span class="act act-{f.action}">{f.action ?? ""}</span>
    {/if}
    <span class="fpath"><span class="pfile">{sp.name}</span><span class="pdir dim">{sp.dir}</span></span>
    {#if kind === "local" && needsResolve(f.depotFile)}
      <span class="unres">needs resolve</span>
    {/if}
    <span class="ftype dim">{f.type ?? ""}</span>
  </div>
  {#if fd?.open}
    {#if fd.loading}
      <div class="finfo dim" style="padding-left:{depth * 16 + 20}px">Loading diff…</div>
    {:else if !fd.text.trim()}
      <div class="finfo dim" style="padding-left:{depth * 16 + 20}px">
        No textual diff (added, binary, or identical).
      </div>
    {:else}
      <DiffView text={fd.text} />
    {/if}
  {/if}
{/snippet}

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
  }
  .body {
    flex: 1;
    /* Bottom only: top padding on a scroller leaves a band above a top:0 sticky
       header — the header stops at the content edge, so rows scroll visibly
       through it. That band was the few-pixel gap under the tab bar. */
    padding: 0 0 2px;
  }
  .marquee {
    position: fixed;
    z-index: 50;
    pointer-events: none;
    border: 1px solid var(--accent);
    background: var(--bg-sel);
    opacity: 0.35;
  }
  .cl {
    display: flex;
    /* center + fixed line box so the async review pill (see .review) fits inside
       the line and its arrival doesn't change the row height. */
    align-items: center;
    line-height: 18px;
    gap: 6px;
    width: 100%;
    text-align: left;
    border: none;
    /* Sticky: the owning changelist stays pinned at the top while its (long) file
       list scrolls, so files are always attributable; the next CL row slides in
       over it. Needs the opaque background (was none). */
    position: sticky;
    top: 0;
    z-index: 2;
    background: var(--bg-panel);
    border-radius: 0;
    padding: 4px 10px;
    font-size: 12px;
    color: var(--text);
    white-space: nowrap;
    cursor: pointer;
    border-bottom: 1px solid var(--border);
    user-select: none;
  }
  .cl:hover {
    background: var(--bg-hover);
  }
  .clsec.dropinto {
    background: var(--bg-sel);
    outline: 1px dashed var(--accent);
    outline-offset: -2px;
  }
  .tw {
    flex: none;
    width: 12px;
    color: var(--text-dim);
    font-size: 10px;
  }
  .cnum {
    flex: none;
    font-weight: 600;
  }
  .desc {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .user,
  .date {
    flex: none;
    font-size: 11px;
  }
  .review {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    /* total height = 16 (line) + 2 (border) = 18px = the row's line box, so the
       badge sits within the line and never grows the row when it lands. */
    line-height: 16px;
    padding: 0 6px;
    box-sizing: border-box;
    border-radius: 10px;
    border: 1px solid currentColor;
    white-space: nowrap;
    color: var(--text-dim);
  }
  .rv-needsReview {
    color: var(--accent);
  }
  .rv-approved {
    color: var(--have);
  }
  .rv-needsRevision,
  .rv-rejected {
    color: var(--warn);
  }
  .rv-requested,
  .rv-archived {
    color: var(--text-dim);
    font-style: italic;
  }
  .subfolder {
    display: flex;
    align-items: center;
    gap: 5px;
    width: 100%;
    text-align: left;
    border: none;
    /* Sticky under the CL header (top = its 27px: 4+4 padding + 18 line + 1
       border), bounded by .shsec; below the header's z-index so it slides under. */
    position: sticky;
    top: 27px;
    z-index: 1;
    background: var(--bg-panel);
    border-radius: 0;
    padding: 3px 10px 3px 20px;
    font-size: 12px;
    color: var(--text);
    cursor: pointer;
    user-select: none;
  }
  .subfolder:hover {
    background: var(--bg-hover);
  }
  .finfo {
    padding: 3px 10px;
    font-size: 11px;
    font-style: italic;
  }
  .frow {
    display: flex;
    align-items: baseline;
    gap: 5px;
    padding: 2px 10px 2px 4px;
    font-size: 12px;
    white-space: nowrap;
    cursor: default;
    user-select: none;
  }
  .frow:hover {
    background: var(--bg-hover);
  }
  .frow[draggable="true"] {
    cursor: grab;
  }
  .frow.dragging {
    opacity: 0.4;
  }
  .frow.selected {
    background: var(--bg-sel);
  }
  .frow.selected:hover {
    background: var(--bg-sel);
  }
  .cl.contextsel {
    background: var(--bg-sel);
  }
  .fchev {
    flex: none;
    border: none;
    background: none;
    border-radius: 0;
    padding: 0 2px;
    color: var(--text-dim);
    cursor: pointer;
    font-size: 10px;
    line-height: 1;
  }
  .fchev:hover {
    color: var(--text);
  }
  .act {
    flex: none;
    text-transform: capitalize;
    /* Wide enough for the longest status p4 can report. Measured at the row's
       12px Segoe UI: "Move/Delete" is 68px and "Move/Add" 57px, against the 52px
       that 4rem used to give (the root font is 13px) — both were overflowing into
       the path column on any rename. 5.6rem = 72.8px, ~5px of slack. */
    width: 5.6rem;
  }
  .act-add {
    color: var(--have);
  }
  .act-delete {
    color: var(--warn);
  }
  .act-edit {
    color: var(--accent);
  }
  .act-desync {
    color: var(--text-dim);
    font-style: italic;
  }
  /* Its own hue: add is green, edit is blue, delete is red, and a checkout that
     holds nothing yet is amber. Same size as every other status — "Writable" is
     44px, well inside the column. */
  .act-unchanged,
  /* A file that is writable where a depot revision belongs: same state, same
     colour, whether it turned up in a changelist or blocked a sync. */
  .act-writable {
    color: var(--writable);
  }
  /* Perforce has no record of this file at all — it is why a sync stopped, and
     nothing else in the app can see it. */
  .act-untracked {
    color: var(--warn);
  }
  .unres {
    flex: none;
    font-size: 10px;
    color: var(--danger, #d76a6a);
    border: 1px solid currentColor;
    border-radius: 999px;
    padding: 0 6px;
    margin-left: 6px;
  }
  .fpath {
    display: flex;
    align-items: baseline;
    gap: 8px;
    flex: 1;
    min-width: 0;
    overflow: hidden;
  }
  .fpath .pfile {
    flex: none;
    white-space: nowrap;
  }
  .fpath .pdir {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .ftype {
    flex: none;
    font-size: 11px;
  }
  .msg {
    padding: 12px;
    font-size: 12px;
  }
</style>
