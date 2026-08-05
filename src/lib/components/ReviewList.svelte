<script lang="ts">
  //! The Reviews tab: Swarm reviews as expandable rows, like the Pending tab's
  //! changelists — expand a review to see its shelved files, expand a file to
  //! see its diff inline.
  //!
  //! The filters drive server-side queries, so every control here reloads rather
  //! than filtering what is already on screen. Search is debounced for that
  //! reason; the others are discrete enough to fire immediately.
  import DiffView from "$lib/components/DiffView.svelte";
  import { fmtTime, firstLine, splitPath, type P4Record, type ReviewRow } from "$lib/p4";
  import type { Role, StatusFilter } from "$lib/reviews.svelte";

  let {
    rows,
    loading,
    paging,
    more,
    error,
    status,
    user,
    role,
    search,
    me,
    refreshKey,
    contextReview,
    onStatus,
    onUser,
    onRole,
    onSearch,
    onLoadMore,
    onFiles,
    onDiff,
    onOpenDiff,
    onContext,
    onFileContext,
  }: {
    rows: ReviewRow[];
    loading: boolean;
    paging: boolean;
    more: boolean;
    error: string;
    status: StatusFilter;
    user: string;
    role: Role;
    search: string;
    me: string; // the connected user, for the "me" shortcut
    refreshKey: number; // bumps when the list reloads → drop per-review caches
    contextReview: number; // id of the review whose context menu is open
    onStatus: (s: StatusFilter) => void;
    onUser: (u: string) => void;
    onRole: (r: Role) => void;
    onSearch: (q: string) => void;
    onLoadMore: () => void;
    onFiles: (change: string) => Promise<P4Record[]>;
    onDiff: (depotFile: string, rev: number, change: string) => Promise<string>;
    onOpenDiff: (depotFile: string, rev: number, change: string) => void;
    onContext: (r: ReviewRow, e: MouseEvent) => void;
    onFileContext?: (f: P4Record, r: ReviewRow, e: MouseEvent) => void;
  } = $props();

  const STATUSES: { key: StatusFilter; label: string }[] = [
    { key: "open", label: "Open" },
    { key: "needsReview", label: "Needs Review" },
    { key: "needsRevision", label: "Needs Revision" },
    { key: "approved", label: "Approved" },
    { key: "rejected", label: "Rejected" },
    { key: "all", label: "All" },
  ];

  type Expanded = { open: boolean; loading: boolean; files: P4Record[] };
  let exp = $state<Record<number, Expanded>>({});
  // Per-file inline diff, keyed by "<review id>|<depotFile>".
  let fdiff = $state<Record<string, { open: boolean; loading: boolean; text: string }>>({});

  // A reload can change what a review's current version is, so the cached file
  // lists belong to the old list, not the new one.
  let seenKey = $state(-1);
  $effect(() => {
    if (refreshKey !== seenKey) {
      seenKey = refreshKey;
      exp = {};
      fdiff = {};
    }
  });

  async function toggle(r: ReviewRow) {
    const cur = exp[r.id];
    if (cur) {
      exp[r.id] = { ...cur, open: !cur.open };
      return;
    }
    if (!r.change) {
      exp[r.id] = { open: true, loading: false, files: [] };
      return;
    }
    exp[r.id] = { open: true, loading: true, files: [] };
    const files = await onFiles(String(r.change));
    exp[r.id] = { open: exp[r.id]?.open ?? true, loading: false, files };
  }

  async function toggleDiff(r: ReviewRow, f: P4Record) {
    const key = `${r.id}|${f.depotFile}`;
    const cur = fdiff[key];
    if (cur) {
      fdiff[key] = { ...cur, open: !cur.open };
      if (cur.text || cur.loading) return;
    } else {
      fdiff[key] = { open: true, loading: true, text: "" };
    }
    const text = await onDiff(f.depotFile, Number(f.rev ?? 0), String(r.change)).catch(
      (e) => `Could not diff this file: ${e}`,
    );
    fdiff[key] = { open: fdiff[key]?.open ?? true, loading: false, text };
  }

  // Debounced so typing doesn't fire a query per keystroke (Swarm does the
  // searching, so each one is a round-trip).
  let draft = $state("");
  let timer: number | null = null;
  // Mirror the store's value when IT changes; `lastSearch` is deliberately not
  // state, so the effect depends on the prop alone and typing can't re-trigger it.
  let lastSearch = "";
  $effect(() => {
    if (search !== lastSearch) {
      lastSearch = search;
      draft = search;
    }
  });
  function typed(v: string) {
    draft = v;
    if (timer !== null) window.clearTimeout(timer);
    timer = window.setTimeout(() => {
      timer = null;
      onSearch(draft);
    }, 350);
  }
  function flushSearch() {
    if (timer !== null) window.clearTimeout(timer);
    timer = null;
    onSearch(draft);
  }

  let userDraft = $state("");
  let lastUser = "";
  $effect(() => {
    if (user !== lastUser) {
      lastUser = user;
      userDraft = user;
    }
  });

  /** `//depot/stream` of a depot path — enough to see which project a review is on. */
  function depotOf(path: string): string {
    return path.split("/").slice(0, 5).join("/");
  }

  /** "3 days ago"-ish: reviews are read by recency, and a bare date buries it. */
  function ago(epoch: number): string {
    if (!epoch) return "";
    const secs = Math.max(0, Math.floor(Date.now() / 1000) - epoch);
    if (secs < 90) return "just now";
    const mins = Math.round(secs / 60);
    if (mins < 60) return `${mins} min ago`;
    const hours = Math.round(mins / 60);
    if (hours < 24) return `${hours}h ago`;
    const days = Math.round(hours / 24);
    if (days < 30) return `${days}d ago`;
    return fmtTime(String(epoch));
  }
</script>

<div class="panel">
  <div class="filters">
    <select
      class="pick"
      title="Review state (Swarm filters this server-side)"
      value={status}
      onchange={(e) => onStatus(e.currentTarget.value as StatusFilter)}
    >
      {#each STATUSES as s (s.key)}
        <option value={s.key}>{s.label}</option>
      {/each}
    </select>

    <select
      class="pick"
      title="Whether the user below is the review's author or one of its reviewers"
      value={role}
      onchange={(e) => onRole(e.currentTarget.value as Role)}
    >
      <option value="author">Author</option>
      <option value="reviewer">Reviewer</option>
    </select>

    <input
      class="userbox"
      placeholder="any user"
      value={userDraft}
      oninput={(e) => (userDraft = e.currentTarget.value)}
      onchange={() => onUser(userDraft)}
      onkeydown={(e) => {
        if (e.key === "Enter") onUser(userDraft);
        if (e.key === "Escape") {
          userDraft = "";
          onUser("");
        }
      }}
    />
    {#if me && user !== me}
      <button class="mini" title="Filter on my own user" onclick={() => onUser(me)}>me</button>
    {/if}
    {#if user}
      <button class="mini" title="Clear the user filter" onclick={() => onUser("")}>✕</button>
    {/if}

    <input
      class="search"
      placeholder="Search reviews (id / message)"
      value={draft}
      oninput={(e) => typed(e.currentTarget.value)}
      onkeydown={(e) => {
        if (e.key === "Enter") flushSearch();
        if (e.key === "Escape") {
          draft = "";
          flushSearch();
        }
      }}
    />
  </div>

  <div class="body scroll">
    {#if error}
      <div class="msg err">{error}</div>
    {/if}
    {#if loading}
      <div class="msg dim">Loading reviews…</div>
    {:else if rows.length === 0 && !error}
      <div class="msg dim">
        No {status === "all" ? "" : STATUSES.find((s) => s.key === status)?.label.toLowerCase()} reviews{user
          ? ` with ${user} as ${role}`
          : ""}{search ? ` matching “${search}”` : ""}.
      </div>
    {:else}
      {#each rows as r (r.id)}
        {@const s = exp[r.id]}
        <div class="rsec">
          <button
            class="rv"
            class:contextsel={contextReview === r.id}
            onclick={() => toggle(r)}
            oncontextmenu={(e) => onContext(r, e)}
            title={`Review ${r.id}\n${r.description}\n\nshelf: @${r.change}`}
          >
            <span class="tw">{s?.open ? "▾" : "▸"}</span>
            <span class="cnum mono">#{r.id}</span>
            <span class="state st-{r.state}">{r.stateLabel}</span>
            <span class="desc">{firstLine(r.description) || "(no description)"}</span>
            <span class="user dim">{r.author}</span>
            <span class="date dim" title={fmtTime(String(r.updated))}>{ago(r.updated)}</span>
          </button>

          {#if s?.open}
            {#if s.loading}
              <div class="finfo dim">Loading files…</div>
            {:else if !r.change}
              <div class="finfo dim">Swarm reports no version for this review.</div>
            {:else if s.files.length === 0}
              <div class="finfo dim">No shelved files in @{r.change}.</div>
            {:else}
              <!-- The depot comes from the files themselves: Swarm's list
                   endpoint doesn't say which stream a review is on. -->
              <div class="finfo dim">
                @{r.change} · {depotOf(s.files[0].depotFile)} · {s.files.length} file{s.files.length === 1
                  ? ""
                  : "s"}
              </div>
              {#each s.files as f (f.depotFile)}
                {@const fd = fdiff[`${r.id}|${f.depotFile}`]}
                {@const sp = splitPath(f.depotFile)}
                <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                <div
                  class="frow mono"
                  title={"Double-click to open the diff window\n" + f.depotFile}
                  ondblclick={() => onOpenDiff(f.depotFile, Number(f.rev ?? 0), String(r.change))}
                  oncontextmenu={(e) => {
                    if (onFileContext) {
                      e.preventDefault();
                      onFileContext(f, r, e);
                    }
                  }}
                >
                  <button
                    class="fchev"
                    title="Show diff"
                    onclick={(e) => {
                      e.stopPropagation();
                      toggleDiff(r, f);
                    }}
                    ondblclick={(e) => e.stopPropagation()}
                  >
                    {fd?.open ? "▾" : "▸"}
                  </button>
                  <span class="act act-{f.action}">{f.action ?? ""}</span>
                  <span class="fpath"
                    ><span class="pfile">{sp.name}</span><span class="pdir dim">{sp.dir}</span></span
                  >
                  <span class="ftype dim">{f.type ?? ""}</span>
                </div>
                {#if fd?.open}
                  {#if fd.loading}
                    <div class="finfo dim" style="padding-left:36px">Loading diff…</div>
                  {:else if !fd.text.trim()}
                    <div class="finfo dim" style="padding-left:36px">
                      No textual diff (added, binary, or identical).
                    </div>
                  {:else}
                    <DiffView text={fd.text} />
                  {/if}
                {/if}
              {/each}
            {/if}
          {/if}
        </div>
      {/each}

      {#if more}
        <button class="loadmore" disabled={paging} onclick={onLoadMore}>
          {paging ? "Loading…" : "Load more reviews"}
        </button>
      {/if}
    {/if}
  </div>
</div>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg-panel);
    min-height: 0;
  }
  .filters {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    flex: none;
  }
  .pick,
  .userbox,
  .search {
    background: var(--bg-input, var(--bg-panel));
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 3px;
    font-size: 12px;
    padding: 2px 5px;
  }
  .userbox {
    width: 130px;
  }
  .search {
    flex: 1;
    min-width: 80px;
  }
  .mini {
    border: 1px solid var(--border);
    background: none;
    color: var(--text-dim);
    border-radius: 3px;
    font-size: 11px;
    padding: 1px 5px;
    cursor: pointer;
  }
  .mini:hover {
    color: var(--text);
    background: var(--bg-hover);
  }
  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 2px 0;
  }
  .msg {
    padding: 8px 10px;
    font-size: 12px;
  }
  .err {
    color: var(--warn);
  }
  /* Section wrapper = the sticky header's containing block, so a pinned review
     row hands over to the next one instead of overlapping it. */
  .rsec {
    position: relative;
  }
  .rv {
    display: flex;
    align-items: center;
    line-height: 18px;
    gap: 6px;
    width: 100%;
    text-align: left;
    border: none;
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
  .rv:hover {
    background: var(--bg-hover);
  }
  .rv.contextsel {
    background: var(--bg-sel);
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
  .date {
    width: 74px;
    text-align: right;
  }
  /* Same scale as the review badge in the Pending tab, so a state means the same
     thing in both places. */
  .state {
    flex: none;
    font-size: 10px;
    font-weight: 600;
    line-height: 16px;
    padding: 0 6px;
    box-sizing: border-box;
    border-radius: 10px;
    border: 1px solid currentColor;
    white-space: nowrap;
    color: var(--text-dim);
  }
  .st-needsReview {
    color: var(--accent);
  }
  .st-approved {
    color: var(--have);
  }
  .st-needsRevision,
  .st-rejected {
    color: var(--warn);
  }
  .st-archived {
    color: var(--text-dim);
    font-style: italic;
  }
  .finfo {
    padding: 3px 10px 3px 26px;
    font-size: 11px;
    font-style: italic;
  }
  .frow {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 10px 2px 20px;
    font-size: 12px;
    white-space: nowrap;
    cursor: default;
  }
  .frow:hover {
    background: var(--bg-hover);
  }
  .fchev {
    flex: none;
    width: 14px;
    border: none;
    background: none;
    color: var(--text-dim);
    font-size: 10px;
    padding: 0;
    cursor: pointer;
  }
  .act {
    flex: none;
    width: 52px;
    font-size: 10px;
    color: var(--text-dim);
  }
  .act-add {
    color: var(--have);
  }
  .act-delete {
    color: var(--warn);
  }
  .fpath {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pdir {
    font-size: 11px;
    margin-left: 6px;
  }
  .ftype {
    flex: none;
    font-size: 11px;
  }
  .loadmore {
    display: block;
    width: calc(100% - 20px);
    margin: 6px 10px 10px;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 3px;
    background: none;
    color: var(--text-dim);
    font-size: 12px;
    cursor: pointer;
  }
  .loadmore:hover:not(:disabled) {
    color: var(--text);
    background: var(--bg-hover);
  }
</style>
