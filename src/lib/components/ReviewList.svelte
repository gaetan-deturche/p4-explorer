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
  import { REVIEW_STATES, type ReviewContent, type Role } from "$lib/reviews.svelte";

  let {
    rows,
    loading,
    paging,
    more,
    error,
    states,
    user,
    role,
    search,
    streamOnly,
    streamPath,
    hideSubmitted,
    me,
    refreshKey,
    contextReview,
    onToggleState,
    onUser,
    onRole,
    onSearch,
    onStreamOnly,
    onHideSubmitted,
    onLoadMore,
    onContent,
    onContentCached,
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
    states: string[]; // ticked review states (same keys as the row badges)
    user: string;
    role: Role;
    search: string;
    streamOnly: boolean;
    streamPath: string; // the stream being scoped to ("" = workspace has none)
    hideSubmitted: boolean;
    me: string; // the connected user, for the "me" shortcut
    refreshKey: number; // bumps when the list reloads → drop per-review caches
    contextReview: number; // id of the review whose context menu is open
    onToggleState: (key: string) => void;
    onUser: (u: string) => void;
    onRole: (r: Role) => void;
    onSearch: (q: string) => void;
    onStreamOnly: (v: boolean) => void;
    onHideSubmitted: (v: boolean) => void;
    onLoadMore: () => void;
    onContent: (r: ReviewRow) => Promise<ReviewContent>;
    /** Cached content (instant); undefined = never fetched (show loading). */
    onContentCached: (r: ReviewRow) => ReviewContent | undefined;
    onDiff: (depotFile: string, rev: number, change: string, submitted: boolean) => Promise<string>;
    onOpenDiff: (depotFile: string, rev: number, change: string, submitted: boolean) => void;
    onContext: (r: ReviewRow, e: MouseEvent) => void;
    onFileContext?: (f: P4Record, r: ReviewRow, e: MouseEvent) => void;
  } = $props();

  type Expanded = {
    open: boolean;
    loading: boolean;
    files: P4Record[];
    /** Changelist the files came from: the shelf, or the submitted change. */
    change: number;
    submitted: boolean;
  };
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
      exp[r.id] = { open: true, loading: false, files: [], change: 0, submitted: false };
      return;
    }
    // Stale-while-revalidate, like the Pending tab's changelists: paint the
    // cached files instantly, then let the fetch reconcile.
    const cached = onContentCached(r);
    exp[r.id] = cached
      ? { open: true, loading: false, ...cached }
      : { open: true, loading: true, files: [], change: r.change, submitted: false };
    const c = await onContent(r);
    exp[r.id] = { open: exp[r.id]?.open ?? true, loading: false, ...c };
  }

  async function toggleDiff(r: ReviewRow, f: P4Record) {
    const s = exp[r.id];
    const key = `${r.id}|${f.depotFile}`;
    const cur = fdiff[key];
    if (cur) {
      fdiff[key] = { ...cur, open: !cur.open };
      if (cur.text || cur.loading) return;
    } else {
      fdiff[key] = { open: true, loading: true, text: "" };
    }
    const text = await onDiff(
      f.depotFile,
      Number(f.rev ?? 0),
      String(s?.change ?? r.change),
      s?.submitted ?? false,
    ).catch((e) => `Could not diff this file: ${e}`);
    fdiff[key] = { open: fdiff[key]?.open ?? true, loading: false, text };
  }

  // The state pills live in a dropdown (five inline cells ate the filter bar).
  let statesOpen = $state(false);
  let statesBox = $state<HTMLElement>();
  $effect(() => {
    if (!statesOpen) return;
    const close = (e: PointerEvent) => {
      if (!statesBox?.contains(e.target as Node)) statesOpen = false;
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") statesOpen = false;
    };
    window.addEventListener("pointerdown", close, true);
    window.addEventListener("keydown", key);
    return () => {
      window.removeEventListener("pointerdown", close, true);
      window.removeEventListener("keydown", key);
    };
  });
  /** Trigger caption: the single ticked state's label, or a count. */
  const stateSummary = $derived.by(() => {
    const on = REVIEW_STATES.filter((s) => states.includes(s.key));
    if (on.length === 0) return "no state";
    if (on.length === 1) return on[0].label;
    return `${on.length} states`;
  });

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

  // Infinite scroll, like the History tab: nearing the bottom fetches the next
  // page. `exhausted` latches when a page adds nothing — Swarm can hand back a
  // cursor with only rows we already have, and without the latch that would ask
  // forever.
  let body = $state<HTMLDivElement>();
  let exhausted = $state(false);
  let lenBeforePage = 0;
  let wasPaging = false;
  $effect(() => {
    void refreshKey; // a filter change starts a fresh list
    exhausted = false;
  });
  $effect(() => {
    if (wasPaging && !paging && rows.length === lenBeforePage) exhausted = true;
    wasPaging = paging;
  });
  function pageIn() {
    lenBeforePage = rows.length;
    onLoadMore();
  }
  function onScroll(e: Event) {
    const el = e.currentTarget as HTMLElement;
    if (!more || paging || exhausted || loading || rows.length === 0) return;
    if (el.scrollTop + el.clientHeight >= el.scrollHeight - 300) pageIn();
  }
  // A page that doesn't fill the view leaves no scrollbar, so the scroll trigger
  // could never fire — keep pulling until it does.
  $effect(() => {
    void rows.length;
    if (!body || !more || paging || exhausted || loading || rows.length === 0) return;
    if (body.scrollHeight <= body.clientHeight + 50) pageIn();
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
    <span class="dd" bind:this={statesBox}>
      <button
        class="pick ddtrigger"
        class:warnish={states.length === 0}
        title="Which review states to list (Swarm filters this server-side)"
        aria-expanded={statesOpen}
        onclick={() => (statesOpen = !statesOpen)}
      >
        {stateSummary} ▾
      </button>
      {#if statesOpen}
        <div class="ddpanel">
          {#each REVIEW_STATES as s (s.key)}
            <button
              class="state st-{s.key} pill"
              class:off={!states.includes(s.key)}
              aria-pressed={states.includes(s.key)}
              onclick={() => onToggleState(s.key)}
            >
              {s.label}
            </button>
          {/each}
        </div>
      {/if}
    </span>

    <label
      class="scope"
      title={streamPath
        ? `Only reviews with a changelist under ${streamPath} — this Swarm serves several projects, and a review from another depot cannot be applied here`
        : "This workspace has no stream, so there is nothing to scope to"}
    >
      <input
        type="checkbox"
        checked={streamOnly}
        disabled={!streamPath}
        onchange={(e) => onStreamOnly(e.currentTarget.checked)}
      />
      this stream
    </label>

    <label
      class="scope"
      title="Hide reviews whose change was already submitted. Swarm leaves those at Needs Review for good, so they otherwise crowd out the ones still waiting to be read."
    >
      <input
        type="checkbox"
        checked={hideSubmitted}
        onchange={(e) => onHideSubmitted(e.currentTarget.checked)}
      />
      hide submitted
    </label>

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

  <div class="body scroll" bind:this={body} onscroll={onScroll}>
    {#if error}
      <div class="msg err">{error}</div>
    {/if}
    {#if loading}
      <div class="msg dim">Loading reviews…</div>
    {:else if rows.length === 0 && !error}
      <div class="msg dim">
        {#if states.length === 0}
          No review state is ticked — pick at least one above.
        {:else}
          No {REVIEW_STATES.filter((s) => states.includes(s.key))
            .map((s) => s.label.toLowerCase())
            .join(" / ")} reviews{user ? ` with ${user} as ${role}` : ""}{search
            ? ` matching “${search}”`
            : ""}{streamOnly && streamPath ? ` on ${streamPath}` : ""}{hideSubmitted
            ? " that aren't submitted yet"
            : ""}.
        {/if}
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
            {#if r.commits.length}
              <span
                class="committed"
                title={`Already submitted as @${[...r.commits].sort((a, b) => b - a)[0]}, though the review is still ${r.stateLabel.toLowerCase()} — submitting deletes the shelf, so the content shown comes from that change.`}
              >
                submitted
              </span>
            {/if}
            <span class="user dim">{r.author}</span>
            <span class="date dim" title={fmtTime(String(r.updated))}>{ago(r.updated)}</span>
          </button>

          {#if s?.open}
            {#if s.loading}
              <div class="finfo dim">Loading files…</div>
            {:else if !r.change}
              <div class="finfo dim">Swarm reports no version for this review.</div>
            {:else if s.files.length === 0}
              <div class="finfo dim">
                Nothing to show: @{r.change} has no shelf{r.commits.length
                  ? " and the submitted change is not readable here"
                  : " (it may have been deleted)"}.
              </div>
            {:else}
              <!-- The depot comes from the files themselves: Swarm's list
                   endpoint doesn't say which stream a review is on. -->
              <div class="finfo dim">
                {s.submitted ? "submitted" : "shelved in"} @{s.change} · {depotOf(s.files[0].depotFile)} ·
                {s.files.length} file{s.files.length === 1 ? "" : "s"}
              </div>
              {#each s.files as f (f.depotFile)}
                {@const fd = fdiff[`${r.id}|${f.depotFile}`]}
                {@const sp = splitPath(f.depotFile)}
                <!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
                <div
                  class="frow mono"
                  title={"Double-click to open the diff window\n" + f.depotFile}
                  ondblclick={() =>
                    onOpenDiff(f.depotFile, Number(f.rev ?? 0), String(s.change), s.submitted)}
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

      {#if paging}
        <div class="foot dim">Loading more…</div>
      {:else if rows.length}
        <div class="foot dim">
          {rows.length} review{rows.length === 1 ? "" : "s"}{more && !exhausted ? "" : " — end of list"}
        </div>
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
  .dd {
    position: relative;
    flex: none;
  }
  .ddtrigger {
    cursor: pointer;
    white-space: nowrap;
  }
  .ddtrigger.warnish {
    color: var(--warn);
  }
  .ddpanel {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    z-index: 40;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 5px;
    padding: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 6px 24px rgba(0, 0, 0, 0.4);
  }
  /* Same cell as the row badges (.state supplies font/color/border); a pill is
     just a clickable one, and unticked reads as an outline ghost. */
  .pill {
    background: none;
    font-family: inherit; /* buttons don't inherit it — the badge cell must match */
    cursor: pointer;
    user-select: none;
  }
  .pill.off {
    color: var(--text-dim);
    opacity: 0.45;
    border-style: dashed;
    font-weight: 400;
    font-style: normal;
  }
  .pill:hover {
    opacity: 1;
  }
  .scope {
    display: flex;
    align-items: center;
    gap: 3px;
    flex: none;
    font-size: 11px;
    color: var(--text-dim);
    white-space: nowrap;
    cursor: pointer;
  }
  .scope input {
    margin: 0;
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
  .committed {
    flex: none;
    font-size: 10px;
    line-height: 16px;
    color: var(--text-dim);
    font-style: italic;
  }
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
    align-items: baseline;
    gap: 5px;
    padding: 2px 10px 2px 20px;
    font-size: 12px;
    white-space: nowrap;
    cursor: default;
    user-select: none;
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
    text-transform: capitalize;
    width: 4rem;
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
  .foot {
    padding: 6px 10px 10px;
    font-size: 11px;
    text-align: center;
  }
</style>
