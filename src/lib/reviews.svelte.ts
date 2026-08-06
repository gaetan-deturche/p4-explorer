//! The Reviews tab: browse Swarm reviews, read their shelved content, apply one.
//!
//! Filters and search go to Swarm, not to a client-side `.filter()` — a server
//! with thousands of reviews cannot be paged into memory to search it, and Swarm
//! already indexes exactly what the tab needs (state, author, participant,
//! keywords). The consequence is that changing a filter refetches; that is why
//! the search is debounced and the state filter is a plain toggle.
//!
//! A review's files and diffs come from Perforce, not Swarm: each version is a
//! shelved changelist, so `describe -S` and the shelved-diff path already in the
//! app serve them.

import { p4, type P4Conn, type P4Record, type ReviewRow } from "$lib/p4";
import { openDiff as openDiffFor } from "$lib/opendiff";
import { cacheGet, cacheSet, storeSet } from "$lib/store.svelte";

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  setError: (m: string) => void;
  setNotice: (m: string, ms?: number) => void;
  /** Depot path of the workspace's stream, for the "this stream only" filter. */
  rootPath: () => string;
};

/** Every review state, in display order, with the same label the row badges
 *  use. The filter is a tickable set of these. */
export const REVIEW_STATES: { key: string; label: string }[] = [
  { key: "needsReview", label: "Needs Review" },
  { key: "needsRevision", label: "Needs Revision" },
  { key: "approved", label: "Approved" },
  { key: "rejected", label: "Rejected" },
  { key: "archived", label: "Archived" },
];
/** Ticked by default: the states that still want something from someone. */
const DEFAULT_STATES = ["needsReview", "needsRevision"];

/** A review's content: the changelist it lives in, and whether that is a
 *  submitted change (no shelf left) rather than a shelf. */
export type ReviewContent = { change: number; submitted: boolean; files: P4Record[] };

export type Role = "author" | "reviewer" | "any";

const PAGE = 25;

let h: Hooks | null = null;
let rows = $state<ReviewRow[]>([]);
let loading = $state(false);
let paging = $state(false);
let error = $state("");
let cursor = 0; // Swarm's `lastSeen`; 0 = no more pages
let states = $state<string[]>([...DEFAULT_STATES]);
let user = $state("");
let role = $state<Role>("author");
let search = $state("");
// On by default: this Swarm serves several projects, and a review from another
// depot can be read but not applied here.
let streamOnly = $state(true);
// Also on by default: a submitted review is settled in practice, whatever state
// Swarm still shows, and here most open reviews are in that position.
let hideSubmitted = $state(true);
let version = $state(0); // bumps when rows change, so children refetch files
let seq = 0; // discards the answer of a superseded request

/** One store key per filter combination, so each revisited view paints from its
 *  own cache. The scope is per client (a review list is server+user specific). */
function cacheScope(): string {
  return `p4:reviews:${h?.conn().client ?? ""}`;
}

// Every row any query has returned, keyed by review id. A filter combination
// never visited still gets instant provisional rows: the pool is filtered
// CLIENT-side with the same predicates while the real query runs. `onStream`
// records whether the row was returned under the stream scope — the one filter
// that cannot be evaluated locally.
type PoolEntry = { r: ReviewRow; s: boolean };
let pool = new Map<number, PoolEntry>();
let poolClient = ""; // pool contents belong to one client
const POOL_CAP = 500;

async function loadPool(): Promise<void> {
  const client = h?.conn().client ?? "";
  if (client === poolClient) return;
  poolClient = client;
  pool = new Map();
  const json = await cacheGet(cacheScope(), "pool"); // SQLite-backed, ~ms
  if (!json) return;
  try {
    for (const e of JSON.parse(json) as PoolEntry[]) pool.set(e.r.id, e);
  } catch {
    /* corrupt — rebuilt as queries land */
  }
}

/** Merge freshly fetched rows into the pool and persist a bounded slice. */
function absorb(fetched: ReviewRow[], onStream: boolean): void {
  for (const r of fetched) {
    const prev = pool.get(r.id);
    pool.set(r.id, { r, s: onStream || (prev?.s ?? false) });
  }
  const slice = [...pool.values()].sort((a, b) => b.r.id - a.r.id).slice(0, POOL_CAP);
  if (slice.length < pool.size) pool = new Map(slice.map((e) => [e.r.id, e]));
  storeSet(cacheScope(), "pool", JSON.stringify(slice));
}

/** The pool filtered with the current controls — client-side approximations of
 *  what Swarm will answer (keywords become a substring match; close enough for
 *  a provisional paint). */
function provisional(): ReviewRow[] {
  const q = search.trim().toLowerCase();
  const out: ReviewRow[] = [];
  for (const { r, s } of pool.values()) {
    if (streamOnly && !s) continue;
    if (!states.includes(r.state)) continue;
    if (hideSubmitted && r.commits.length > 0) continue;
    if (user) {
      const asAuthor = r.author === user;
      const asReviewer = r.reviewers.includes(user);
      if (role === "author" ? !asAuthor : role === "reviewer" ? !asReviewer : !asAuthor && !asReviewer)
        continue;
    }
    if (q && !String(r.id).includes(q) && !r.description.toLowerCase().includes(q)) continue;
    out.push(r);
  }
  return out.sort((a, b) => b.id - a.id);
}
function fingerprint(): string {
  return [
    [...states].sort().join(","),
    user,
    role,
    search.trim(),
    streamOnly ? "s" : "",
    hideSubmitted ? "h" : "",
  ].join("|");
}

export const reviews = {
  init(hooks: Hooks) {
    h = hooks;
  },
  get rows() {
    return rows;
  },
  get loading() {
    return loading;
  },
  get paging() {
    return paging;
  },
  get error() {
    return error;
  },
  get version() {
    return version;
  },
  /** True while more pages are available. */
  get more() {
    return cursor > 0;
  },
  get states() {
    return states;
  },
  get user() {
    return user;
  },
  get role() {
    return role;
  },
  get search() {
    return search;
  },
  get streamOnly() {
    return streamOnly;
  },
  get hideSubmitted() {
    return hideSubmitted;
  },
  /** The stream the filter is scoping to ("" when the workspace has none). */
  get streamPath() {
    return h?.rootPath() ?? "";
  },

  /** Change a filter and reload. Every setter goes through here so a filter can
   *  never be shown without the list it implies. */
  toggleState(key: string) {
    states = states.includes(key) ? states.filter((s) => s !== key) : [...states, key];
    void reviews.load();
  },
  setUser(next: string) {
    const v = next.trim();
    if (v === user) return;
    user = v;
    void reviews.load();
  },
  setRole(next: Role) {
    if (next === role) return;
    role = next;
    if (user) void reviews.load(); // the role only matters with a user
  },
  setSearch(next: string) {
    if (next === search) return;
    search = next;
    void reviews.load();
  },
  setStreamOnly(next: boolean) {
    if (next === streamOnly) return;
    streamOnly = next;
    void reviews.load();
  },
  setHideSubmitted(next: boolean) {
    if (next === hideSubmitted) return;
    hideSubmitted = next;
    void reviews.load();
  },

  /** Fetch the first page for the current filters. */
  async load() {
    if (!h || !h.connected()) {
      rows = [];
      error = "";
      return;
    }
    if (states.length === 0) {
      // No state[] at all would make Swarm return every review ever.
      seq++;
      rows = [];
      cursor = 0;
      error = "";
      loading = false;
      version++;
      return;
    }
    const mine = ++seq;
    // Paint the cached list for this exact filter set before the round-trip —
    // Swarm answers in ~100ms but p4-backed setups deserve the same instant
    // paint every other tab gets; the fetch below reconciles.
    const scope = cacheScope();
    const fp = fingerprint();
    await loadPool();
    // SQLite is the source of truth; this resolves in ~ms and never misses an
    // entry the way the localStorage mirror can.
    const cached = await cacheGet(scope, fp);
    let painted = false;
    if (cached) {
      try {
        const c = JSON.parse(cached) as { rows: ReviewRow[]; cursor: number };
        rows = c.rows;
        cursor = c.cursor;
        painted = true;
        version++;
      } catch {
        /* corrupt cache — the fetch below replaces it */
      }
    }
    if (!painted) {
      // Never asked with THESE filters — but rows from other queries can be
      // filtered locally into a provisional answer while the real one runs.
      const guess = provisional();
      if (guess.length) {
        rows = guess;
        cursor = 0; // unknown; the fetch sets the real paging cursor
        painted = true;
        version++;
      }
    }
    loading = !painted; // with anything on screen, refresh silently
    error = "";
    // Refresh to the depth already on screen: replacing a cached multi-page
    // list with ONE fresh page made the view shrink and then regrow as the
    // fill-effect re-paged. One request — the backend pages internally.
    const depth = painted ? Math.min(Math.max(rows.length, PAGE), 200) : PAGE;
    try {
      const page = await p4.swarmReviews(h.conn(), {
        states,
        user,
        role,
        keywords: search.trim(),
        max: depth,
        after: 0,
        streamPath: streamOnly ? (h.rootPath() ?? "") : "",
        hideSubmitted,
      });
      if (mine !== seq) return; // a newer filter already asked
      // Never swap a SHORTER list over the cached paint while more exists —
      // the backend's scan budget can under-deliver a deep request, and the
      // shrink-then-regrow is exactly the flicker this replaces. Keep the old
      // rows on screen and page until the fresh list reaches the same depth.
      const fresh = [...page.reviews];
      let seen = page.lastSeen;
      let guard = 6;
      while (fresh.length < depth && seen > 0 && guard-- > 0) {
        const more = await p4.swarmReviews(h.conn(), {
          states,
          user,
          role,
          keywords: search.trim(),
          max: PAGE,
          after: seen,
          streamPath: streamOnly ? (h.rootPath() ?? "") : "",
          hideSubmitted,
        });
        if (mine !== seq) return;
        const known = new Set(fresh.map((r) => r.id));
        fresh.push(...more.reviews.filter((r) => !known.has(r.id)));
        seen = more.lastSeen;
        if (more.error) break;
      }
      rows = fresh;
      cursor = seen;
      error = page.error;
      if (!page.error) {
        storeSet(scope, fp, JSON.stringify({ rows, cursor }));
        absorb(fresh, streamOnly);
      }
      version++;
    } catch (e) {
      if (mine !== seq) return;
      rows = [];
      cursor = 0;
      error = String(e);
    } finally {
      if (mine === seq) loading = false;
    }
  },

  /** Append the next page (the list pages on scroll). */
  async loadMore() {
    if (!h || !h.connected() || !cursor || paging) return;
    const mine = seq;
    paging = true;
    try {
      const page = await p4.swarmReviews(h.conn(), {
        states,
        user,
        role,
        keywords: search.trim(),
        max: PAGE,
        after: cursor,
        streamPath: streamOnly ? (h.rootPath() ?? "") : "",
        hideSubmitted,
      });
      if (mine !== seq) return; // filters changed under us; that load owns `rows`
      // Swarm can repeat a row across pages when reviews are updated mid-paging.
      const known = new Set(rows.map((r) => r.id));
      rows = [...rows, ...page.reviews.filter((r) => !known.has(r.id))];
      cursor = page.lastSeen;
      if (page.error) error = page.error;
      storeSet(cacheScope(), fingerprint(), JSON.stringify({ rows, cursor }));
      absorb(page.reviews, streamOnly);
      // Deliberately NOT bumping `version`: a page is an append, so the rows
      // already on screen (and their loaded file lists) are still valid. Bumping
      // it would collapse every expanded review on each scroll.
    } catch (e) {
      if (mine === seq) error = String(e);
    } finally {
      if (mine === seq) paging = false;
    }
  },

  /** Where a review's content actually is, and its files.
   *
   *  Normally the shelf on the review's own changelist. But a review that was
   *  submitted without waiting for approval has no shelf left — submitting
   *  deletes it — while Swarm still reports it as needing review. For those the
   *  content is the submitted changelist, so the tab shows that instead of
   *  claiming the review is empty. */
  /** The cached content of a review, or undefined when never fetched. Lets the
   *  list paint an expansion instantly while `content` reconciles. */
  async contentCached(row: ReviewRow): Promise<ReviewContent | undefined> {
    if (!h || !row.change) return undefined;
    // Through ALL cache layers (mem → localStorage → SQLite): the sync layers
    // evict, and a post-boot expansion should still paint from disk in ~ms.
    const json = await cacheGet(`p4:rvcontent:${h.conn().client}`, String(row.change));
    if (json === null) return undefined;
    try {
      return JSON.parse(json) as ReviewContent;
    } catch {
      return undefined;
    }
  },

  async content(row: ReviewRow): Promise<ReviewContent> {
    if (!h || !row.change) return { change: row.change, submitted: false, files: [] };
    const conn = h.conn();
    const shelved = await p4
      .describeShelved(conn, String(row.change))
      .then((recs) => recs.filter((r) => r.depotFile))
      .catch(() => [] as P4Record[]);
    const save = (c: ReviewContent): ReviewContent => {
      cacheSet(`p4:rvcontent:${conn.client}`, String(row.change), JSON.stringify(c));
      return c;
    };
    if (shelved.length) return save({ change: row.change, submitted: false, files: shelved });

    // Newest commit first: a review can land in more than one changelist.
    const commit = [...row.commits].sort((a, b) => b - a)[0];
    if (!commit) return { change: row.change, submitted: false, files: [] };
    const files = await p4
      .describe(conn, String(commit))
      .then((recs) => recs.filter((r) => r.depotFile))
      .catch(() => [] as P4Record[]);
    // An empty result is not cached: it may be a transient p4 error, and a
    // stale "nothing here" is worse than refetching next time.
    return files.length ? save({ change: commit, submitted: true, files }) : { change: commit, submitted: true, files };
  },

  /** Inline diff of one file, from wherever the content is. */
  diff(file: string, rev: number, change: string, submitted: boolean): Promise<string> {
    return submitted
      ? p4.diff2(h!.conn(), file, rev) // the submitted revision against the one before
      : p4.diffShelved(h!.conn(), file, rev, change);
  },

  /** Open the diff outside the list: Unreal's asset diff for a `.uasset`, else
   *  the in-app diff window or the external P4DIFF tool, exactly as the History
   *  and Pending tabs do. The two sides come from the shelf or from the submitted
   *  revision, depending on where this review's content is. */
  openDiff(file: string, rev: number, change: string, submitted: boolean) {
    return openDiffFor(
      h!.conn(),
      submitted ? { kind: "rev", file, rev } : { kind: "shelved", file, rev, change },
      h!.setNotice,
    );
  },

  /** The review's page on the Swarm server, or "" when unconfigured. */
  async url(id: number): Promise<string> {
    if (!h) return "";
    const base = await p4.swarmUrl(h.conn()).catch(() => "");
    return base ? `${base}/reviews/${id}` : "";
  },
};
