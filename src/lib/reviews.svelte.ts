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

type Hooks = {
  conn: () => P4Conn;
  connected: () => boolean;
  setError: (m: string) => void;
  /** Depot path of the workspace's stream, for the "this stream only" filter. */
  rootPath: () => string;
};

/** Swarm states that count as "still open" — what the tab shows by default. */
export const OPEN_STATES = ["needsReview", "needsRevision"];
/** Every state, for the "All" option (Swarm has no "any state" filter). */
export const ALL_STATES = [
  "needsReview",
  "needsRevision",
  "approved",
  "rejected",
  "archived",
];

export type Role = "author" | "reviewer" | "any";
export type StatusFilter = "open" | "needsReview" | "needsRevision" | "approved" | "rejected" | "all";

const PAGE = 25;

let h: Hooks | null = null;
let rows = $state<ReviewRow[]>([]);
let loading = $state(false);
let paging = $state(false);
let error = $state("");
let cursor = 0; // Swarm's `lastSeen`; 0 = no more pages
let status = $state<StatusFilter>("open");
let user = $state("");
let role = $state<Role>("author");
let search = $state("");
// On by default: this Swarm serves several projects, and a review from another
// depot can be read but not applied here.
let streamOnly = $state(true);
let version = $state(0); // bumps when rows change, so children refetch files
let seq = 0; // discards the answer of a superseded request

/** The `state[]` values a status filter maps to. */
function statesFor(f: StatusFilter): string[] {
  if (f === "open") return OPEN_STATES;
  if (f === "all") return ALL_STATES;
  return [f];
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
  get status() {
    return status;
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
  /** The stream the filter is scoping to ("" when the workspace has none). */
  get streamPath() {
    return h?.rootPath() ?? "";
  },

  /** Change a filter and reload. Every setter goes through here so a filter can
   *  never be shown without the list it implies. */
  setStatus(next: StatusFilter) {
    if (next === status) return;
    status = next;
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

  /** Fetch the first page for the current filters. */
  async load() {
    if (!h || !h.connected()) {
      rows = [];
      error = "";
      return;
    }
    const mine = ++seq;
    loading = true;
    error = "";
    try {
      const page = await p4.swarmReviews(h.conn(), {
        states: statesFor(status),
        user,
        role,
        keywords: search.trim(),
        max: PAGE,
        after: 0,
        streamPath: streamOnly ? (h.rootPath() ?? "") : "",
      });
      if (mine !== seq) return; // a newer filter already asked
      rows = page.reviews;
      cursor = page.lastSeen;
      error = page.error;
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
        states: statesFor(status),
        user,
        role,
        keywords: search.trim(),
        max: PAGE,
        after: cursor,
        streamPath: streamOnly ? (h.rootPath() ?? "") : "",
      });
      if (mine !== seq) return; // filters changed under us; that load owns `rows`
      // Swarm can repeat a row across pages when reviews are updated mid-paging.
      const known = new Set(rows.map((r) => r.id));
      rows = [...rows, ...page.reviews.filter((r) => !known.has(r.id))];
      cursor = page.lastSeen;
      if (page.error) error = page.error;
      // Deliberately NOT bumping `version`: a page is an append, so the rows
      // already on screen (and their loaded file lists) are still valid. Bumping
      // it would collapse every expanded review on each scroll.
    } catch (e) {
      if (mine === seq) error = String(e);
    } finally {
      if (mine === seq) paging = false;
    }
  },

  /** Shelved files of a review's current version. */
  async files(change: string): Promise<P4Record[]> {
    if (!h || !change || change === "0") return [];
    return p4.describeShelved(h.conn(), change).then(
      (recs) => recs.filter((r) => r.depotFile),
      () => [],
    );
  },

  /** Inline diff of one shelved file against its base revision. */
  diff(file: string, rev: number, change: string): Promise<string> {
    return p4.diffShelved(h!.conn(), file, rev, change);
  },

  /** Open the same diff in the diff window. */
  openDiff(file: string, rev: number, change: string) {
    void p4.openDiffShelved(h!.conn(), file, rev, change).catch((e) => h!.setError(String(e)));
  },

  /** The review's page on the Swarm server, or "" when unconfigured. */
  async url(id: number): Promise<string> {
    if (!h) return "";
    const base = await p4.swarmUrl(h.conn()).catch(() => "");
    return base ? `${base}/reviews/${id}` : "";
  },
};
